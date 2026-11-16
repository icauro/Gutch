//! Turns destination differences into reconstruction jobs and change lists.

use crate::Error;
use crate::Result;
use crate::artifact::hash::ChunkHash;
use crate::artifact::integrity::verify_full_file_integrity;
use crate::artifact::reference::ArtifactRef;
use crate::filesystem::paths::ManifestPathResolver;
use crate::installation::contracts::SnapshotReconstructSource;
use crate::installation::execute::job_io::build_source_chunk_locations;
use crate::installation::execute::jobs::{
    ReconstructArtifactChunk, ReconstructArtifactJob, ReconstructJob, ReconstructPipelineFile,
    ReconstructPipelinePlan, ReconstructSourceChunkJob, ReconstructTarget, SourceChunkLocation,
};
use crate::installation::inspect::InspectionSpans;
use crate::installation::inspect::InstallationIssues;
use crate::installation::inspect::InstallationVerifyMode;
use crate::installation::inspect::VerifyRequest;
use crate::installation::inspect::classify_directory;
use crate::manifest::snapshot::SnapshotManifest;
use crate::manifest::{ChunkRef, FileManifest, SnapshotEntry};
use crate::progress::OperationControl;
use crate::repo::artifacts::required_artifacts;
use crate::repo::index::ChunkIndex;
use crate::repo::paths::RepoPaths;
use crate::runtime::RuntimeResourceHints;
use crate::transfer::download::ArtifactPriority;
use crate::transfer::download::PendingArtifactDownload;
use rayon::ThreadPool;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::hash_map::Entry;
use std::fs;
use std::mem;
use std::path::Path;

pub(crate) fn build_artifact_priorities(
    pipeline_plan: &ReconstructPipelinePlan,
    pending_artifact_downloads: &[PendingArtifactDownload],
) -> HashMap<ArtifactRef, ArtifactPriority> {
    pending_artifact_downloads
        .iter()
        .map(|download| {
            let mut priority = ArtifactPriority::default();
            if let Some(file_indices) = pipeline_plan
                .missing_artifact_to_file_indices
                .get(&download.artifact)
            {
                for &file_index in file_indices {
                    let remaining = pipeline_plan.missing_artifact_counts[file_index];
                    if remaining == 0 {
                        continue;
                    }
                    if remaining == 1 {
                        priority.ready_file_bytes = priority
                            .ready_file_bytes
                            .saturating_add(pipeline_plan.files[file_index].size);
                    } else {
                        priority.near_ready_score = priority.near_ready_score.saturating_add(
                            pipeline_plan.files[file_index].size / remaining as u64,
                        );
                    }
                    priority.dependent_file_count += 1;
                }
            }
            (download.artifact.clone(), priority)
        })
        .collect()
}

fn can_copy_source_file(file: &FileManifest, source_files: &HashMap<&str, &FileManifest>) -> bool {
    source_files.get(file.path.as_str()).is_some_and(|source| {
        source.full_file_hash == file.full_file_hash && source.size == file.size
    })
}

fn build_source_locations_for_target(
    source: SnapshotReconstructSource<'_>,
    target: &SnapshotManifest,
    source_files: &HashMap<&str, &FileManifest>,
) -> Result<HashMap<ChunkHash, SourceChunkLocation>> {
    let required_chunk_count: usize = target
        .files()
        .filter(|file| !can_copy_source_file(file, source_files))
        .map(|file| file.chunks.len())
        .sum();
    let source_chunk_count: usize = source
        .snapshot_manifest
        .files()
        .map(|file| file.chunks.len())
        .sum();
    // Do not allocate a filter whose reference bound is as large as the source
    // input. Full-source reuse keeps the direct index path.
    let required_chunks = if required_chunk_count < source_chunk_count {
        Some(
            target
                .files()
                .filter(|file| !can_copy_source_file(file, source_files))
                .flat_map(|file| file.chunks.iter().map(|chunk| chunk.chunk_hash))
                .collect::<HashSet<_>>(),
        )
    } else {
        None
    };
    build_source_chunk_locations(source, required_chunks.as_ref())
}

