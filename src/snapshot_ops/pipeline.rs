use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::apply::build_snapshot_reconstruct_result;
use super::transfer::{
    ArtifactDownloadEvent, ArtifactDownloadScheduler, ArtifactPriority, FetchTransferProgress,
    PendingBundleDownload,
};
use super::types::{
    ReconstructTimings, SnapshotReconstructFromRepoResult, SnapshotReconstructSource,
};
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::{
    GutchOperation, GutchOperationControl, GutchProgressEvent, GutchProgressPhase, emit_progress,
};
use crate::runtime::{RuntimeResourceHints, plan_reconstruct_runtime, plan_verify_runtime};
use crate::{Error, Result};

pub(super) struct ReconstructPipelinePlan {
    files: Vec<ReconstructPipelineFile>,
    bundle_jobs: Vec<BundleReconstructJob>,
    bundle_only: bool,
    artifact_to_file_indices: HashMap<crate::ArtifactRef, Vec<usize>>,
    remaining_artifact_dependencies: Vec<usize>,
    pub required_bundle_count: usize,
    pub required_artifacts: HashSet<crate::ArtifactRef>,
    pub required_artifact_count: usize,
    required_chunk_count: usize,
    reused_source_chunk_count: usize,
}

pub(super) struct ReconstructPipelineApplyRequest<'a> {
    pub repo_root: &'a Path,
    pub compression_profile: crate::manifest::CompressionProfile,
    pub snapshot_manifest: &'a SnapshotManifest,
    pub destination_dir: &'a Path,
    pub staging_dir: Option<&'a Path>,
    pub source_destination_dir: Option<PathBuf>,
    pub runtime_hints: &'a RuntimeResourceHints,
    pub existing_artifacts: &'a HashSet<crate::ArtifactRef>,
    pub pending_bundle_downloads: &'a [PendingBundleDownload],
    pub transfer_policy: super::types::TransferConcurrencyPolicy,
    pub control: &'a GutchOperationControl,
    pub download_client: Option<&'a reqwest::blocking::Client>,
    pub transfer_progress: Option<&'a Arc<FetchTransferProgress>>,
}

#[derive(Clone)]
struct ReconstructPipelineFile {
    path: String,
    size: u64,
    full_file_hash: String,
    reconstruction: ReconstructPipelineReconstruction,
}

#[derive(Clone)]
enum ReconstructPipelineReconstruction {
    CopyFromSource {
        source_path: PathBuf,
    },
    ReconstructFromParts {
        chunks: Vec<ReconstructPipelineChunkInput>,
    },
}

#[derive(Clone)]
enum ReconstructPipelineChunkInput {
    Source {
        chunk_hash: String,
        raw_size: u64,
        source_path: PathBuf,
        source_offset: u64,
    },
    Artifact(ReconstructPipelineChunk),
}

#[derive(Clone)]
struct ReconstructPipelineChunk {
    chunk_hash: String,
    raw_size: u64,
    compressed_size: u64,
    artifact: crate::ArtifactRef,
    artifact_path: PathBuf,
}

#[derive(Clone)]
struct BundleReconstructJob {
    artifact: crate::ArtifactRef,
    artifact_path: PathBuf,
    chunks: HashMap<String, BundleReconstructChunk>,
}

#[derive(Clone)]
struct BundleReconstructChunk {
    raw_size: u64,
    targets: Vec<BundleReconstructTarget>,
}

#[derive(Clone)]
struct BundleReconstructTarget {
    file_index: usize,
    file_offset: u64,
}

struct SourceChunkLocation {
    path: PathBuf,
    offset: u64,
}

enum ArtifactPipelineEvent {
    Ready(crate::ArtifactRef),
    Failed(Error),
    WorkerDone,
}

enum ReconstructPipelineEvent {
    Artifact(ArtifactPipelineEvent),
    FileCompleted(ReconstructTimings),
    FileFailed(Error),
    FileWorkerDone,
}

pub(super) fn prepare_snapshot_entries(
    snapshot_manifest: &SnapshotManifest,
    output_dir: &Path,
) -> Result<()> {
    crate::manifest::validate_snapshot_entries(&snapshot_manifest.entries)?;
    crate::helpers::create_dir_all(output_dir)?;
    create_snapshot_directories(snapshot_manifest, output_dir)
}

fn create_snapshot_directories(
    snapshot_manifest: &SnapshotManifest,
    output_dir: &Path,
) -> Result<()> {
    let mut directories = snapshot_manifest
        .entries
        .iter()
        .filter_map(|entry| match entry {
            crate::manifest::SnapshotEntry::Directory(directory) => Some(directory),
            _ => None,
        })
        .collect::<Vec<_>>();
    directories.sort_by_key(|directory| directory.path.matches('/').count());
    for directory in directories {
        let path = crate::path_guard::resolve_manifest_relative_path(output_dir, &directory.path)?;
        crate::helpers::create_dir_all(&path)?;
    }
    Ok(())
}

