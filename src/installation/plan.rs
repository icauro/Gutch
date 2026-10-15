use super::io::build_source_chunk_locations;
use super::jobs::{
    ReconstructArtifactChunk, ReconstructArtifactJob, ReconstructJob, ReconstructPipelineFile,
    ReconstructPipelinePlan, ReconstructSourceChunkJob, ReconstructTarget,
};
use super::types::SnapshotReconstructSource;
use crate::Error;
use crate::Result;
use crate::installation::inspect::RepairClassification;
use crate::installation::inspect::VerifyRequest;
use crate::installation::inspect::classify_directory_with_marker_namespace_and_pool;
use crate::manifest::SnapshotEntry;
use crate::manifest::snapshot::SnapshotManifest;
use crate::runtime::RuntimeResourceHints;
use crate::transfer::download::ArtifactPriority;
use crate::transfer::download::PendingArtifactDownload;
use rayon::ThreadPool;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

pub(crate) fn build_artifact_priorities(
    pipeline_plan: &ReconstructPipelinePlan,
    pending_artifact_downloads: &[PendingArtifactDownload],
) -> HashMap<crate::repo::artifacts::ArtifactRef, ArtifactPriority> {
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

pub(crate) fn build_reconstruct_pipeline_plan(
    snapshot_manifest: &SnapshotManifest,
    chunk_index: &crate::repo::index::ChunkIndex,
    repo_paths: &crate::repo::paths::GutchRepoPaths,
    existing_artifacts: &HashSet<crate::repo::artifacts::ArtifactRef>,
    verified_source: Option<SnapshotReconstructSource<'_>>,
) -> Result<ReconstructPipelinePlan> {
    let mut files = Vec::with_capacity(snapshot_manifest.file_count());
    let mut jobs = Vec::new();
    let mut chunk_positions: HashMap<(usize, crate::artifact::hash::ChunkHash), usize> =
        HashMap::new();
    let mut chunk_targets: HashMap<(usize, usize), Vec<ReconstructTarget>> = HashMap::new();
    let mut artifact_job_indices: HashMap<crate::repo::artifacts::ArtifactRef, usize> =
        HashMap::new();
    let mut source_chunk_job_indices: HashMap<(PathBuf, u64, u32, String), usize> = HashMap::new();
    let mut missing_artifact_to_file_indices: HashMap<
        crate::repo::artifacts::ArtifactRef,
        Vec<usize>,
    > = HashMap::new();
    let mut missing_artifact_counts = Vec::with_capacity(snapshot_manifest.file_count());
    let mut bundle_artifacts = HashSet::new();
    let mut required_artifacts = HashSet::new();
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
    let source_chunk_locations = verified_source
        .map(build_source_chunk_locations)
        .transpose()?
        .unwrap_or_default();

    for file_manifest in snapshot_manifest.files() {
        if let Some(source_file) = source_files_by_path.get(file_manifest.path.as_str())
            && source_file.full_file_hash == file_manifest.full_file_hash
            && source_file.size == file_manifest.size
        {
            let source_root = verified_source.expect("source files map only exists with source");
            let file_index = files.len();
            let source_path = crate::filesystem::paths::resolve_manifest_relative_path(
                source_root.destination_dir,
                &file_manifest.path,
            )?;
            files.push(ReconstructPipelineFile {
                path: file_manifest.path.clone(),
                size: file_manifest.size,
                full_file_hash: crate::artifact::hash::ChunkHash::parse_hex(
                    &file_manifest.full_file_hash,
                )?,
            });
            jobs.push(ReconstructJob::CopySourceFile {
                file_index,
                source_path,
            });
            missing_artifact_counts.push(0);
            reused_source_chunk_count += file_manifest.chunks.len();
            continue;
        }

        let file_index = files.len();
        let mut missing_artifact_dependencies = HashSet::new();
        let mut file_offset = 0u64;
        for chunk_ref in &file_manifest.chunks {
            if let Some(source_chunk) = source_chunk_locations
                .get(&chunk_ref.chunk_hash)
                .filter(|source_chunk| source_chunk.raw_size == u64::from(chunk_ref.raw_size))
            {
                let source_key = (
                    source_chunk.path.clone(),
                    source_chunk.offset,
                    chunk_ref.raw_size,
                    chunk_ref.chunk_hash.clone(),
                );
                let source_job_index =
                    if let Some(&job_index) = source_chunk_job_indices.get(&source_key) {
                        job_index
                    } else {
                        let job_index = jobs.len();
                        jobs.push(ReconstructJob::SourceChunk(ReconstructSourceChunkJob {
                            chunk_hash: chunk_ref.chunk_hash.clone(),
                            raw_size: chunk_ref.raw_size,
                            source_path: source_chunk.path.clone(),
                            source_offset: source_chunk.offset,
                            targets: Vec::new(),
                        }));
                        source_chunk_job_indices.insert(source_key, job_index);
                        job_index
                    };
                let ReconstructJob::SourceChunk(source_job) = &mut jobs[source_job_index] else {
                    unreachable!("source chunk job index points to another job kind");
                };
                source_job.targets.push(ReconstructTarget {
                    file_index,
                    file_offset,
                });
                reused_source_chunk_count += 1;
                file_offset = file_offset
                    .checked_add(u64::from(chunk_ref.raw_size))
                    .ok_or_else(|| {
                        Error::manifest(format!("file '{}' size overflows u64", file_manifest.path))
                    })?;
                continue;
            }
            let entry = chunk_index
                .entries
                .get(&chunk_ref.chunk_hash)
                .ok_or_else(|| {
                    Error::manifest(format!(
                        "missing chunk index entry for required chunk '{}'",
                        chunk_ref.chunk_hash
                    ))
                })?;
            if entry.raw_size != chunk_ref.raw_size {
                return Err(Error::manifest(format!(
                    "chunk '{}' size mismatch in file '{}': manifest {}, index {}",
                    chunk_ref.chunk_hash, file_manifest.path, chunk_ref.raw_size, entry.raw_size
                )));
            }
            let artifact =
                crate::repo::artifacts::ArtifactRef::from_chunk_entry(&chunk_ref.chunk_hash, entry);
            if let crate::repo::artifacts::ArtifactRef::Bundle { .. } = &artifact {
                bundle_artifacts.insert(artifact.clone());
            }
            let job_index = if let Some(&job_index) = artifact_job_indices.get(&artifact) {
                job_index
            } else {
                let job_index = jobs.len();
                jobs.push(ReconstructJob::Artifact(ReconstructArtifactJob {
                    artifact: artifact.clone(),
                    artifact_path: artifact.local_path(repo_paths)?,
                    chunks: Vec::new(),
                    targets: Vec::new(),
                }));
                artifact_job_indices.insert(artifact.clone(), job_index);
                job_index
            };
            let ReconstructJob::Artifact(artifact_job) = &mut jobs[job_index] else {
                unreachable!("artifact job index points to another job kind");
            };
            let hash = crate::artifact::hash::ChunkHash::parse_hex(&chunk_ref.chunk_hash)?;
            let position = *chunk_positions.entry((job_index, hash)).or_insert_with(|| {
                let position = artifact_job.chunks.len();
                artifact_job.chunks.push(ReconstructArtifactChunk {
                    chunk_hash: hash,
                    raw_size: chunk_ref.raw_size,
                    compressed_size: entry.compressed_size,
                    target_range: 0..0,
                });
                position
            });
            if artifact_job.chunks[position].raw_size != chunk_ref.raw_size {
                return Err(Error::manifest(
                    "inconsistent chunk sizes in reconstruction plan",
                ));
            }
            chunk_targets
                .entry((job_index, position))
                .or_default()
                .push(ReconstructTarget {
                    file_index,
                    file_offset,
                });
            required_artifacts.insert(artifact.clone());
            if !existing_artifacts.contains(&artifact) {
                missing_artifact_dependencies.insert(artifact.clone());
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
        for artifact in &missing_artifact_dependencies {
            missing_artifact_to_file_indices
                .entry(artifact.clone())
                .or_default()
                .push(file_index);
        }
        missing_artifact_counts.push(missing_artifact_dependencies.len());
        files.push(ReconstructPipelineFile {
            path: file_manifest.path.clone(),
            size: file_manifest.size,
            full_file_hash: crate::artifact::hash::ChunkHash::parse_hex(
                &file_manifest.full_file_hash,
            )?,
        });
    }

    for (job_index, job) in jobs.iter_mut().enumerate() {
        if let ReconstructJob::Artifact(job) = job {
            for (position, chunk) in job.chunks.iter_mut().enumerate() {
                let start = job.targets.len();
                job.targets.extend(
                    chunk_targets
                        .remove(&(job_index, position))
                        .unwrap_or_default(),
                );
                chunk.target_range = start..job.targets.len();
            }
            job.chunks.sort_by_key(|chunk| chunk.chunk_hash);
        }
    }
    jobs.sort_by(|left, right| match (left, right) {
        (ReconstructJob::SourceChunk(left), ReconstructJob::SourceChunk(right)) => {
            (&left.source_path, left.source_offset).cmp(&(&right.source_path, right.source_offset))
        }
        (ReconstructJob::SourceChunk(_), _) => std::cmp::Ordering::Less,
        (_, ReconstructJob::SourceChunk(_)) => std::cmp::Ordering::Greater,
        _ => std::cmp::Ordering::Equal,
    });
    Ok(ReconstructPipelinePlan {
        files,
        jobs,
        missing_artifact_to_file_indices,
        missing_artifact_counts,
        required_bundle_count: bundle_artifacts.len(),
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
    pub(crate) required_artifacts: HashSet<crate::repo::artifacts::ArtifactRef>,
}

pub(crate) fn inspect_installation_changes(
    snapshot_manifest: &SnapshotManifest,
    chunk_index: Option<&crate::repo::index::ChunkIndex>,
    destination_dir: &Path,
    inspection: super::inspect::VerifyMode,
    runtime_hints: &RuntimeResourceHints,
    control: &crate::progress::GutchOperationControl,
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
    let updates = if inspection == super::inspect::VerifyMode::MetadataOnly {
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
    chunk_index: Option<&crate::repo::index::ChunkIndex>,
    classification: RepairClassification,
) -> Result<InstallationChanges> {
    let missing_file_count = classification.missing_files.len();
    let corrupt_file_count = classification.corrupt_files.len();
    let extra_file_count = classification.extra_files.len();
    let needs_rebuild = missing_file_count > 0 || corrupt_file_count > 0;
    let affected_manifest = subset_manifest_for_issues(snapshot_manifest, &classification)?;
    let required_artifacts = if needs_rebuild {
        chunk_index
            .map(|index| crate::repo::artifacts::required_artifacts(&affected_manifest, index))
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
    control: &crate::progress::GutchOperationControl,
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
        let path =
            crate::filesystem::paths::resolve_manifest_relative_path(destination, entry.path())?;
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
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
                    match crate::file_integrity::verify_full_file_integrity(
                        &path,
                        file.size,
                        crate::artifact::hash::ChunkHash::parse_hex(&file.full_file_hash)?,
                        None,
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
