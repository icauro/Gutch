//! Turns destination differences into reconstruction jobs and change lists.

use crate::Error;
use crate::Result;
use crate::artifact::hash::ChunkHash;
use crate::artifact::integrity::verify_full_file_integrity;
use crate::artifact::reference::ArtifactRef;
use crate::filesystem::paths::resolve_manifest_relative_path;
use crate::installation::contracts::SnapshotReconstructSource;
use crate::installation::execute::job_io::build_source_chunk_locations;
use crate::installation::execute::jobs::{
    ReconstructArtifactChunk, ReconstructArtifactJob, ReconstructJob, ReconstructPipelineFile,
    ReconstructPipelinePlan, ReconstructSourceChunkJob, ReconstructTarget, SourceChunkLocation,
};
use crate::installation::inspect::RepairClassification;
use crate::installation::inspect::VerifyMode;
use crate::installation::inspect::VerifyRequest;
use crate::installation::inspect::classify_directory_with_marker_namespace_and_pool;
use crate::manifest::snapshot::SnapshotManifest;
use crate::manifest::{FileManifest, SnapshotEntry};
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
    let mut files = Vec::with_capacity(snapshot_manifest.file_count());
    let mut jobs = Vec::new();
    // One indexed location per hash; repeated references reuse its resolved job and slot.
    let mut artifact_chunk_positions: HashMap<ChunkHash, (usize, usize)> = HashMap::new();
    // Jobs and their chunks already have dense indices; targets use the same layout.
    let mut chunk_targets: Vec<Vec<Vec<ReconstructTarget>>> = Vec::new();
    let mut artifact_job_indices: HashMap<ArtifactRef, usize> = HashMap::new();
    // The source catalog fixes one location for each hash; size is checked before lookup.
    let mut source_chunk_job_indices: HashMap<ChunkHash, usize> = HashMap::new();
    let mut missing_artifact_to_file_indices: HashMap<ArtifactRef, Vec<usize>> = HashMap::new();
    let mut missing_artifact_counts = Vec::with_capacity(snapshot_manifest.file_count());
    let mut reused_source_chunk_count = 0usize;
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

    for file_manifest in snapshot_manifest.files() {
        if can_copy_source_file(file_manifest, &source_files_by_path) {
            let source_root = verified_source.expect("source files map only exists with source");
            let file_index = files.len();
            let source_path =
                resolve_manifest_relative_path(source_root.destination_dir, &file_manifest.path)?;
            files.push(ReconstructPipelineFile {
                path: file_manifest.path.clone(),
                size: file_manifest.size,
                full_file_hash: ChunkHash::parse_hex(&file_manifest.full_file_hash)?,
            });
            jobs.push(ReconstructJob::CopySourceFile {
                file_index,
                source_path,
            });
            missing_artifact_counts.push(0);
            reused_source_chunk_count += file_manifest.chunks.len();
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
        let file_index = files.len();
        let mut file_offset = 0u64;
        for chunk_ref in &file_manifest.chunks {
            if let Some(source_chunk) = source_chunk_locations
                .get(&chunk_ref.chunk_hash)
                .filter(|source_chunk| source_chunk.raw_size == u64::from(chunk_ref.raw_size))
            {
                let source_key = chunk_ref.chunk_hash;
                let target = ReconstructTarget {
                    file_index,
                    file_offset,
                };
                if let Some(&job_index) = source_chunk_job_indices.get(&source_key) {
                    let ReconstructJob::SourceChunk(source_job) = &mut jobs[job_index] else {
                        unreachable!("source chunk job index points to another job kind");
                    };
                    source_job.additional_targets.push(target);
                } else {
                    let job_index = jobs.len();
                    jobs.push(ReconstructJob::SourceChunk(ReconstructSourceChunkJob {
                        chunk_hash: chunk_ref.chunk_hash,
                        raw_size: chunk_ref.raw_size,
                        source_path: source_chunk.path.clone(),
                        source_offset: source_chunk.offset,
                        first_target: target,
                        additional_targets: Vec::new(),
                    }));
                    source_chunk_job_indices.insert(source_key, job_index);
                }
                reused_source_chunk_count += 1;
                file_offset = file_offset
                    .checked_add(u64::from(chunk_ref.raw_size))
                    .ok_or_else(|| {
                        Error::manifest(format!("file '{}' size overflows u64", file_manifest.path))
                    })?;
                continue;
            }
            let hash = chunk_ref.chunk_hash;
            let (job_index, position) = match artifact_chunk_positions.entry(hash) {
                Entry::Occupied(position) => *position.get(),
                Entry::Vacant(position_entry) => {
                    let entry = chunk_index
                        .entries
                        .get_hash(&chunk_ref.chunk_hash)
                        .ok_or_else(|| {
                            Error::manifest(format!(
                                "missing chunk index entry for required chunk '{}'",
                                chunk_ref.chunk_hash
                            ))
                        })?;
                    let artifact = ArtifactRef::from_chunk_entry(&chunk_ref.chunk_hash, entry);
                    let job_index = if let Some(&job_index) = artifact_job_indices.get(&artifact) {
                        job_index
                    } else {
                        let job_index = jobs.len();
                        jobs.push(ReconstructJob::Artifact(ReconstructArtifactJob {
                            artifact: artifact.clone(),
                            artifact_path: artifact.local_path(repo_paths)?,
                            encoded_size: entry.artifact_size,
                            chunks: Vec::new(),
                            targets: Vec::new(),
                        }));
                        // Copy/source jobs keep empty slots so artifact slots retain job indices.
                        chunk_targets.resize_with(jobs.len(), Vec::new);
                        artifact_job_indices.insert(artifact.clone(), job_index);
                        job_index
                    };
                    let ReconstructJob::Artifact(artifact_job) = &mut jobs[job_index] else {
                        unreachable!("artifact job index points to another job kind");
                    };
                    let position = artifact_job.chunks.len();
                    chunk_targets[job_index].push(Vec::new());
                    artifact_job.chunks.push(ReconstructArtifactChunk {
                        chunk_hash: hash,
                        raw_size: entry.raw_size,
                        target_range: 0..0,
                    });
                    *position_entry.insert((job_index, position))
                }
            };
            let ReconstructJob::Artifact(artifact_job) = &mut jobs[job_index] else {
                unreachable!("artifact job index points to another job kind");
            };
            let indexed_raw_size = artifact_job.chunks[position].raw_size;
            if indexed_raw_size != chunk_ref.raw_size {
                return Err(Error::manifest(format!(
                    "chunk '{}' size mismatch in file '{}': manifest {}, index {}",
                    hash, file_manifest.path, chunk_ref.raw_size, indexed_raw_size
                )));
            }
            chunk_targets[job_index][position].push(ReconstructTarget {
                file_index,
                file_offset,
            });
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
        missing_artifact_counts.push(0);
        files.push(ReconstructPipelineFile {
            path: file_manifest.path.clone(),
            size: file_manifest.size,
            full_file_hash: ChunkHash::parse_hex(&file_manifest.full_file_hash)?,
        });
    }

    // Only missing artifacts need dependency scratch; reuse it between jobs.
    let mut seen_dependent_files = Vec::new();
    // Resolve ranges before jobs and chunks are sorted: slots use insertion order.
    for (job_index, job) in jobs.iter_mut().enumerate() {
        if let ReconstructJob::Artifact(job) = job {
            for (position, chunk) in job.chunks.iter_mut().enumerate() {
                let start = job.targets.len();
                job.targets
                    .extend(mem::take(&mut chunk_targets[job_index][position]));
                chunk.target_range = start..job.targets.len();
            }
            // Artifact targets are the authoritative file dependencies for this job.
            if !existing_artifacts.contains(&job.artifact) {
                seen_dependent_files.resize(files.len(), false);
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
    }
    jobs.sort_by(|left, right| match (left, right) {
        (ReconstructJob::SourceChunk(left), ReconstructJob::SourceChunk(right)) => {
            (&left.source_path, left.source_offset).cmp(&(&right.source_path, right.source_offset))
        }
        (ReconstructJob::SourceChunk(_), _) => Ordering::Less,
        (_, ReconstructJob::SourceChunk(_)) => Ordering::Greater,
        _ => Ordering::Equal,
    });
    // The job map already owns one entry for each artifact the plan will use.
    let required_artifacts: HashSet<_> = artifact_job_indices.into_keys().collect();
    let required_bundle_count = required_artifacts
        .iter()
        .filter(|artifact| matches!(artifact, ArtifactRef::Bundle { .. }))
        .count();
    Ok(ReconstructPipelinePlan {
        files,
        jobs,
        missing_artifact_to_file_indices,
        missing_artifact_counts,
        required_bundle_count,
        required_artifact_count: required_artifacts.len(),
        required_artifacts,
        required_chunk_count: snapshot_manifest
            .files()
            .map(|file| file.chunks.len())
            .sum(),
        reused_source_chunk_count,
    })
}

pub(crate) struct InstallationChanges {
    pub(crate) metadata_updates: Vec<SnapshotEntry>,
    pub(crate) classification: RepairClassification,
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
    inspection: VerifyMode,
    runtime_hints: &RuntimeResourceHints,
    control: &OperationControl,
    operation_pool: Option<&ThreadPool>,
) -> Result<InstallationChanges> {
    let (mut classification, _) = classify_directory_with_marker_namespace_and_pool(
        VerifyRequest {
            snapshot_manifest,
            dir: destination_dir,
            mode: inspection,
            runtime_hints,
            control: control.clone(),
        },
        "installation.repair.verify",
        operation_pool,
    )?;
    let updates = if inspection == VerifyMode::MetadataOnly {
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
    classification: RepairClassification,
) -> Result<InstallationChanges> {
    let missing_file_count = classification.missing_files.len();
    let corrupt_file_count = classification.corrupt_files.len();
    let extra_file_count = classification.extra_files.len();
    let needs_rebuild = missing_file_count > 0 || corrupt_file_count > 0;
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
    classification: &mut RepairClassification,
    destination: &Path,
    control: &OperationControl,
) -> Result<Vec<SnapshotEntry>> {
    let mut updates = Vec::new();
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
        let path = resolve_manifest_relative_path(destination, entry.path())?;
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        let matches = match entry {
            SnapshotEntry::Directory(_) => metadata.file_type().is_dir(),
            SnapshotEntry::File(file)
                if metadata.file_type().is_file() && metadata.len() == file.size =>
            {
                let metadata_differs = metadata.permissions().readonly() != file.read_only;
                #[cfg(unix)]
                let metadata_differs = {
                    use std::os::unix::fs::PermissionsExt;
                    metadata_differs
                        || (metadata.permissions().mode() & 0o111 != 0) != file.executable
                };
                if !metadata_differs {
                    false
                } else {
                    match verify_full_file_integrity(
                        &path,
                        file.size,
                        ChunkHash::parse_hex(&file.full_file_hash)?,
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

fn subset_manifest_for_issues(
    snapshot_manifest: &SnapshotManifest,
    classification: &RepairClassification,
) -> Result<SnapshotManifest> {
    let affected_paths = classification
        .missing_files
        .iter()
        .chain(classification.corrupt_files.iter())
        .map(|issue| issue.path.as_str())
        .collect::<HashSet<_>>();
    let mut subset = snapshot_manifest.clone();
    let mut required_paths = affected_paths.clone();
    loop {
        let previous_len = required_paths.len();
        for entry in &snapshot_manifest.entries {
            if required_paths.contains(entry.path())
                && let SnapshotEntry::Symlink(link) = entry
            {
                required_paths.insert(link.target_path.as_str());
            }
        }
        if required_paths.len() == previous_len {
            break;
        }
    }
    subset.entries = snapshot_manifest
        .entries
        .iter()
        .filter(|entry| required_paths.contains(entry.path()))
        .cloned()
        .collect();
    subset.total_logical_size = subset.files().map(|file| file.size).sum();
    subset.total_compressed_size_estimate = subset.total_logical_size / 2;
    if !affected_paths.is_empty() && subset.entries.is_empty() {
        return Err(Error::verify(
            "repair classification referenced entries that are not present in the snapshot manifest",
        ));
    }
    Ok(subset)
}