pub(crate) fn build_reconstruct_pipeline_plan(
    snapshot_manifest: &SnapshotManifest,
    chunk_index: &ChunkIndex,
    repo_paths: &RepoPaths,
    existing_artifacts: &HashSet<ArtifactRef>,
    verified_source: Option<SnapshotReconstructSource<'_>>,
) -> Result<ReconstructPipelinePlan> {
    let mut builder = PipelinePlanBuilder::new(chunk_index, repo_paths, snapshot_manifest);
    let source_files_by_path = verified_source
        .map(|source| {
            source
                .snapshot_manifest
                .files()
                .map(|file| (file.path.as_str(), file))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let mut source_chunk_locations = None;
    let source_paths = verified_source
        .map(|source| ManifestPathResolver::new(source.destination_dir))
        .transpose()?;

    for file_manifest in snapshot_manifest.files() {
        if can_copy_source_file(file_manifest, &source_files_by_path) {
            let source_paths = source_paths
                .as_ref()
                .expect("source files map only exists with source");
            builder.add_source_file_copy(file_manifest, source_paths)?;
            continue;
        }
        // Whole-file copies need no chunk-location index. A partial reuse still
        // searches every verified source file, including those copied above.
        if source_chunk_locations.is_none() {
            source_chunk_locations = Some(
                verified_source
                    .map(|source| {
                        build_source_locations_for_target(
                            source,
                            snapshot_manifest,
                            &source_files_by_path,
                        )
                    })
                    .transpose()?
                    .unwrap_or_default(),
            );
        }
        let source_chunk_locations = source_chunk_locations
            .as_ref()
            .expect("source chunk locations initialized above");
        builder.add_chunked_file(file_manifest, source_chunk_locations)?;
    }

    let required_chunk_count = snapshot_manifest
        .files()
        .map(|file| file.chunks.len())
        .sum();
    Ok(builder.finish(existing_artifacts, required_chunk_count))
}

/// Collects reconstruction jobs while walking the target manifest's files.
struct PipelinePlanBuilder<'a> {
    chunk_index: &'a ChunkIndex,
    repo_paths: &'a RepoPaths,
    files: Vec<ReconstructPipelineFile>,
    jobs: Vec<ReconstructJob>,
    /// One indexed location per hash; repeated references reuse its job and slot.
    artifact_chunk_positions: HashMap<ChunkHash, (usize, usize)>,
    /// Targets per job and chunk slot. Jobs and their chunks already have dense
    /// indices, so the targets use the same layout.
    chunk_targets: Vec<Vec<Vec<ReconstructTarget>>>,
    artifact_job_indices: HashMap<ArtifactRef, usize>,
    /// The source catalog fixes one location for each hash; the size is
    /// checked before lookup.
    source_chunk_job_indices: HashMap<ChunkHash, usize>,
    reused_chunk_count: usize,
}

impl<'a> PipelinePlanBuilder<'a> {
    fn new(
        chunk_index: &'a ChunkIndex,
        repo_paths: &'a RepoPaths,
        snapshot_manifest: &SnapshotManifest,
    ) -> Self {
        Self {
            chunk_index,
            repo_paths,
            files: Vec::with_capacity(snapshot_manifest.file_count()),
            jobs: Vec::new(),
            artifact_chunk_positions: HashMap::new(),
            chunk_targets: Vec::new(),
            artifact_job_indices: HashMap::new(),
            source_chunk_job_indices: HashMap::new(),
            reused_chunk_count: 0,
        }
    }

    fn push_file(&mut self, file_manifest: &FileManifest) -> Result<()> {
        self.files.push(ReconstructPipelineFile {
            path: file_manifest.path.clone(),
            size: file_manifest.size,
            full_file_hash: file_manifest.full_file_hash,
        });
        Ok(())
    }

    /// Copies an identical file from the verified source installation.
    fn add_source_file_copy(
        &mut self,
        file_manifest: &FileManifest,
        source_paths: &ManifestPathResolver,
    ) -> Result<()> {
        let file_index = self.files.len();
        let source_path = source_paths.resolve(&file_manifest.path)?;
        self.push_file(file_manifest)?;
        self.jobs.push(ReconstructJob::CopySourceFile {
            file_index,
            source_path,
        });
        self.reused_chunk_count += file_manifest.chunks.len();
        Ok(())
    }

    /// Writes a file chunk by chunk, taking each chunk from the verified
    /// source when it has one and from a repository artifact otherwise.
    fn add_chunked_file(
        &mut self,
        file_manifest: &FileManifest,
        source_chunk_locations: &HashMap<ChunkHash, SourceChunkLocation>,
    ) -> Result<()> {
        let file_index = self.files.len();
        let mut file_offset = 0u64;
        for chunk_ref in &file_manifest.chunks {
            let target = ReconstructTarget {
                file_index,
                file_offset,
            };
            let source_chunk = source_chunk_locations
                .get(&chunk_ref.chunk_hash)
                .filter(|source_chunk| source_chunk.raw_size == u64::from(chunk_ref.raw_size));
            match source_chunk {
                Some(source_chunk) => self.target_source_chunk(chunk_ref, source_chunk, target),
                None => self.target_artifact_chunk(chunk_ref, &file_manifest.path, target)?,
            }
            file_offset = file_offset
                .checked_add(u64::from(chunk_ref.raw_size))
                .ok_or_else(|| {
                    Error::manifest(format!("file '{}' size overflows u64", file_manifest.path))
                })?;
        }
        if file_offset != file_manifest.size {
            return Err(Error::manifest(format!(
                "file '{}' chunk sizes total {}, expected {}",
                file_manifest.path, file_offset, file_manifest.size
            )));
        }
        self.push_file(file_manifest)
    }

    fn target_source_chunk(
        &mut self,
        chunk_ref: &ChunkRef,
        source_chunk: &SourceChunkLocation,
        target: ReconstructTarget,
    ) {
        self.reused_chunk_count += 1;
        if let Some(&job_index) = self.source_chunk_job_indices.get(&chunk_ref.chunk_hash) {
            let ReconstructJob::SourceChunk(source_job) = &mut self.jobs[job_index] else {
                unreachable!("source chunk job index points to another job kind");
            };
            source_job.additional_targets.push(target);
            return;
        }
        self.source_chunk_job_indices
            .insert(chunk_ref.chunk_hash, self.jobs.len());
        self.jobs
            .push(ReconstructJob::SourceChunk(ReconstructSourceChunkJob {
                chunk_hash: chunk_ref.chunk_hash,
                raw_size: chunk_ref.raw_size,
                source_path: source_chunk.path.clone(),
                source_offset: source_chunk.offset,
                first_target: target,
                additional_targets: Vec::new(),
            }));
    }

    fn target_artifact_chunk(
        &mut self,
        chunk_ref: &ChunkRef,
        file_path: &str,
        target: ReconstructTarget,
    ) -> Result<()> {
        let (job_index, position) = self.artifact_chunk_slot(chunk_ref)?;
        let ReconstructJob::Artifact(artifact_job) = &self.jobs[job_index] else {
            unreachable!("artifact job index points to another job kind");
        };
        let indexed_raw_size = artifact_job.chunks[position].raw_size;
        if indexed_raw_size != chunk_ref.raw_size {
            return Err(Error::manifest(format!(
                "chunk '{}' size mismatch in file '{file_path}': manifest {}, index {indexed_raw_size}",
                chunk_ref.chunk_hash, chunk_ref.raw_size
            )));
        }
        self.chunk_targets[job_index][position].push(target);
        Ok(())
    }

    /// Returns the job and chunk slot that produce `chunk_ref`, adding the
    /// artifact job and the slot on first use.
    fn artifact_chunk_slot(&mut self, chunk_ref: &ChunkRef) -> Result<(usize, usize)> {
        let hash = chunk_ref.chunk_hash;
        let position_entry = match self.artifact_chunk_positions.entry(hash) {
            Entry::Occupied(position) => return Ok(*position.get()),
            Entry::Vacant(position_entry) => position_entry,
        };
        let entry = self.chunk_index.entries.get(&hash).ok_or_else(|| {
            Error::manifest(format!(
                "missing chunk index entry for required chunk '{hash}'"
            ))
        })?;
        let artifact = ArtifactRef::from_chunk_entry(&hash, entry);
        let job_index = match self.artifact_job_indices.get(&artifact) {
            Some(&job_index) => job_index,
            None => {
                let job_index = self.jobs.len();
                self.jobs
                    .push(ReconstructJob::Artifact(ReconstructArtifactJob {
                        artifact: artifact.clone(),
                        artifact_path: artifact.local_path(self.repo_paths)?,
                        encoded_size: entry.artifact_size,
                        decoded_size: 0,
                        chunks: Vec::new(),
                        targets: Vec::new(),
                    }));
                // Copy and source jobs keep empty slots so artifact slots keep
                // their job indices.
                self.chunk_targets.resize_with(self.jobs.len(), Vec::new);
                self.artifact_job_indices.insert(artifact, job_index);
                job_index
            }
        };
        let ReconstructJob::Artifact(artifact_job) = &mut self.jobs[job_index] else {
            unreachable!("artifact job index points to another job kind");
        };
        let position = artifact_job.chunks.len();
        self.chunk_targets[job_index].push(Vec::new());
        artifact_job.chunks.push(ReconstructArtifactChunk {
            chunk_hash: hash,
            raw_size: entry.raw_size,
            target_range: 0..0,
        });
        Ok(*position_entry.insert((job_index, position)))
    }

    /// Moves each chunk's targets into its job, records which files wait on
    /// each missing artifact, and orders the jobs.
    /// Sums each used artifact's chunks from the index; jobs list only the required ones.
    fn resolve_decoded_sizes(&mut self) {
        if self.artifact_job_indices.is_empty() {
            return;
        }
        for (hash, entry) in self.chunk_index.entries.iter() {
            let artifact = ArtifactRef::from_chunk_entry(hash, entry);
            if let Some(&job_index) = self.artifact_job_indices.get(&artifact)
                && let ReconstructJob::Artifact(job) = &mut self.jobs[job_index]
            {
                job.decoded_size += u64::from(entry.raw_size);
            }
        }
    }

    fn finish(
        mut self,
        existing_artifacts: &HashSet<ArtifactRef>,
        required_chunk_count: usize,
    ) -> ReconstructPipelinePlan {
        self.resolve_decoded_sizes();
        let file_count = self.files.len();
        let mut missing_artifact_counts = vec![0; file_count];
        let mut missing_artifact_to_file_indices = HashMap::new();
        // Only missing artifacts need dependency scratch; reuse it between jobs.
        let mut seen_dependent_files = Vec::new();
        // Resolve ranges before jobs and chunks are sorted: slots use insertion order.
        for (job_index, job) in self.jobs.iter_mut().enumerate() {
            let ReconstructJob::Artifact(job) = job else {
                continue;
            };
            for (position, chunk) in job.chunks.iter_mut().enumerate() {
                let start = job.targets.len();
                job.targets
                    .extend(mem::take(&mut self.chunk_targets[job_index][position]));
                chunk.target_range = start..job.targets.len();
            }
            // Artifact targets are the authoritative file dependencies for this job.
            if !existing_artifacts.contains(&job.artifact) {
                seen_dependent_files.resize(file_count, false);
                let mut file_indices = Vec::new();
                for target in &job.targets {
                    if !seen_dependent_files[target.file_index] {
                        seen_dependent_files[target.file_index] = true;
                        file_indices.push(target.file_index);
                    }
                }
                file_indices.sort_unstable();
                for &file_index in &file_indices {
                    missing_artifact_counts[file_index] += 1;
                    seen_dependent_files[file_index] = false;
                }
                missing_artifact_to_file_indices.insert(job.artifact.clone(), file_indices);
            }
            job.chunks.sort_by_key(|chunk| chunk.chunk_hash);
        }
        // Source chunks run first, in source file order.
        self.jobs.sort_by(|left, right| match (left, right) {
            (ReconstructJob::SourceChunk(left), ReconstructJob::SourceChunk(right)) => {
                (&left.source_path, left.source_offset)
                    .cmp(&(&right.source_path, right.source_offset))
            }
            (ReconstructJob::SourceChunk(_), _) => Ordering::Less,
            (_, ReconstructJob::SourceChunk(_)) => Ordering::Greater,
            _ => Ordering::Equal,
        });
        // The job map already owns one entry for each artifact the plan uses.
        let required_artifacts: HashSet<_> = self.artifact_job_indices.into_keys().collect();
        let required_bundle_count = required_artifacts
            .iter()
            .filter(|artifact| artifact.is_bundle())
            .count();
        ReconstructPipelinePlan {
            files: self.files,
            jobs: self.jobs,
            missing_artifact_to_file_indices,
            missing_artifact_counts,
            required_bundle_count,
            required_artifact_count: required_artifacts.len(),
            required_artifacts,
            required_chunk_count,
            reused_chunk_count: self.reused_chunk_count,
        }
    }
}

pub(crate) struct InstallationChanges {
    pub(crate) metadata_updates: Vec<SnapshotEntry>,
    pub(crate) classification: InstallationIssues,
    pub(crate) affected_manifest: SnapshotManifest,
    pub(crate) missing_file_count: usize,
    pub(crate) corrupt_file_count: usize,
    pub(crate) extra_file_count: usize,
    pub(crate) needs_rebuild: bool,
    pub(crate) required_artifacts: HashSet<ArtifactRef>,
}

pub(crate) fn inspect_installation_changes(
    snapshot_manifest: &SnapshotManifest,
    chunk_index: Option<&ChunkIndex>,
    destination_dir: &Path,
    inspection: InstallationVerifyMode,
    runtime_hints: &RuntimeResourceHints,
    control: &OperationControl,
    operation_pool: Option<&ThreadPool>,
) -> Result<InstallationChanges> {
    let (mut classification, _) = classify_directory(VerifyRequest {
        snapshot_manifest,
        dir: destination_dir,
        mode: inspection,
        runtime_hints,
        control: control.clone(),
        spans: &InspectionSpans::REPAIR,
        pool: operation_pool,
    })?;
    let updates = if inspection == InstallationVerifyMode::MetadataOnly {
        Vec::new()
    } else {
        resolve_metadata_updates(
            snapshot_manifest,
            &mut classification,
            destination_dir,
            control,
        )?
    };
    let mut changes = build_installation_changes(snapshot_manifest, chunk_index, classification)?;
    changes.corrupt_file_count += updates.len();
    changes.metadata_updates = updates;
    Ok(changes)
}

pub(crate) fn build_installation_changes(
    snapshot_manifest: &SnapshotManifest,
    chunk_index: Option<&ChunkIndex>,
    classification: InstallationIssues,
) -> Result<InstallationChanges> {
    let missing_file_count = classification.missing_file_count();
    let corrupt_file_count = classification.corrupt_file_count();
    let extra_file_count = classification.extra_file_count();
    let needs_rebuild = !classification.matches_snapshot();
    let affected_manifest = subset_manifest_for_issues(snapshot_manifest, &classification)?;
    let required_artifacts = if needs_rebuild {
        chunk_index
            .map(|index| required_artifacts(&affected_manifest, index))
            .transpose()?
            .unwrap_or_default()
    } else {
        HashSet::new()
    };

    Ok(InstallationChanges {
        metadata_updates: Vec::new(),
        classification,
        affected_manifest,
        missing_file_count,
        corrupt_file_count,
        extra_file_count,
        needs_rebuild,
        required_artifacts,
    })
}

/// A permissions mismatch does not require rebuilding a file whose bytes still match.
pub(crate) fn resolve_metadata_updates(
    target: &SnapshotManifest,
    classification: &mut InstallationIssues,
    destination: &Path,
    control: &OperationControl,
) -> Result<Vec<SnapshotEntry>> {
    let mut updates = Vec::new();
    let destination = ManifestPathResolver::new(destination)?;
    let entries: HashMap<_, _> = target
        .entries
        .iter()
        .map(|entry| (entry.path(), entry))
        .collect();
    for issue in &classification.corrupt_files {
        control.checkpoint()?;
        let Some(entry) = entries.get(issue.path.as_str()) else {
            continue;
        };
        let path = destination.resolve_entry(entry.path())?;
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        let matches = match entry {
            SnapshotEntry::Directory(_) => metadata.file_type().is_dir(),
            SnapshotEntry::File(file)
                if metadata.file_type().is_file() && metadata.len() == file.size =>
            {
                let metadata_differs = metadata.permissions().readonly() != file.read_only
                    || (cfg!(unix)
                        && crate::filesystem::is_executable(&metadata) != file.executable);
                if !metadata_differs {
                    false
                } else {
                    match verify_full_file_integrity(
                        &path,
                        file.size,
                        file.full_file_hash,
                        1,
                        control,
                    ) {
                        Ok(_) => true,
                        Err(Error::Cancelled) => return Err(Error::Cancelled),
                        Err(_) => false,
                    }
                }
            }
            _ => false,
        };
        if matches {
            updates.push((*entry).clone());
        }
    }
    let paths: HashSet<_> = updates.iter().map(|entry| entry.path()).collect();
    classification
        .corrupt_files
        .retain(|issue| !paths.contains(issue.path.as_str()));
    Ok(updates)
}

/// `whole * part / total`, assuming the subset compresses like the snapshot.
fn proportional_estimate(whole: u64, part: u64, total: u64) -> u64 {
    if total == 0 {
        return 0;
    }
    let scaled = u128::from(whole) * u128::from(part) / u128::from(total);
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

fn subset_manifest_for_issues(
    snapshot_manifest: &SnapshotManifest,
    classification: &InstallationIssues,
) -> Result<SnapshotManifest> {
    let affected_paths = classification
        .missing_files
        .iter()
        .chain(classification.corrupt_files.iter())
        .map(|issue| issue.path.as_str())
        .collect::<HashSet<_>>();
    // Link targets stay out unless they need repair too: links are created in
    // the destination, where an intact target already is.
    let mut subset = snapshot_manifest.clone();
    subset.entries = snapshot_manifest
        .entries
        .iter()
        .filter(|entry| affected_paths.contains(entry.path()))
        .cloned()
        .collect();
    subset.total_logical_size = subset.files().map(|file| file.size).sum();
    subset.total_compressed_size_estimate = proportional_estimate(
        snapshot_manifest.total_compressed_size_estimate,
        subset.total_logical_size,
        snapshot_manifest.total_logical_size,
    );
    if !affected_paths.is_empty() && subset.entries.is_empty() {
        return Err(Error::verify(
            "repair classification referenced entries that are not present in the snapshot manifest",
        ));
    }
    Ok(subset)
}

#[cfg(test)]
mod tests {
    use super::proportional_estimate;
    use super::subset_manifest_for_issues;
    use crate::installation::inspect::{
        InstallationIssue, InstallationIssueKind, InstallationIssues,
    };
    use crate::manifest::SnapshotEntry;
    use crate::manifest::SnapshotTagSet;
    use crate::manifest::SymlinkManifest;
    use crate::manifest::snapshot::SnapshotManifest;

    #[test]
    fn repair_subset_leaves_out_intact_link_targets() {
        let mut manifest = SnapshotManifest::new("s".into(), SnapshotTagSet::default());
        manifest.entries.push(SnapshotEntry::Directory(
            crate::manifest::DirectoryManifest {
                path: "data".into(),
                read_only: false,
            },
        ));
        manifest
            .entries
            .push(SnapshotEntry::Symlink(SymlinkManifest {
                path: "current".into(),
                target_path: "data".into(),
            }));
        let classification = InstallationIssues {
            missing_files: vec![InstallationIssue {
                kind: InstallationIssueKind::Missing,
                path: "current".into(),
                detail: String::new(),
            }],
            ..Default::default()
        };

        let subset = subset_manifest_for_issues(&manifest, &classification).unwrap();

        let paths: Vec<_> = subset.entries.iter().map(SnapshotEntry::path).collect();
        assert_eq!(paths, ["current"]);
    }

    #[test]
    fn repair_subset_estimate_scales_with_logical_size() {
        assert_eq!(proportional_estimate(600, 25, 100), 150);
        assert_eq!(proportional_estimate(600, 0, 100), 0);
        assert_eq!(proportional_estimate(600, 0, 0), 0);
        assert_eq!(
            proportional_estimate(u64::MAX, u64::MAX, u64::MAX),
            u64::MAX
        );
    }
}