fn relative_link_text(link_path: &str, target_path: &str) -> PathBuf {
    let parent = Path::new(link_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let parent_parts = parent.components().collect::<Vec<_>>();
    let target_parts = Path::new(target_path).components().collect::<Vec<_>>();
    let mut common = 0usize;
    while common < parent_parts.len()
        && common < target_parts.len()
        && parent_parts[common] == target_parts[common]
    {
        common += 1;
    }
    let mut relative = PathBuf::new();
    for _ in common..parent_parts.len() {
        relative.push("..");
    }
    for component in &target_parts[common..] {
        relative.push(component.as_os_str());
    }
    relative
}

pub(super) fn finalize_snapshot_entries(
    snapshot_manifest: &SnapshotManifest,
    output_dir: &Path,
) -> Result<()> {
    for entry in &snapshot_manifest.entries {
        let crate::manifest::SnapshotEntry::Symlink(link) = entry else {
            continue;
        };
        let link_path = crate::path_guard::resolve_manifest_relative_path(output_dir, &link.path)?;
        let link_text = relative_link_text(&link.path, &link.target_path);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&link_text, &link_path).map_err(|error| {
            Error::Reconstruct(format!(
                "failed to create symbolic link '{}': {error}",
                link.path
            ))
        })?;
        #[cfg(windows)]
        {
            let target_is_directory = snapshot_manifest.entries.iter().any(|target| {
                target.path() == link.target_path
                    && matches!(target, crate::manifest::SnapshotEntry::Directory(_))
            });
            let result = if target_is_directory {
                std::os::windows::fs::symlink_dir(&link_text, &link_path)
            } else {
                std::os::windows::fs::symlink_file(&link_text, &link_path)
            };
            result.map_err(|error| Error::Reconstruct(format!(
                "failed to create symbolic link '{}'; enable Windows Developer Mode or grant symbolic-link privilege: {error}",
                link.path
            )))?;
        }
    }

    for file in snapshot_manifest.files() {
        let path = crate::path_guard::resolve_manifest_relative_path(output_dir, &file.path)?;
        #[cfg(unix)]
        if file.executable {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&path)?.permissions();
            permissions.set_mode(permissions.mode() | 0o111);
            std::fs::set_permissions(&path, permissions)?;
        }
        let mut permissions = std::fs::metadata(&path)?.permissions();
        permissions.set_readonly(file.read_only);
        std::fs::set_permissions(&path, permissions)?;
    }

    let mut directories = snapshot_manifest
        .entries
        .iter()
        .filter_map(|entry| match entry {
            crate::manifest::SnapshotEntry::Directory(directory) => Some(directory),
            _ => None,
        })
        .collect::<Vec<_>>();
    directories.sort_by_key(|directory| std::cmp::Reverse(directory.path.matches('/').count()));
    for directory in directories {
        let path = crate::path_guard::resolve_manifest_relative_path(output_dir, &directory.path)?;
        let mut permissions = std::fs::metadata(&path)?.permissions();
        permissions.set_readonly(directory.read_only);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

fn has_files_still_waiting_on_artifacts(
    scheduled_files: &[bool],
    remaining_artifact_dependencies: &[usize],
) -> bool {
    scheduled_files
        .iter()
        .zip(remaining_artifact_dependencies.iter())
        .any(|(scheduled, remaining)| !scheduled && *remaining > 0)
}

fn build_artifact_priorities(
    pipeline_plan: &ReconstructPipelinePlan,
    pending_bundle_downloads: &[PendingBundleDownload],
) -> HashMap<crate::ArtifactRef, ArtifactPriority> {
    pending_bundle_downloads
        .iter()
        .map(|download| {
            let mut priority = ArtifactPriority::default();
            if let Some(file_indices) = pipeline_plan
                .artifact_to_file_indices
                .get(&download.artifact)
            {
                for &file_index in file_indices {
                    let remaining = pipeline_plan.remaining_artifact_dependencies[file_index];
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

fn map_artifact_download_event(event: ArtifactDownloadEvent) -> ReconstructPipelineEvent {
    ReconstructPipelineEvent::Artifact(match event {
        ArtifactDownloadEvent::Ready(artifact) => ArtifactPipelineEvent::Ready(artifact),
        ArtifactDownloadEvent::Failed(error) => ArtifactPipelineEvent::Failed(error),
        ArtifactDownloadEvent::WorkerDone => ArtifactPipelineEvent::WorkerDone,
    })
}

pub(super) fn build_reconstruct_pipeline_plan(
    snapshot_manifest: &SnapshotManifest,
    chunk_index: &crate::manifest::ChunkIndex,
    repo_paths: &crate::GutchRepoPaths,
    existing_artifacts: &HashSet<crate::ArtifactRef>,
    verified_source: Option<SnapshotReconstructSource<'_>>,
) -> Result<ReconstructPipelinePlan> {
    let mut files = Vec::with_capacity(snapshot_manifest.file_count());
    let mut bundle_jobs = Vec::new();
    let mut bundle_job_indices: HashMap<crate::ArtifactRef, usize> = HashMap::new();
    let mut artifact_to_file_indices: HashMap<crate::ArtifactRef, Vec<usize>> = HashMap::new();
    let mut remaining_artifact_dependencies = Vec::with_capacity(snapshot_manifest.file_count());
    let mut bundled_artifacts = HashSet::new();
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
            files.push(ReconstructPipelineFile {
                path: file_manifest.path.clone(),
                size: file_manifest.size,
                full_file_hash: file_manifest.full_file_hash.clone(),
                reconstruction: ReconstructPipelineReconstruction::CopyFromSource {
                    source_path: crate::path_guard::resolve_manifest_relative_path(
                        source_root.destination_dir,
                        &file_manifest.path,
                    )?,
                },
            });
            remaining_artifact_dependencies.push(0);
            reused_source_chunk_count += file_manifest.chunks.len();
            continue;
        }

        let file_index = files.len();
        let mut chunks = Vec::with_capacity(file_manifest.chunks.len());
        let mut missing_artifact_dependencies = HashSet::new();
        let mut file_offset = 0u64;
        for chunk_ref in &file_manifest.chunks {
            if let Some(source_chunk) = source_chunk_locations.get(&chunk_ref.chunk_hash) {
                chunks.push(ReconstructPipelineChunkInput::Source {
                    chunk_hash: chunk_ref.chunk_hash.clone(),
                    raw_size: chunk_ref.raw_size,
                    source_path: source_chunk.path.clone(),
                    source_offset: source_chunk.offset,
                });
                reused_source_chunk_count += 1;
                file_offset = file_offset.checked_add(chunk_ref.raw_size).ok_or_else(|| {
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
            let artifact = crate::ArtifactRef::from_chunk_entry(&chunk_ref.chunk_hash, entry);
            if let crate::ArtifactRef::Bundle { .. } = &artifact {
                bundled_artifacts.insert(artifact.clone());
                let job_index = match bundle_job_indices.entry(artifact.clone()) {
                    std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let job_index = bundle_jobs.len();
                        bundle_jobs.push(BundleReconstructJob {
                            artifact: artifact.clone(),
                            artifact_path: artifact.local_path(repo_paths)?,
                            chunks: HashMap::new(),
                        });
                        entry.insert(job_index);
                        job_index
                    }
                };
                let bundle_job = &mut bundle_jobs[job_index];
                let bundle_chunk = bundle_job
                    .chunks
                    .entry(chunk_ref.chunk_hash.clone())
                    .or_insert_with(|| BundleReconstructChunk {
                        raw_size: chunk_ref.raw_size,
                        targets: Vec::new(),
                    });
                if bundle_chunk.raw_size != chunk_ref.raw_size {
                    return Err(Error::manifest(format!(
                        "chunk '{}' has inconsistent sizes in bundle '{}'",
                        chunk_ref.chunk_hash,
                        artifact.id()
                    )));
                }
                bundle_chunk.targets.push(BundleReconstructTarget {
                    file_index,
                    file_offset,
                });
            }
            required_artifacts.insert(artifact.clone());
            if !existing_artifacts.contains(&artifact) {
                missing_artifact_dependencies.insert(artifact.clone());
            }
            chunks.push(ReconstructPipelineChunkInput::Artifact(
                ReconstructPipelineChunk {
                    chunk_hash: chunk_ref.chunk_hash.clone(),
                    raw_size: chunk_ref.raw_size,
                    compressed_size: entry.compressed_size,
                    artifact_path: artifact.local_path(repo_paths)?,
                    artifact,
                },
            ));
            file_offset = file_offset.checked_add(chunk_ref.raw_size).ok_or_else(|| {
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
            artifact_to_file_indices
                .entry(artifact.clone())
                .or_default()
                .push(file_index);
        }
        remaining_artifact_dependencies.push(missing_artifact_dependencies.len());
        files.push(ReconstructPipelineFile {
            path: file_manifest.path.clone(),
            size: file_manifest.size,
            full_file_hash: file_manifest.full_file_hash.clone(),
            reconstruction: ReconstructPipelineReconstruction::ReconstructFromParts { chunks },
        });
    }

    let bundle_only = files.iter().all(|file| {
        matches!(
            &file.reconstruction,
            ReconstructPipelineReconstruction::ReconstructFromParts { chunks }
                if chunks.iter().all(|chunk| matches!(
                    chunk,
                    ReconstructPipelineChunkInput::Artifact(ReconstructPipelineChunk {
                        artifact: crate::ArtifactRef::Bundle { .. },
                        ..
                    })
                ))
        )
    });

    Ok(ReconstructPipelinePlan {
        files,
        bundle_jobs,
        bundle_only,
        artifact_to_file_indices,
        remaining_artifact_dependencies,
        required_bundle_count: bundled_artifacts.len(),
        required_artifact_count: required_artifacts.len(),
        required_artifacts,
        required_chunk_count: snapshot_manifest
            .files()
            .map(|file| file.chunks.len())
            .sum(),
        reused_source_chunk_count,
    })
}

pub(super) fn apply_reconstruct_pipeline(
    request: ReconstructPipelineApplyRequest<'_>,
    pipeline_plan: ReconstructPipelinePlan,
) -> Result<SnapshotReconstructFromRepoResult> {
    request.control.checkpoint()?;
    let _operation_span = request
        .control
        .profile_span("gutch.snapshot.reconstruct.apply");
    let staging_dir = {
        let _staging_span = request
            .control
            .profile_span("gutch.snapshot.reconstruct.prepare.staging");
        let staging_dir = request
            .staging_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(|| {
                super::apply::default_snapshot_staging_dir(
                    request.destination_dir,
                    &request.snapshot_manifest.snapshot_id,
                )
            });
        if request.destination_dir == staging_dir {
            return Err(Error::PathError {
                path: request.destination_dir.to_path_buf(),
                reason: "destination and staging directories must be different".to_string(),
            });
        }
        if staging_dir.exists() {
            crate::helpers::remove_dir_all(&staging_dir)?;
        }
        if let Some(parent) = staging_dir.parent() {
            crate::helpers::create_dir_all(parent)?;
        }
        staging_dir
    };

    let main_result = (|| -> Result<SnapshotReconstructFromRepoResult> {
        {
            let _prepare_span = request
                .control
                .profile_span("gutch.snapshot.reconstruct.prepare.entries");
            prepare_snapshot_entries(request.snapshot_manifest, &staging_dir)?;
        }
        let required_chunk_count = pipeline_plan.required_chunk_count;
        let required_bundle_count = pipeline_plan.required_bundle_count;
        let required_artifact_count = pipeline_plan.required_artifact_count;
        let reused_source_chunk_count = pipeline_plan.reused_source_chunk_count;
        let (reconstruct_timings, verify_timings) =
            execute_reconstruct_pipeline_reconstruction(ExecuteReconstructPipelineRequest {
                pipeline_plan,
                staging_dir: &staging_dir,
                snapshot_id: &request.snapshot_manifest.snapshot_id,
                compression_profile: &request.compression_profile,
                runtime_hints: request.runtime_hints,
                existing_artifacts: request.existing_artifacts,
                pending_bundle_downloads: request.pending_bundle_downloads,
                transfer_policy: request.transfer_policy,
                control: request.control,
                download_client: request.download_client,
                transfer_progress: request.transfer_progress,
            })?;
        {
            let _finalize_span = request
                .control
                .profile_span("gutch.snapshot.reconstruct.finalize.entries");
            finalize_snapshot_entries(request.snapshot_manifest, &staging_dir)?;
        }
        // Promotion is a short consistency-critical section. Observe cancellation immediately
        // before it, then finish the swap without interruption.
        {
            let _promote_span = request
                .control
                .profile_span("gutch.snapshot.reconstruct.finalize.promote");
            request.control.checkpoint()?;
            crate::helpers::replace_directory(&staging_dir, request.destination_dir)?;
        }
        Ok(SnapshotReconstructFromRepoResult {
            repo_root: request.repo_root.to_path_buf(),
            snapshot_manifest: request.snapshot_manifest.clone(),
            reconstruct_result: build_snapshot_reconstruct_result(
                super::apply::BuildSnapshotReconstructResultRequest {
                    snapshot_manifest: request.snapshot_manifest,
                    destination_dir: request.destination_dir,
                    staging_dir: &staging_dir,
                    reused_source_chunk_count,
                    required_chunk_count,
                    required_bundle_count,
                    required_artifact_count,
                    reconstruct_timings,
                    verify_timings,
                },
            ),
            resolved_bundle_file_count: required_bundle_count,
            resolved_artifact_file_count: required_artifact_count,
            source_destination_dir: request.source_destination_dir.clone(),
        })
    })();

    if main_result.is_err() && staging_dir.exists() {
        let _ = crate::helpers::remove_dir_all(&staging_dir);
    }
    main_result
}

pub(super) struct ExecuteReconstructPipelineRequest<'a> {
    pub(super) pipeline_plan: ReconstructPipelinePlan,
    pub(super) staging_dir: &'a Path,
    pub(super) snapshot_id: &'a str,
    pub(super) compression_profile: &'a crate::manifest::CompressionProfile,
    pub(super) runtime_hints: &'a RuntimeResourceHints,
    pub(super) existing_artifacts: &'a HashSet<crate::ArtifactRef>,
    pub(super) pending_bundle_downloads: &'a [PendingBundleDownload],
    pub(super) transfer_policy: super::types::TransferConcurrencyPolicy,
    pub(super) control: &'a GutchOperationControl,
    pub(super) download_client: Option<&'a reqwest::blocking::Client>,
    pub(super) transfer_progress: Option<&'a Arc<FetchTransferProgress>>,
}

enum BundledReconstructEvent {
    BundleCompleted(ReconstructTimings),
    Failed(Error),
    WorkerDone,
}

fn execute_bundled_reconstruct_pipeline(
    request: ExecuteReconstructPipelineRequest<'_>,
) -> Result<(ReconstructTimings, crate::verify::VerifyTimings)> {
    let ExecuteReconstructPipelineRequest {
        pipeline_plan,
        staging_dir,
        snapshot_id,
        compression_profile,
        runtime_hints,
        pending_bundle_downloads,
        control,
        ..
    } = request;
    if !pending_bundle_downloads.is_empty() {
        return Err(Error::reconstruct(
            "bundled reconstruction cannot run before pending downloads complete",
        ));
    }
    if pipeline_plan
        .remaining_artifact_dependencies
        .iter()
        .any(|remaining| *remaining > 0)
    {
        return Err(Error::reconstruct(
            "bundled reconstruction requires all bundles to be available locally",
        ));
    }

    control.checkpoint()?;
    let _pipeline_span = control.profile_span("gutch.snapshot.reconstruct.pipeline");
    let reconstruct_started_at = Instant::now();
    let runtime_plan = plan_reconstruct_runtime(runtime_hints);
    let mut reconstruct_timings = ReconstructTimings {
        runtime_plan: Some(runtime_plan.clone()),
        verify_files: true,
        ..ReconstructTimings::default()
    };
    let mut verify_timings = crate::verify::VerifyTimings {
        verify_mode: crate::verify::VerifyMode::Fast,
        runtime_plan: Some(plan_verify_runtime(runtime_hints)),
        ..crate::verify::VerifyTimings::default()
    };

    let prepare_started_at = Instant::now();
    let output_paths = Arc::new(prepare_bundled_output_files(
        &pipeline_plan.files,
        staging_dir,
    )?);
    reconstruct_timings.prepare_wall = prepare_started_at.elapsed();
    let file_plans = Arc::new(pipeline_plan.files);
    let bundle_jobs = Arc::new(pipeline_plan.bundle_jobs);
    for job in bundle_jobs.iter() {
        if !job.artifact_path.is_file() {
            return Err(Error::reconstruct(format!(
                "required bundle '{}' is not available at {}",
                job.artifact.id(),
                job.artifact_path.display()
            )));
        }
    }

    emit_progress(
        control.progress(),
        GutchProgressEvent {
            operation: GutchOperation::Fetch,
            phase: GutchProgressPhase::Reconstruct,
            message: "Reconstructing bundled artifacts (0/0)".to_string(),
            current: 0,
            total: bundle_jobs.len().max(1),
            transferred_bytes: 0,
            total_bytes: 0,
            detail: Some(snapshot_id.to_string()),
            done: false,
        },
    );

    let worker_count = runtime_plan
        .worker_count
        .min(bundle_jobs.len())
        .max(1.min(bundle_jobs.len()));
    let (task_tx, task_rx) = mpsc::channel::<usize>();
    let task_rx = Arc::new(Mutex::new(task_rx));
    let (event_tx, event_rx) = mpsc::channel::<BundledReconstructEvent>();
    let mut worker_handles = Vec::with_capacity(worker_count);
    for _ in 0..worker_count {
        let task_rx = Arc::clone(&task_rx);
        let event_tx = event_tx.clone();
        let bundle_jobs = Arc::clone(&bundle_jobs);
        let file_plans = Arc::clone(&file_plans);
        let output_paths = Arc::clone(&output_paths);
        let compression_profile = compression_profile.clone();
        let control = control.clone();
        worker_handles.push(thread::spawn(move || {
            loop {
                let next_job = match task_rx.lock() {
                    Ok(receiver) => receiver.recv(),
                    Err(_) => {
                        let _ = event_tx.send(BundledReconstructEvent::Failed(Error::reconstruct(
                            "bundled reconstruction task state is unavailable",
                        )));
                        return;
                    }
                };
                let job_index = match next_job {
                    Ok(job_index) => job_index,
                    Err(_) => {
                        let _ = event_tx.send(BundledReconstructEvent::WorkerDone);
                        return;
                    }
                };
                let result = reconstruct_bundle_job(
                    &bundle_jobs[job_index],
                    &file_plans,
                    &output_paths,
                    &compression_profile,
                    &control,
                );
                match result {
                    Ok(timings) => {
                        if event_tx
                            .send(BundledReconstructEvent::BundleCompleted(timings))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = event_tx.send(BundledReconstructEvent::Failed(error));
                        return;
                    }
                }
            }
        }));
    }
    drop(event_tx);
    for job_index in 0..bundle_jobs.len() {
        task_tx
            .send(job_index)
            .map_err(|_| Error::reconstruct("bundled reconstruction queue closed unexpectedly"))?;
    }
    drop(task_tx);

    let mut completed_bundle_count = 0usize;
    let mut completed_worker_count = 0usize;
    let mut pipeline_error = None;
    while completed_worker_count < worker_count {
        match event_rx.recv() {
            Ok(BundledReconstructEvent::BundleCompleted(bundle_timings)) => {
                completed_bundle_count += 1;
                accumulate_pipeline_timings(
                    &mut reconstruct_timings,
                    &mut verify_timings,
                    &bundle_timings,
                );
                emit_progress(
                    control.progress(),
                    GutchProgressEvent {
                        operation: GutchOperation::Fetch,
                        phase: GutchProgressPhase::Reconstruct,
                        message: format!(
                            "Reconstructing bundled artifacts ({}/{})",
                            completed_bundle_count,
                            bundle_jobs.len().max(1)
                        ),
                        current: completed_bundle_count,
                        total: bundle_jobs.len().max(1),
                        transferred_bytes: 0,
                        total_bytes: 0,
                        detail: None,
                        done: false,
                    },
                );
            }
            Ok(BundledReconstructEvent::Failed(error)) => {
                pipeline_error = Some(error);
                break;
            }
            Ok(BundledReconstructEvent::WorkerDone) => completed_worker_count += 1,
            Err(_) => {
                pipeline_error = Some(Error::reconstruct(
                    "bundled reconstruction event channel closed unexpectedly",
                ));
                break;
            }
        }
    }
    for handle in worker_handles {
        handle
            .join()
            .map_err(|_| Error::reconstruct("bundled reconstruction worker panicked"))?;
    }
    if let Some(error) = pipeline_error {
        return Err(error);
    }
    if completed_bundle_count != bundle_jobs.len() {
        return Err(Error::reconstruct(format!(
            "bundled reconstruction completed {completed_bundle_count} of {} bundles",
            bundle_jobs.len()
        )));
    }

    let verify_started_at = Instant::now();
    let verified_file_bytes = verify_bundled_output_files(
        &file_plans,
        &output_paths,
        control,
        &mut reconstruct_timings,
    )?;
    let verify_elapsed = verify_started_at.elapsed();
    verify_timings.file_hash += verify_elapsed;
    verify_timings.file_hash_bytes = verify_timings
        .file_hash_bytes
        .saturating_add(verified_file_bytes);
    emit_progress(
        control.progress(),
        GutchProgressEvent {
            operation: GutchOperation::Fetch,
            phase: GutchProgressPhase::Reconstruct,
            message: format!(
                "Reconstructing files ({}/{})",
                file_plans.len(),
                file_plans.len().max(1)
            ),
            current: file_plans.len(),
            total: file_plans.len().max(1),
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: false,
        },
    );

    reconstruct_timings.reconstruct_wall = reconstruct_started_at.elapsed();
    reconstruct_timings.total = reconstruct_timings.reconstruct_wall;
    verify_timings.verify_files_wall = reconstruct_timings.reconstruct_wall;
    verify_timings.total = reconstruct_timings.reconstruct_wall;
    Ok((reconstruct_timings, verify_timings))
}

fn prepare_bundled_output_files(
    files: &[ReconstructPipelineFile],
    staging_dir: &Path,
) -> Result<Vec<PathBuf>> {
    let mut output_paths = Vec::with_capacity(files.len());
    for file_plan in files {
        let output_path =
            crate::path_guard::resolve_manifest_relative_path(staging_dir, &file_plan.path)?;
        if let Some(parent) = output_path.parent() {
            crate::helpers::create_dir_all(parent)?;
        }
        let output = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&output_path)?;
        output.set_len(file_plan.size)?;
        output_paths.push(output_path);
    }
    Ok(output_paths)
}

fn reconstruct_bundle_job(
    job: &BundleReconstructJob,
    file_plans: &[ReconstructPipelineFile],
    output_paths: &[PathBuf],
    compression_profile: &crate::manifest::CompressionProfile,
    control: &GutchOperationControl,
) -> Result<ReconstructTimings> {
    control.checkpoint()?;
    let _bundle_span = control.profile_span_with_args(
        "gutch.snapshot.reconstruct.bundle",
        bundle_reconstruct_trace_args(job),
    );
    let profiler = control.profiler();
    let compressed_bytes = std::fs::metadata(&job.artifact_path)?.len();
    let mut reader = crate::bundle::BundleReader::open_with_profiler(
        File::open(&job.artifact_path)?,
        compression_profile,
        profiler,
    )?;
    let mut output_handles = BundleOutputHandleCache::new(output_paths);
    let mut remaining_chunks = job.chunks.keys().cloned().collect::<HashSet<_>>();
    let mut timings = ReconstructTimings::default();
    let stream_started_at = Instant::now();
    let stream_metrics = reader.stream_chunks(profiler, |_, entry, bytes| {
        let Some(chunk) = job.chunks.get(&entry.chunk_hash) else {
            return Ok(());
        };
        if chunk.raw_size != entry.raw_size || bytes.len() as u64 != chunk.raw_size {
            return Err(Error::reconstruct(format!(
                "bundle chunk '{}' size mismatch: expected {}, got {}",
                entry.chunk_hash,
                chunk.raw_size,
                bytes.len()
            )));
        }
        remaining_chunks.remove(&entry.chunk_hash);
        let rebuild_started_at = Instant::now();
        for target in &chunk.targets {
            control.checkpoint()?;
            let _chunk_span = control.profile_span_with_args(
                "gutch.snapshot.reconstruct.file.artifact_chunk",
                bundle_target_trace_args(
                    job,
                    &entry.chunk_hash,
                    target,
                    &file_plans[target.file_index],
                ),
            );
            let write_started_at = Instant::now();
            output_handles.write_at(target.file_index, target.file_offset, bytes)?;
            let write_elapsed = write_started_at.elapsed();
            timings.file_write += write_elapsed;
            timings.file_write_bytes = timings.file_write_bytes.saturating_add(bytes.len() as u64);
            timings.file_rebuild_bytes = timings
                .file_rebuild_bytes
                .saturating_add(bytes.len() as u64);
        }
        timings.file_rebuild += rebuild_started_at.elapsed();
        Ok(())
    })?;
    let stream_elapsed = stream_started_at.elapsed();

    if !remaining_chunks.is_empty() {
        return Err(Error::reconstruct(format!(
            "bundle '{}' is missing {} required chunks",
            job.artifact.id(),
            remaining_chunks.len()
        )));
    }

    timings.artifact_read = stream_elapsed;
    timings.artifact_read_compressed_bytes = compressed_bytes;
    timings.artifact_read_decompressed_bytes = stream_metrics.raw_bytes;
    timings.bundle_read = stream_elapsed;
    timings.bundle_read_compressed_bytes = compressed_bytes;
    timings.bundle_read_decompressed_bytes = stream_metrics.raw_bytes;
    timings.chunk_verify = stream_metrics.hash_verify;
    timings.chunk_verify_bytes = stream_metrics.raw_bytes;
    Ok(timings)
}

struct BundleOutputHandleCache<'a> {
    output_paths: &'a [PathBuf],
    handles: HashMap<usize, File>,
    lru: VecDeque<usize>,
    max_open: usize,
}

impl<'a> BundleOutputHandleCache<'a> {
    fn new(output_paths: &'a [PathBuf]) -> Self {
        Self {
            output_paths,
            handles: HashMap::new(),
            lru: VecDeque::new(),
            max_open: 128,
        }
    }

    fn write_at(&mut self, file_index: usize, offset: u64, bytes: &[u8]) -> Result<()> {
        if file_index >= self.output_paths.len() {
            return Err(Error::reconstruct(format!(
                "bundle target references unknown file index {}",
                file_index
            )));
        }
        if !self.handles.contains_key(&file_index) {
            while self.handles.len() >= self.max_open {
                if let Some(oldest) = self.lru.pop_front() {
                    self.handles.remove(&oldest);
                }
            }
            let file = OpenOptions::new()
                .write(true)
                .open(&self.output_paths[file_index])?;
            self.handles.insert(file_index, file);
        }
        self.lru.retain(|index| *index != file_index);
        self.lru.push_back(file_index);
        let file = self
            .handles
            .get_mut(&file_index)
            .ok_or_else(|| Error::reconstruct("output file handle disappeared"))?;
        write_file_at(file, offset, bytes)
    }
}

#[cfg(windows)]
fn write_file_at(file: &mut File, offset: u64, bytes: &[u8]) -> Result<()> {
    use std::os::windows::fs::FileExt;

    let mut written = 0usize;
    while written < bytes.len() {
        let count = file.seek_write(&bytes[written..], offset + written as u64)?;
        if count == 0 {
            return Err(Error::reconstruct("output file write returned zero bytes"));
        }
        written += count;
    }
    Ok(())
}

#[cfg(unix)]
fn write_file_at(file: &mut File, offset: u64, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::FileExt;

    file.write_all_at(bytes, offset)?;
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn write_file_at(file: &mut File, offset: u64, bytes: &[u8]) -> Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(bytes)?;
    Ok(())
}

fn bundle_reconstruct_trace_args(
    job: &BundleReconstructJob,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let logical_bytes = job
        .chunks
        .values()
        .map(|chunk| chunk.raw_size.saturating_mul(chunk.targets.len() as u64))
        .sum::<u64>();
    std::collections::BTreeMap::from([
        ("artifact_kind".to_string(), serde_json::json!("bundle")),
        (
            "artifact_id".to_string(),
            serde_json::json!(job.artifact.id()),
        ),
        (
            "artifact_path".to_string(),
            serde_json::json!(job.artifact_path.display().to_string()),
        ),
        (
            "required_chunk_count".to_string(),
            serde_json::json!(job.chunks.len()),
        ),
        (
            "logical_write_mib".to_string(),
            serde_json::json!(logical_bytes as f64 / (1024.0 * 1024.0)),
        ),
    ])
}

fn bundle_target_trace_args(
    job: &BundleReconstructJob,
    chunk_hash: &str,
    target: &BundleReconstructTarget,
    file_plan: &ReconstructPipelineFile,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut args = crate::profile::file_trace_args(file_plan.path.clone(), file_plan.size);
    args.insert("artifact_kind".to_string(), serde_json::json!("bundle"));
    args.insert(
        "artifact_id".to_string(),
        serde_json::json!(job.artifact.id()),
    );
    args.insert(
        "artifact_path".to_string(),
        serde_json::json!(job.artifact_path.display().to_string()),
    );
    args.insert("chunk_hash".to_string(), serde_json::json!(chunk_hash));
    args.insert(
        "raw_size_mib".to_string(),
        serde_json::json!(
            job.chunks
                .get(chunk_hash)
                .map(|chunk| chunk.raw_size as f64 / (1024.0 * 1024.0))
                .unwrap_or_default()
        ),
    );
    args.insert(
        "file_offset".to_string(),
        serde_json::json!(target.file_offset),
    );
    args
}

fn verify_bundled_output_files(
    files: &[ReconstructPipelineFile],
    output_paths: &[PathBuf],
    control: &GutchOperationControl,
    timings: &mut ReconstructTimings,
) -> Result<u64> {
    let mut verified_bytes = 0u64;
    for (file_index, file_plan) in files.iter().enumerate() {
        control.checkpoint()?;
        let _file_span = control.profile_span_with_args(
            "gutch.snapshot.reconstruct.file",
            reconstruct_file_trace_args(file_plan),
        );
        let _hash_span = control.profile_span("gutch.snapshot.reconstruct.file.full_hash");
        let mut input = File::open(&output_paths[file_index])?;
        let actual_size = input.metadata()?.len();
        if actual_size != file_plan.size {
            return Err(Error::reconstruct(format!(
                "reconstructed size {} does not match expected {} for {}",
                actual_size, file_plan.size, file_plan.path
            )));
        }
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0u8; 1024 * 1024];
        let mut file_bytes = 0u64;
        loop {
            let started_at = Instant::now();
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            timings.file_verify += started_at.elapsed();
            timings.file_verify_bytes = timings.file_verify_bytes.saturating_add(read as u64);
            file_bytes = file_bytes.saturating_add(read as u64);
        }
        let actual_hash = crate::hash::hash_hex(&hasher.finalize());
        if actual_hash != file_plan.full_file_hash {
            return Err(Error::HashMismatch {
                expected: file_plan.full_file_hash.clone(),
                actual: actual_hash,
            });
        }
        verified_bytes = verified_bytes.saturating_add(file_bytes);
    }
    Ok(verified_bytes)
}

pub(super) fn execute_reconstruct_pipeline_reconstruction(
    request: ExecuteReconstructPipelineRequest<'_>,
) -> Result<(ReconstructTimings, crate::verify::VerifyTimings)> {
    if request.pending_bundle_downloads.is_empty() && request.pipeline_plan.bundle_only {
        return execute_bundled_reconstruct_pipeline(request);
    }

    let ExecuteReconstructPipelineRequest {
        mut pipeline_plan,
        staging_dir,
        snapshot_id,
        compression_profile,
        runtime_hints,
        existing_artifacts,
        pending_bundle_downloads,
        transfer_policy,
        control,
        download_client,
        transfer_progress,
    } = request;
    control.checkpoint()?;
    let _pipeline_span = control.profile_span("gutch.snapshot.reconstruct.pipeline");
    let reconstruct_started_at = Instant::now();
    let mut reconstruct_timings = ReconstructTimings {
        runtime_plan: Some(plan_reconstruct_runtime(runtime_hints)),
        verify_files: true,
        ..ReconstructTimings::default()
    };
    let mut verify_timings = crate::verify::VerifyTimings {
        verify_mode: crate::verify::VerifyMode::Fast,
        runtime_plan: Some(plan_verify_runtime(runtime_hints)),
        ..crate::verify::VerifyTimings::default()
    };
    let mut ready_files = VecDeque::new();
    let mut scheduled_files = vec![false; pipeline_plan.files.len()];
    let mut completed_file_count = 0usize;
    let mut available_artifacts = existing_artifacts.clone();

    for (file_index, remaining) in pipeline_plan
        .remaining_artifact_dependencies
        .iter()
        .enumerate()
    {
        if *remaining == 0 {
            scheduled_files[file_index] = true;
            ready_files.push_back(file_index);
        }
    }

    emit_progress(
        control.progress(),
        GutchProgressEvent {
            operation: GutchOperation::Fetch,
            phase: GutchProgressPhase::Reconstruct,
            message: "Reconstructing files (0/0)".to_string(),
            current: 0,
            total: pipeline_plan.files.len().max(1),
            transferred_bytes: 0,
            total_bytes: 0,
            detail: Some(snapshot_id.to_string()),
            done: false,
        },
    );

    let reconstruct_worker_count = reconstruct_timings
        .runtime_plan
        .as_ref()
        .map(|plan| plan.worker_count.max(1))
        .unwrap_or(1);
    if pending_bundle_downloads.is_empty()
        && pipeline_plan
            .remaining_artifact_dependencies
            .iter()
            .any(|remaining| *remaining > 0)
    {
        return Err(Error::reconstruct(
            "reconstruct pipeline requires bundles that are not available locally",
        ));
    }

    let artifact_worker_priorities =
        build_artifact_priorities(&pipeline_plan, pending_bundle_downloads);
    let artifact_scheduler = if pending_bundle_downloads.is_empty() {
        None
    } else {
        let Some(download_client) = download_client else {
            return Err(Error::reconstruct(
                "reconstruct pipeline requires a download client for pending bundles",
            ));
        };
        let Some(transfer_progress) = transfer_progress else {
            return Err(Error::reconstruct(
                "reconstruct pipeline requires transfer progress for pending bundles",
            ));
        };
        Some(ArtifactDownloadScheduler::new(
            pending_bundle_downloads,
            artifact_worker_priorities,
            transfer_policy,
            download_client,
            control,
            Arc::clone(transfer_progress),
        )?)
    };
    let artifact_worker_count = artifact_scheduler
        .as_ref()
        .map(ArtifactDownloadScheduler::worker_count)
        .unwrap_or(0);
    let (pipeline_event_tx, pipeline_event_rx) = mpsc::channel::<ReconstructPipelineEvent>();
    let (file_task_tx, file_task_rx) = mpsc::channel::<usize>();
    let file_task_rx = Arc::new(Mutex::new(file_task_rx));
    let file_plans = Arc::new(pipeline_plan.files.clone());
    let mut file_worker_handles = Vec::new();
    let compression_profile = compression_profile.clone();
    for _ in 0..reconstruct_worker_count {
        let compression_profile = compression_profile.clone();
        let event_tx = pipeline_event_tx.clone();
        let file_task_rx = Arc::clone(&file_task_rx);
        let staging_dir = staging_dir.to_path_buf();
        let file_plans = Arc::clone(&file_plans);
        let control = control.clone();
        file_worker_handles.push(thread::spawn(move || {
            loop {
                let next_file_index = match file_task_rx.lock() {
                    Ok(receiver) => receiver.recv(),
                    Err(_) => {
                        let _ = event_tx.send(ReconstructPipelineEvent::FileFailed(
                            Error::reconstruct("file task state is unavailable"),
                        ));
                        return;
                    }
                };
                let file_index = match next_file_index {
                    Ok(file_index) => file_index,
                    Err(_) => {
                        let _ = event_tx.send(ReconstructPipelineEvent::FileWorkerDone);
                        return;
                    }
                };
                if let Err(error) = control.checkpoint() {
                    let _ = event_tx.send(ReconstructPipelineEvent::FileFailed(error));
                    return;
                }
                match reconstruct_pipeline_file(
                    &file_plans[file_index],
                    &staging_dir,
                    &compression_profile,
                    &control,
                ) {
                    Ok(file_timings) => {
                        let _ =
                            event_tx.send(ReconstructPipelineEvent::FileCompleted(file_timings));
                    }
                    Err(error) => {
                        let _ = event_tx.send(ReconstructPipelineEvent::FileFailed(error));
                        return;
                    }
                }
            }
        }));
    }
    drop(pipeline_event_tx);

    let mut completed_artifact_workers = 0usize;
    let mut completed_file_workers = 0usize;
    let mut dispatched_file_count = 0usize;
    let mut pipeline_error: Option<Error> = None;
    let _dispatch_span = control.profile_span("gutch.snapshot.reconstruct.dispatch");
    let queue_ready_file = |file_index: usize| -> Result<()> {
        file_task_tx
            .send(file_index)
            .map_err(|_| Error::reconstruct("reconstruct pipeline file queue closed unexpectedly"))
    };
    while completed_file_count < pipeline_plan.files.len() {
        if let Err(error) = control.checkpoint() {
            pipeline_error = Some(error);
            break;
        }
        while let Some(file_index) = ready_files.pop_front() {
            if let Err(error) = queue_ready_file(file_index) {
                pipeline_error = Some(error);
                break;
            }
            dispatched_file_count += 1;
        }
        if let Some(scheduler) = artifact_scheduler.as_ref() {
            let reconstruct_backlog =
                ready_files.len() + dispatched_file_count.saturating_sub(completed_file_count);
            scheduler.set_downstream_backpressure(
                reconstruct_backlog >= reconstruct_worker_count.saturating_mul(2),
            );
        }

        if pipeline_error.is_some() || completed_file_count == pipeline_plan.files.len() {
            break;
        }

        let next_event = loop {
            if completed_artifact_workers < artifact_worker_count
                && let Some(scheduler) = artifact_scheduler.as_ref()
            {
                match scheduler.try_recv() {
                    Ok(event) => break Ok(map_artifact_download_event(event)),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        break Err(Error::reconstruct(
                            "artifact download scheduler channel closed unexpectedly",
                        ));
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
            match pipeline_event_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => break Ok(event),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break Err(Error::reconstruct(
                        "reconstruct pipeline event channel closed unexpectedly",
                    ));
                }
            }
        };
        match next_event {
            Ok(ReconstructPipelineEvent::Artifact(ArtifactPipelineEvent::Ready(artifact))) => {
                if available_artifacts.insert(artifact.clone())
                    && let Some(file_indices) =
                        pipeline_plan.artifact_to_file_indices.get(&artifact)
                {
                    for &file_index in file_indices {
                        if pipeline_plan.remaining_artifact_dependencies[file_index] == 0 {
                            continue;
                        }
                        pipeline_plan.remaining_artifact_dependencies[file_index] -= 1;
                        if pipeline_plan.remaining_artifact_dependencies[file_index] == 0
                            && !scheduled_files[file_index]
                        {
                            scheduled_files[file_index] = true;
                            ready_files.push_back(file_index);
                        }
                    }
                }
                if let Some(scheduler) = artifact_scheduler.as_ref() {
                    scheduler.update_priorities(build_artifact_priorities(
                        &pipeline_plan,
                        pending_bundle_downloads,
                    ));
                }
            }
            Ok(ReconstructPipelineEvent::Artifact(ArtifactPipelineEvent::Failed(error))) => {
                pipeline_error = Some(error);
                break;
            }
            Ok(ReconstructPipelineEvent::Artifact(ArtifactPipelineEvent::WorkerDone)) => {
                completed_artifact_workers += 1;
                if completed_artifact_workers == artifact_worker_count
                    && let Some(transfer_progress) = transfer_progress
                {
                    transfer_progress.finish();
                }
                if completed_artifact_workers == artifact_worker_count
                    && completed_file_count < pipeline_plan.files.len()
                    && has_files_still_waiting_on_artifacts(
                        &scheduled_files,
                        &pipeline_plan.remaining_artifact_dependencies,
                    )
                {
                    pipeline_error = Some(Error::reconstruct(
                        "reconstruct pipeline stopped before all files became ready",
                    ));
                    break;
                }
            }
            Ok(ReconstructPipelineEvent::FileCompleted(file_timings)) => {
                completed_file_count += 1;
                accumulate_pipeline_timings(
                    &mut reconstruct_timings,
                    &mut verify_timings,
                    &file_timings,
                );
                emit_progress(
                    control.progress(),
                    GutchProgressEvent {
                        operation: GutchOperation::Fetch,
                        phase: GutchProgressPhase::Reconstruct,
                        message: format!(
                            "Reconstructing files ({}/{})",
                            completed_file_count,
                            pipeline_plan.files.len().max(1)
                        ),
                        current: completed_file_count,
                        total: pipeline_plan.files.len().max(1),
                        transferred_bytes: 0,
                        total_bytes: 0,
                        detail: None,
                        done: false,
                    },
                );
            }
            Ok(ReconstructPipelineEvent::FileFailed(error)) => {
                pipeline_error = Some(error);
                break;
            }
            Ok(ReconstructPipelineEvent::FileWorkerDone) => {
                completed_file_workers += 1;
                if completed_file_workers == reconstruct_worker_count
                    && completed_file_count < pipeline_plan.files.len()
                {
                    pipeline_error = Some(Error::reconstruct(
                        "reconstruct pipeline file workers stopped before reconstruction completed",
                    ));
                    break;
                }
            }
            Err(error) => {
                pipeline_error = Some(error);
                break;
            }
        }
    }

    drop(file_task_tx);
    if pipeline_error.is_some() {
        if let Some(scheduler) = artifact_scheduler.as_ref() {
            scheduler.cancel();
        }
    }
    if let Some(scheduler) = artifact_scheduler {
        scheduler.join()?;
    }
    for handle in file_worker_handles {
        handle
            .join()
            .map_err(|_| Error::reconstruct("file reconstruct worker panicked"))?;
    }

    if let Some(error) = pipeline_error {
        return Err(error);
    }

    reconstruct_timings.reconstruct_wall = reconstruct_started_at.elapsed();
    reconstruct_timings.total = reconstruct_timings.reconstruct_wall;
    verify_timings.verify_files_wall = reconstruct_timings.reconstruct_wall;
    verify_timings.total = reconstruct_timings.reconstruct_wall;
    Ok((reconstruct_timings, verify_timings))
}

fn reconstruct_pipeline_file(
    file_plan: &ReconstructPipelineFile,
    staging_dir: &Path,
    compression_profile: &crate::manifest::CompressionProfile,
    control: &GutchOperationControl,
) -> Result<ReconstructTimings> {
    control.checkpoint()?;
    let _file_span = control.profile_span_with_args(
        "gutch.snapshot.reconstruct.file",
        reconstruct_file_trace_args(file_plan),
    );
    match &file_plan.reconstruction {
        ReconstructPipelineReconstruction::CopyFromSource { source_path } => {
            let _strategy_span =
                control.profile_span("gutch.snapshot.reconstruct.file.copy_source");
            copy_pipeline_file_from_source(file_plan, source_path, staging_dir, control)
        }
        ReconstructPipelineReconstruction::ReconstructFromParts { chunks } => {
            reconstruct_pipeline_file_from_parts(
                file_plan,
                chunks,
                staging_dir,
                compression_profile,
                control,
            )
        }
    }
}

fn reconstruct_file_trace_args(
    file_plan: &ReconstructPipelineFile,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut args = crate::profile::file_trace_args(&file_plan.path, file_plan.size);
    let mut chunk_hashes = HashSet::new();
    let mut bundle_artifacts = HashSet::new();

    if let ReconstructPipelineReconstruction::ReconstructFromParts { chunks } =
        &file_plan.reconstruction
    {
        for chunk in chunks {
            match chunk {
                ReconstructPipelineChunkInput::Source { chunk_hash, .. } => {
                    chunk_hashes.insert(chunk_hash);
                }
                ReconstructPipelineChunkInput::Artifact(chunk) => {
                    chunk_hashes.insert(&chunk.chunk_hash);
                    if matches!(&chunk.artifact, crate::ArtifactRef::Bundle { .. }) {
                        bundle_artifacts.insert(&chunk.artifact);
                    }
                }
            }
        }
    }

    args.insert(
        "required_distinct_chunks".to_string(),
        serde_json::json!(chunk_hashes.len()),
    );
    args.insert(
        "required_distinct_bundles".to_string(),
        serde_json::json!(bundle_artifacts.len()),
    );
    args
}

fn artifact_chunk_trace_args(
    chunk: &ReconstructPipelineChunk,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let artifact_kind = match &chunk.artifact {
        crate::ArtifactRef::Bundle { .. } => "bundle",
        crate::ArtifactRef::LooseChunk { .. } => "loose_chunk",
    };
    std::collections::BTreeMap::from([
        (
            "artifact_kind".to_string(),
            serde_json::json!(artifact_kind),
        ),
        (
            "artifact_id".to_string(),
            serde_json::json!(chunk.artifact.id()),
        ),
        (
            "artifact_path".to_string(),
            serde_json::json!(chunk.artifact_path.display().to_string()),
        ),
        (
            "chunk_hash".to_string(),
            serde_json::json!(chunk.chunk_hash),
        ),
        (
            "raw_size_mib".to_string(),
            serde_json::json!(chunk.raw_size as f64 / (1024.0 * 1024.0)),
        ),
        (
            "compressed_size_mib".to_string(),
            serde_json::json!(chunk.compressed_size as f64 / (1024.0 * 1024.0)),
        ),
    ])
}

fn copy_pipeline_file_from_source(
    file_plan: &ReconstructPipelineFile,
    source_path: &Path,
    staging_dir: &Path,
    control: &GutchOperationControl,
) -> Result<ReconstructTimings> {
    let mut timings = ReconstructTimings::default();
    let output_path =
        crate::path_guard::resolve_manifest_relative_path(staging_dir, &file_plan.path)?;
    if let Some(parent) = output_path.parent() {
        crate::helpers::create_dir_all(parent)?;
    }
    let mut input = File::open(source_path)?;
    let mut output = std::io::BufWriter::new(File::create(&output_path)?);
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut written = 0u64;

    loop {
        control.checkpoint()?;
        let read = {
            let _read_span = control.profile_span("gutch.snapshot.reconstruct.file.source_read");
            let rebuild_started_at = Instant::now();
            let read = std::io::Read::read(&mut input, &mut buffer)?;
            timings.file_rebuild += rebuild_started_at.elapsed();
            read
        };
        if read == 0 {
            break;
        }
        {
            let _write_span = control.profile_span("gutch.snapshot.reconstruct.file.output_write");
            write_verified_file_bytes(&mut output, &mut hasher, &buffer[..read], &mut timings)?;
        }
        written += read as u64;
    }

    finish_verified_file(
        output, hasher, written, file_plan, timings, control, "copied",
    )
}

fn reconstruct_pipeline_file_from_parts(
    file_plan: &ReconstructPipelineFile,
    chunks: &[ReconstructPipelineChunkInput],
    staging_dir: &Path,
    compression_profile: &crate::manifest::CompressionProfile,
    control: &GutchOperationControl,
) -> Result<ReconstructTimings> {
    let mut timings = ReconstructTimings::default();
    let output_path =
        crate::path_guard::resolve_manifest_relative_path(staging_dir, &file_plan.path)?;
    if let Some(parent) = output_path.parent() {
        crate::helpers::create_dir_all(parent)?;
    }
    let mut output = std::io::BufWriter::new(File::create(&output_path)?);
    let mut file_hasher = blake3::Hasher::new();
    let mut written = 0u64;
    let mut bundle_readers: HashMap<
        crate::ArtifactRef,
        (crate::bundle::BundleReader<File>, HashMap<String, usize>),
    > = HashMap::new();
    let mut source_readers: HashMap<PathBuf, std::io::BufReader<File>> = HashMap::new();

    for chunk in chunks {
        control.checkpoint()?;
        let chunk_bytes = match chunk {
            ReconstructPipelineChunkInput::Source {
                chunk_hash,
                raw_size,
                source_path,
                source_offset,
            } => {
                let _source_chunk_span =
                    control.profile_span("gutch.snapshot.reconstruct.file.source_chunk");
                let reader = match source_readers.entry(source_path.clone()) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(std::io::BufReader::new(File::open(source_path)?))
                    }
                };
                let read_started_at = Instant::now();
                reader.seek(SeekFrom::Start(*source_offset))?;
                let mut chunk_bytes = vec![0u8; *raw_size as usize];
                reader.read_exact(&mut chunk_bytes)?;
                timings.file_rebuild += read_started_at.elapsed();
                let verify_started_at = Instant::now();
                let actual_hash = crate::hash::hash_hex(&crate::hash::hash_chunk(&chunk_bytes));
                timings.chunk_verify += verify_started_at.elapsed();
                timings.chunk_verify_bytes += chunk_bytes.len() as u64;
                if actual_hash != *chunk_hash {
                    return Err(Error::HashMismatch {
                        expected: chunk_hash.clone(),
                        actual: actual_hash,
                    });
                }
                chunk_bytes
            }
            ReconstructPipelineChunkInput::Artifact(chunk) => {
                let _artifact_chunk_span = control.profile_span_with_args(
                    "gutch.snapshot.reconstruct.file.artifact_chunk",
                    artifact_chunk_trace_args(chunk),
                );
                let profiler = control.profiler();
                let read_started_at = Instant::now();
                let chunk_bytes = match &chunk.artifact {
                    crate::ArtifactRef::Bundle { .. } => {
                        let reader_entry = match bundle_readers.entry(chunk.artifact.clone()) {
                            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                            std::collections::hash_map::Entry::Vacant(entry) => {
                                let file = File::open(&chunk.artifact_path)?;
                                let reader = crate::bundle::BundleReader::open_with_profiler(
                                    file,
                                    compression_profile,
                                    profiler,
                                )?;
                                let chunk_index = reader.chunk_index();
                                entry.insert((reader, chunk_index))
                            }
                        };
                        let chunk_index = reader_entry
                            .1
                            .get(&chunk.chunk_hash)
                            .copied()
                            .ok_or_else(|| Error::MissingChunk(chunk.chunk_hash.clone()))?;
                        let chunk_bytes = reader_entry
                            .0
                            .read_chunk_bytes_with_profiler(chunk_index, profiler)?;
                        let _materialize_span = control.profile_span(
                            "gutch.snapshot.reconstruct.file.artifact_chunk.materialize",
                        );
                        chunk_bytes.to_vec()
                    }
                    crate::ArtifactRef::LooseChunk { hash } => {
                        crate::artifact::store::read_loose_chunk_with_profiler(
                            &chunk.artifact_path,
                            hash,
                            chunk.raw_size,
                            compression_profile,
                            profiler,
                        )?
                    }
                };
                let read_elapsed = read_started_at.elapsed();
                timings.artifact_read += read_elapsed;
                timings.artifact_read_compressed_bytes += chunk.compressed_size;
                timings.artifact_read_decompressed_bytes += chunk_bytes.len() as u64;
                if matches!(chunk.artifact, crate::ArtifactRef::Bundle { .. }) {
                    timings.bundle_read += read_elapsed;
                    timings.bundle_read_compressed_bytes += chunk.compressed_size;
                    timings.bundle_read_decompressed_bytes += chunk_bytes.len() as u64;
                }
                if chunk_bytes.len() as u64 != chunk.raw_size {
                    return Err(Error::reconstruct(format!(
                        "chunk size mismatch for '{}' in {}: expected {}, got {}",
                        chunk.chunk_hash,
                        file_plan.path,
                        chunk.raw_size,
                        chunk_bytes.len()
                    )));
                }
                chunk_bytes
            }
        };
        {
            let _write_span = control.profile_span("gutch.snapshot.reconstruct.file.output_write");
            write_verified_file_bytes(&mut output, &mut file_hasher, &chunk_bytes, &mut timings)?;
        }
        written += chunk_bytes.len() as u64;
    }

    finish_verified_file(
        output,
        file_hasher,
        written,
        file_plan,
        timings,
        control,
        "reconstructed",
    )
}

fn write_verified_file_bytes(
    output: &mut std::io::BufWriter<File>,
    hasher: &mut blake3::Hasher,
    bytes: &[u8],
    timings: &mut ReconstructTimings,
) -> Result<()> {
    let write_started_at = Instant::now();
    output.write_all(bytes)?;
    timings.file_write += write_started_at.elapsed();
    timings.file_write_bytes += bytes.len() as u64;
    timings.file_rebuild_bytes += bytes.len() as u64;
    let verify_started_at = Instant::now();
    hasher.update(bytes);
    timings.file_verify += verify_started_at.elapsed();
    timings.file_verify_bytes += bytes.len() as u64;
    Ok(())
}

fn finish_verified_file(
    mut output: std::io::BufWriter<File>,
    hasher: blake3::Hasher,
    written: u64,
    file_plan: &ReconstructPipelineFile,
    mut timings: ReconstructTimings,
    control: &GutchOperationControl,
    operation: &str,
) -> Result<ReconstructTimings> {
    if written != file_plan.size {
        return Err(Error::reconstruct(format!(
            "{operation} size {} does not match expected {} for {}",
            written, file_plan.size, file_plan.path
        )));
    }

    {
        let _flush_span = control.profile_span("gutch.snapshot.reconstruct.file.output_flush");
        let flush_started_at = Instant::now();
        output.flush()?;
        timings.file_write += flush_started_at.elapsed();
    }
    let actual_hash = {
        let _hash_span = control.profile_span("gutch.snapshot.reconstruct.file.full_hash");
        let verify_started_at = Instant::now();
        let actual_hash = crate::hash::hash_hex(&hasher.finalize());
        timings.file_verify += verify_started_at.elapsed();
        actual_hash
    };
    if actual_hash != file_plan.full_file_hash {
        return Err(Error::HashMismatch {
            expected: file_plan.full_file_hash.clone(),
            actual: actual_hash,
        });
    }

    Ok(timings)
}

fn build_source_chunk_locations(
    source: SnapshotReconstructSource<'_>,
) -> Result<HashMap<String, SourceChunkLocation>> {
    let mut chunk_locations = HashMap::new();
    for file_manifest in source.snapshot_manifest.files() {
        let typed_file = crate::manifest::runtime::typed_file_manifest(file_manifest)?;
        let source_path = crate::path_guard::resolve_manifest_relative_path(
            source.destination_dir,
            &file_manifest.path,
        )?;
        let mut offset = 0u64;
        for chunk_ref in typed_file.chunks {
            chunk_locations
                .entry(chunk_ref.hash_hex.to_string())
                .or_insert_with(|| SourceChunkLocation {
                    path: source_path.clone(),
                    offset,
                });
            offset += chunk_ref.raw_size;
        }
    }
    Ok(chunk_locations)
}

fn accumulate_pipeline_timings(
    reconstruct_timings: &mut ReconstructTimings,
    verify_timings: &mut crate::verify::VerifyTimings,
    file_timings: &ReconstructTimings,
) {
    reconstruct_timings.artifact_read += file_timings.artifact_read;
    reconstruct_timings.artifact_read_compressed_bytes +=
        file_timings.artifact_read_compressed_bytes;
    reconstruct_timings.artifact_read_decompressed_bytes +=
        file_timings.artifact_read_decompressed_bytes;
    reconstruct_timings.bundle_read += file_timings.bundle_read;
    reconstruct_timings.bundle_read_compressed_bytes += file_timings.bundle_read_compressed_bytes;
    reconstruct_timings.bundle_read_decompressed_bytes +=
        file_timings.bundle_read_decompressed_bytes;
    reconstruct_timings.chunk_verify += file_timings.chunk_verify;
    reconstruct_timings.chunk_verify_bytes += file_timings.chunk_verify_bytes;
    reconstruct_timings.file_rebuild += file_timings.file_rebuild;
    reconstruct_timings.file_rebuild_bytes += file_timings.file_rebuild_bytes;
    reconstruct_timings.file_verify += file_timings.file_verify;
    reconstruct_timings.file_verify_bytes += file_timings.file_verify_bytes;
    reconstruct_timings.file_write += file_timings.file_write;
    reconstruct_timings.file_write_bytes += file_timings.file_write_bytes;

    verify_timings.file_hash += file_timings.file_verify;
    verify_timings.file_hash_bytes += file_timings.file_verify_bytes;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::hash_hex;
    use crate::manifest::{self, ChunkRef, FileManifest, SnapshotTagSet};

    fn test_hash(bytes: &[u8]) -> String {
        hash_hex(&blake3::hash(bytes))
    }

    fn snapshot_manifest_with_file(snapshot_id: &str, full_file_hash: &str) -> SnapshotManifest {
        snapshot_manifest_with_chunk(snapshot_id, full_file_hash, test_hash(b"game"))
    }

    fn snapshot_manifest_with_chunk(
        snapshot_id: &str,
        full_file_hash: &str,
        chunk_hash: String,
    ) -> SnapshotManifest {
        let mut manifest =
            SnapshotManifest::new(snapshot_id.to_string(), SnapshotTagSet::default());
        manifest.push_file(FileManifest {
            path: "game.exe".to_string(),
            size: 4,
            executable: true,
            read_only: false,
            full_file_hash: full_file_hash.to_string(),
            chunks: vec![ChunkRef {
                chunk_hash,
                raw_size: 4,
            }],
        });
        manifest
    }

    fn bundled_chunk_index() -> crate::manifest::ChunkIndex {
        crate::manifest::ChunkIndex {
            format_version: "1".to_string(),
            updated_at: String::new(),
            entries: std::collections::BTreeMap::from([(
                test_hash(b"game"),
                manifest::ChunkIndexEntry {
                    bundle_id: "bundle-a".to_string(),
                    raw_size: 4,
                    compressed_size: 4,
                    storage_mode: manifest::ArtifactStorageMode::Bundled,
                },
            )]),
        }
    }

    fn snapshot_manifest_with_two_chunks(
        snapshot_id: &str,
        full_file_hash: &str,
        first_chunk_hash: String,
        second_chunk_hash: String,
    ) -> SnapshotManifest {
        let mut manifest =
            SnapshotManifest::new(snapshot_id.to_string(), SnapshotTagSet::default());
        manifest.push_file(FileManifest {
            path: "game.exe".to_string(),
            size: 8,
            executable: true,
            read_only: false,
            full_file_hash: full_file_hash.to_string(),
            chunks: vec![
                ChunkRef {
                    chunk_hash: first_chunk_hash,
                    raw_size: 4,
                },
                ChunkRef {
                    chunk_hash: second_chunk_hash,
                    raw_size: 4,
                },
            ],
        });
        manifest
    }

    #[test]
    fn pipeline_plan_copies_identical_source_file_without_bundle_dependency() {
        let source_root = crate::test_util::temp_path("gutch_pipeline_copy_source");
        let _ = std::fs::remove_dir_all(&source_root);
        std::fs::create_dir_all(&source_root).unwrap();
        std::fs::write(source_root.join("game.exe"), b"game").unwrap();
        let source_manifest = snapshot_manifest_with_file("source", &test_hash(b"game"));
        let snapshot_manifest = snapshot_manifest_with_file("snapshot", &test_hash(b"game"));
        let repo_root = crate::test_util::temp_path("gutch_pipeline_copy_repo");
        let _ = std::fs::remove_dir_all(&repo_root);
        let repo_paths = crate::GutchRepoPaths::from_repo_root(&repo_root);

        let plan = build_reconstruct_pipeline_plan(
            &snapshot_manifest,
            &bundled_chunk_index(),
            &repo_paths,
            &HashSet::new(),
            Some(SnapshotReconstructSource {
                snapshot_manifest: &source_manifest,
                destination_dir: &source_root,
            }),
        )
        .unwrap();

        assert_eq!(plan.required_bundle_count, 0);
        assert_eq!(plan.reused_source_chunk_count, 1);
        assert!(matches!(
            plan.files[0].reconstruction,
            ReconstructPipelineReconstruction::CopyFromSource { .. }
        ));

        let _ = std::fs::remove_dir_all(&source_root);
        let _ = std::fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn pipeline_plan_reconstructs_changed_source_file_from_bundles() {
        let source_root = crate::test_util::temp_path("gutch_pipeline_rebuild_source");
        let _ = std::fs::remove_dir_all(&source_root);
        std::fs::create_dir_all(&source_root).unwrap();
        std::fs::write(source_root.join("game.exe"), b"old!").unwrap();
        let source_manifest =
            snapshot_manifest_with_chunk("source", &test_hash(b"old!"), test_hash(b"old!"));
        let snapshot_manifest =
            snapshot_manifest_with_chunk("snapshot", &test_hash(b"game"), test_hash(b"game"));
        let repo_root = crate::test_util::temp_path("gutch_pipeline_rebuild_repo");
        let _ = std::fs::remove_dir_all(&repo_root);
        let repo_paths = crate::GutchRepoPaths::from_repo_root(&repo_root);

        let plan = build_reconstruct_pipeline_plan(
            &snapshot_manifest,
            &bundled_chunk_index(),
            &repo_paths,
            &HashSet::new(),
            Some(SnapshotReconstructSource {
                snapshot_manifest: &source_manifest,
                destination_dir: &source_root,
            }),
        )
        .unwrap();

        assert_eq!(plan.required_bundle_count, 1);
        assert_eq!(plan.reused_source_chunk_count, 0);
        assert!(matches!(
            plan.files[0].reconstruction,
            ReconstructPipelineReconstruction::ReconstructFromParts { .. }
        ));

        let _ = std::fs::remove_dir_all(&source_root);
        let _ = std::fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn pipeline_plan_mixes_source_and_bundle_chunks_for_changed_file() {
        let source_root = crate::test_util::temp_path("gutch_pipeline_mixed_source");
        let _ = std::fs::remove_dir_all(&source_root);
        std::fs::create_dir_all(&source_root).unwrap();
        std::fs::write(source_root.join("game.exe"), b"gameold!").unwrap();

        let shared_chunk_hash = test_hash(b"game");
        let source_only_chunk_hash = test_hash(b"old!");
        let snapshot_only_chunk_hash = test_hash(b"new!");
        let source_manifest = snapshot_manifest_with_two_chunks(
            "source",
            &test_hash(b"gameold!"),
            shared_chunk_hash.clone(),
            source_only_chunk_hash,
        );
        let snapshot_manifest = snapshot_manifest_with_two_chunks(
            "snapshot",
            &test_hash(b"gamenew!"),
            shared_chunk_hash.clone(),
            snapshot_only_chunk_hash.clone(),
        );
        let repo_root = crate::test_util::temp_path("gutch_pipeline_mixed_repo");
        let _ = std::fs::remove_dir_all(&repo_root);
        let repo_paths = crate::GutchRepoPaths::from_repo_root(&repo_root);
        let chunk_index = crate::manifest::ChunkIndex {
            format_version: "1".to_string(),
            updated_at: String::new(),
            entries: std::collections::BTreeMap::from([(
                snapshot_only_chunk_hash,
                manifest::ChunkIndexEntry {
                    bundle_id: "bundle-b".to_string(),
                    raw_size: 4,
                    compressed_size: 4,
                    storage_mode: manifest::ArtifactStorageMode::Bundled,
                },
            )]),
        };

        let plan = build_reconstruct_pipeline_plan(
            &snapshot_manifest,
            &chunk_index,
            &repo_paths,
            &HashSet::new(),
            Some(SnapshotReconstructSource {
                snapshot_manifest: &source_manifest,
                destination_dir: &source_root,
            }),
        )
        .unwrap();

        assert_eq!(plan.required_bundle_count, 1);
        assert_eq!(plan.reused_source_chunk_count, 1);
        assert!(matches!(
            &plan.files[0].reconstruction,
            ReconstructPipelineReconstruction::ReconstructFromParts { chunks }
                if matches!(chunks[0], ReconstructPipelineChunkInput::Source { .. })
                    && matches!(chunks[1], ReconstructPipelineChunkInput::Artifact(_))
        ));

        let _ = std::fs::remove_dir_all(&source_root);
        let _ = std::fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn pipeline_reconstructs_from_loose_chunk_artifact() {
        let repo_root = crate::test_util::temp_path("gutch_pipeline_loose_repo");
        let destination = crate::test_util::temp_path("gutch_pipeline_loose_destination");
        let _ = std::fs::remove_dir_all(&repo_root);
        let _ = std::fs::remove_dir_all(&destination);
        let repo_paths = crate::GutchRepoPaths::from_repo_root(&repo_root);
        crate::ensure_gutch_repo_layout(&repo_paths, "loose-test").unwrap();

        let chunk_hash = test_hash(b"game");
        let artifact = crate::ArtifactRef::LooseChunk {
            hash: chunk_hash.clone(),
        };
        let chunk_path = artifact.local_path(&repo_paths).unwrap();
        std::fs::create_dir_all(chunk_path.parent().unwrap()).unwrap();
        std::fs::write(
            &chunk_path,
            crate::compress::compress_with_level(
                b"game",
                crate::manifest::CompressionProfile::for_preset(
                    crate::manifest::CompressionPreset::Zstd3,
                )
                .zstd_level,
            )
            .unwrap(),
        )
        .unwrap();
        let chunk_index = crate::manifest::ChunkIndex {
            format_version: "1".to_string(),
            updated_at: String::new(),
            entries: std::collections::BTreeMap::from([(
                chunk_hash.clone(),
                manifest::ChunkIndexEntry {
                    bundle_id: chunk_hash.clone(),
                    raw_size: 4,
                    compressed_size: std::fs::metadata(&chunk_path).unwrap().len(),
                    storage_mode: manifest::ArtifactStorageMode::LooseChunks,
                },
            )]),
        };
        let snapshot_manifest =
            snapshot_manifest_with_chunk("loose-snapshot", &test_hash(b"game"), chunk_hash);
        let existing_artifacts = HashSet::from([artifact]);
        let plan = build_reconstruct_pipeline_plan(
            &snapshot_manifest,
            &chunk_index,
            &repo_paths,
            &existing_artifacts,
            None,
        )
        .unwrap();

        assert_eq!(plan.required_bundle_count, 0);
        assert_eq!(plan.required_artifact_count, 1);
        let result = apply_reconstruct_pipeline(
            ReconstructPipelineApplyRequest {
                repo_root: &repo_root,
                compression_profile: crate::manifest::CompressionProfile::for_preset(
                    crate::manifest::CompressionPreset::Zstd3,
                ),
                snapshot_manifest: &snapshot_manifest,
                destination_dir: &destination,
                staging_dir: None,
                source_destination_dir: None,
                runtime_hints: &RuntimeResourceHints::default(),
                existing_artifacts: &existing_artifacts,
                pending_bundle_downloads: &[],
                transfer_policy: crate::snapshot_ops::types::TransferConcurrencyPolicy::default(),
                control: &crate::GutchOperationControl::default(),
                download_client: None,
                transfer_progress: None,
            },
            plan,
        )
        .unwrap();

        assert_eq!(result.reconstruct_result.required_artifact_count, 1);
        assert_eq!(
            std::fs::read(destination.join("game.exe")).unwrap(),
            b"game"
        );
        let _ = std::fs::remove_dir_all(&repo_root);
        let _ = std::fs::remove_dir_all(&destination);
    }

    #[test]
    fn artifact_completion_check_only_fails_when_files_still_wait_on_artifacts() {
        assert!(!has_files_still_waiting_on_artifacts(
            &[true, true],
            &[0, 1]
        ));
        assert!(!has_files_still_waiting_on_artifacts(
            &[true, true],
            &[0, 0]
        ));
        assert!(has_files_still_waiting_on_artifacts(
            &[true, false],
            &[0, 1]
        ));
    }
}
