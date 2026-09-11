use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

use rayon::prelude::*;

use super::transfer::ArtifactPriority;
use super::transfer::PendingBundleDownload;
use super::types::SnapshotReconstructSource;
use crate::Error;
use crate::Result;
use crate::manifest::snapshot::SnapshotManifest;

use super::io::*;
use super::jobs::*;
pub(super) fn build_artifact_priorities(
    pipeline_plan: &ReconstructPipelinePlan,
    pending_bundle_downloads: &[PendingBundleDownload],
) -> HashMap<crate::artifact::store::ArtifactRef, ArtifactPriority> {
    pending_bundle_downloads
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

pub(super) fn build_reconstruct_pipeline_plan(
    snapshot_manifest: &SnapshotManifest,
    chunk_index: &crate::manifest::ChunkIndex,
    repo_paths: &crate::repo::paths::GutchRepoPaths,
    existing_artifacts: &HashSet<crate::artifact::store::ArtifactRef>,
    verified_source: Option<SnapshotReconstructSource<'_>>,
) -> Result<ReconstructPipelinePlan> {
    let mut files = Vec::with_capacity(snapshot_manifest.file_count());
    let mut jobs = Vec::new();
    let mut chunk_positions: HashMap<(usize, crate::artifact::hash::ChunkHash), usize> =
        HashMap::new();
    let mut chunk_targets: HashMap<(usize, usize), Vec<ReconstructTarget>> = HashMap::new();
    let mut artifact_job_indices: HashMap<crate::artifact::store::ArtifactRef, usize> =
        HashMap::new();
    let mut source_chunk_job_indices: HashMap<(PathBuf, u64, u32, String), usize> = HashMap::new();
    let mut missing_artifact_to_file_indices: HashMap<
        crate::artifact::store::ArtifactRef,
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
                trace_metadata: ReconstructFileTraceMetadata::default(),
                #[cfg(test)]
                reconstruction: ReconstructPipelineReconstruction::CopyFromSource,
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
        let mut chunks = Vec::with_capacity(file_manifest.chunks.len());
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
                chunks.push(ReconstructPipelineChunkInput::Source {
                    chunk_hash: chunk_ref.chunk_hash.clone(),
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
                crate::artifact::store::ArtifactRef::from_chunk_entry(&chunk_ref.chunk_hash, entry);
            if let crate::artifact::store::ArtifactRef::Bundle { .. } = &artifact {
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
                    targets: 0..0,
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
            chunks.push(ReconstructPipelineChunkInput::Artifact(
                ReconstructPipelineChunk {
                    chunk_hash: chunk_ref.chunk_hash.clone(),
                    artifact,
                },
            ));
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
        let reconstruction = ReconstructPipelineReconstruction::ReconstructFromParts { chunks };
        let trace_metadata = reconstruct_file_trace_metadata(&reconstruction);
        files.push(ReconstructPipelineFile {
            path: file_manifest.path.clone(),
            size: file_manifest.size,
            full_file_hash: crate::artifact::hash::ChunkHash::parse_hex(
                &file_manifest.full_file_hash,
            )?,
            trace_metadata,
            #[cfg(test)]
            reconstruction,
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
                chunk.targets = start..job.targets.len();
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
