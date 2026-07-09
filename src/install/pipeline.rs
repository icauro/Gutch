use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::Write;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use super::apply::build_target_install_apply_result;
use super::transfer::{FetchTransferProgress, PendingBundleDownload, download_pending_bundle};
use super::types::{TargetInstallApplyFromRepoResult, TargetInstallDownloadAndApplyRequest};
use crate::manifest::target::TargetManifest;
use crate::progress::{
    GutchOperation, GutchProgressEvent, GutchProgressPhase, GutchProgressSink, emit_progress,
};
use crate::runtime::{RuntimeResourceHints, plan_reconstruct_runtime, plan_verify_runtime};
use crate::{Error, Result};

pub(super) struct InstallPipelinePlan {
    files: Vec<InstallPipelineFile>,
    bundle_to_file_indices: HashMap<String, Vec<usize>>,
    remaining_bundle_dependencies: Vec<usize>,
    pub required_bundle_ids: HashSet<String>,
    pub required_bundle_count: usize,
    required_chunk_count: usize,
    reused_source_chunk_count: usize,
}

#[derive(Clone)]
struct InstallPipelineFile {
    path: String,
    size: u64,
    full_file_hash: String,
    materialization: InstallPipelineMaterialization,
}

#[derive(Clone)]
enum InstallPipelineMaterialization {
    CopyFromSource {
        source_path: PathBuf,
    },
    ReconstructFromParts {
        chunks: Vec<InstallPipelineChunkInput>,
    },
}

#[derive(Clone)]
enum InstallPipelineChunkInput {
    Source {
        chunk_hash: String,
        raw_size: u64,
        source_path: PathBuf,
        source_offset: u64,
    },
    Bundle(InstallPipelineChunk),
}

#[derive(Clone)]
struct InstallPipelineChunk {
    chunk_hash: String,
    raw_size: u64,
    compressed_size: u64,
    bundle_id: String,
    bundle_path: PathBuf,
}

struct SourceChunkLocation {
    path: PathBuf,
    offset: u64,
}

enum BundlePipelineEvent {
    Ready(String),
    Failed(String),
    WorkerDone,
}

enum InstallPipelineEvent {
    Bundle(BundlePipelineEvent),
    FileCompleted(crate::ReconstructTimings),
    FileFailed(String),
    FileWorkerDone,
}

pub(super) fn build_install_pipeline_plan(
    target_manifest: &TargetManifest,
    chunk_index: &crate::manifest::ChunkIndex,
    repo_paths: &crate::GutchRepoPaths,
    existing_bundle_ids: &HashSet<String>,
    verified_source: Option<crate::reconstruct::VerifiedSourceInstall<'_>>,
) -> Result<InstallPipelinePlan> {
    let mut files = Vec::with_capacity(target_manifest.files.len());
    let mut bundle_to_file_indices: HashMap<String, Vec<usize>> = HashMap::new();
    let mut remaining_bundle_dependencies = Vec::with_capacity(target_manifest.files.len());
    let mut required_bundle_ids = HashSet::new();
    let mut reused_source_chunk_count = 0usize;
    let source_files_by_path = verified_source
        .map(|source| {
            source
                .target_manifest
                .files
                .iter()
                .map(|file| (file.path.as_str(), file))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let source_chunk_locations = verified_source
        .map(build_source_chunk_locations)
        .transpose()?
        .unwrap_or_default();

    for file_manifest in &target_manifest.files {
        if let Some(source_file) = source_files_by_path.get(file_manifest.path.as_str())
            && source_file.full_file_hash == file_manifest.full_file_hash
            && source_file.size == file_manifest.size
        {
            let source_root = verified_source.expect("source files map only exists with source");
            files.push(InstallPipelineFile {
                path: file_manifest.path.clone(),
                size: file_manifest.size,
                full_file_hash: file_manifest.full_file_hash.clone(),
                materialization: InstallPipelineMaterialization::CopyFromSource {
                    source_path: crate::path_guard::resolve_manifest_relative_path(
                        source_root.install_dir,
                        &file_manifest.path,
                    )?,
                },
            });
            remaining_bundle_dependencies.push(0);
            reused_source_chunk_count += file_manifest.chunks.len();
            continue;
        }

        let mut chunks = Vec::with_capacity(file_manifest.chunks.len());
        let mut missing_bundle_dependencies = HashSet::new();
        for chunk_ref in &file_manifest.chunks {
            if let Some(source_chunk) = source_chunk_locations.get(&chunk_ref.chunk_hash) {
                chunks.push(InstallPipelineChunkInput::Source {
                    chunk_hash: chunk_ref.chunk_hash.clone(),
                    raw_size: chunk_ref.raw_size,
                    source_path: source_chunk.path.clone(),
                    source_offset: source_chunk.offset,
                });
                reused_source_chunk_count += 1;
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
            if entry.storage_mode != crate::manifest::ArtifactStorageMode::Bundled {
                return Err(Error::manifest(format!(
                    "fresh install pipeline only supports bundled chunk storage, but '{}' uses {:?}",
                    file_manifest.path, entry.storage_mode
                )));
            }
            chunks.push(InstallPipelineChunkInput::Bundle(InstallPipelineChunk {
                chunk_hash: chunk_ref.chunk_hash.clone(),
                raw_size: chunk_ref.raw_size,
                compressed_size: entry.compressed_size,
                bundle_id: entry.bundle_id.clone(),
                bundle_path: crate::bundle_path(repo_paths, &entry.bundle_id),
            }));
            required_bundle_ids.insert(entry.bundle_id.clone());
            if !existing_bundle_ids.contains(&entry.bundle_id) {
                missing_bundle_dependencies.insert(entry.bundle_id.clone());
            }
        }

        let file_index = files.len();
        for bundle_id in &missing_bundle_dependencies {
            bundle_to_file_indices
                .entry(bundle_id.clone())
                .or_default()
                .push(file_index);
        }
        remaining_bundle_dependencies.push(missing_bundle_dependencies.len());
        files.push(InstallPipelineFile {
            path: file_manifest.path.clone(),
            size: file_manifest.size,
            full_file_hash: file_manifest.full_file_hash.clone(),
            materialization: InstallPipelineMaterialization::ReconstructFromParts { chunks },
        });
    }

    Ok(InstallPipelinePlan {
        files,
        bundle_to_file_indices,
        remaining_bundle_dependencies,
        required_bundle_ids: required_bundle_ids.clone(),
        required_bundle_count: required_bundle_ids.len(),
        required_chunk_count: target_manifest
            .files
            .iter()
            .map(|file| file.chunks.len())
            .sum(),
        reused_source_chunk_count,
    })
}

pub(super) fn apply_install_pipeline(
    request: &TargetInstallDownloadAndApplyRequest,
    repo_root: &Path,
    target_manifest: &TargetManifest,
    source_install_dir: Option<PathBuf>,
    existing_bundle_ids: &HashSet<String>,
    pending_bundle_downloads: &[PendingBundleDownload],
    pipeline_plan: InstallPipelinePlan,
    download_client: &reqwest::blocking::Client,
    transfer_progress: &Arc<FetchTransferProgress>,
) -> Result<TargetInstallApplyFromRepoResult> {
    let staging_dir = request.staging_dir.clone().unwrap_or_else(|| {
        super::apply::default_target_install_staging_dir(
            &request.install_dir,
            &target_manifest.target_id,
        )
    });
    if staging_dir.exists() {
        crate::helpers::remove_dir_all(&staging_dir)?;
    }
    if let Some(parent) = staging_dir.parent() {
        crate::helpers::create_dir_all(parent)?;
    }

    let progress = request.fetch_request.progress.clone();
    let main_result = (|| -> Result<TargetInstallApplyFromRepoResult> {
        let required_chunk_count = pipeline_plan.required_chunk_count;
        let required_bundle_count = pipeline_plan.required_bundle_count;
        let reused_source_chunk_count = pipeline_plan.reused_source_chunk_count;
        let (reconstruct_timings, verify_timings) = execute_install_pipeline_materialization(
            pipeline_plan,
            &staging_dir,
            &target_manifest.target_id,
            &request.runtime_hints,
            existing_bundle_ids,
            pending_bundle_downloads,
            &progress,
            Some(download_client),
            Some(transfer_progress),
            request.fetch_request.session_id.as_deref(),
        )?;
        let backup_dir = super::apply::promote_staging_install(
            &request.install_dir,
            &staging_dir,
            target_manifest,
        )?;
        Ok(TargetInstallApplyFromRepoResult {
            repo_root: repo_root.to_path_buf(),
            target_manifest: target_manifest.clone(),
            apply_result: build_target_install_apply_result(
                target_manifest,
                &request.install_dir,
                &staging_dir,
                backup_dir,
                reused_source_chunk_count,
                required_chunk_count,
                required_bundle_count,
                reconstruct_timings,
                verify_timings,
            ),
            resolved_bundle_file_count: required_bundle_count,
            source_install_dir,
        })
    })();

    if main_result.is_err() && staging_dir.exists() {
        let _ = crate::helpers::remove_dir_all(&staging_dir);
    }
    main_result
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_install_pipeline_materialization(
    mut pipeline_plan: InstallPipelinePlan,
    staging_dir: &Path,
    target_id: &str,
    runtime_hints: &RuntimeResourceHints,
    existing_bundle_ids: &HashSet<String>,
    pending_bundle_downloads: &[PendingBundleDownload],
    progress: &Option<Arc<dyn GutchProgressSink>>,
    download_client: Option<&reqwest::blocking::Client>,
    transfer_progress: Option<&Arc<FetchTransferProgress>>,
    session_id: Option<&str>,
) -> Result<(crate::ReconstructTimings, crate::verify::VerifyTimings)> {
    let reconstruct_started_at = Instant::now();
    let mut reconstruct_timings = crate::ReconstructTimings::default();
    reconstruct_timings.runtime_plan = Some(plan_reconstruct_runtime(runtime_hints));
    let mut verify_timings = crate::verify::VerifyTimings {
        verify_mode: crate::verify::VerifyMode::Fast,
        runtime_plan: Some(plan_verify_runtime(runtime_hints)),
        ..crate::verify::VerifyTimings::default()
    };
    let mut ready_files = VecDeque::new();
    let mut scheduled_files = vec![false; pipeline_plan.files.len()];
    let mut completed_file_count = 0usize;
    let mut available_bundle_ids = existing_bundle_ids.clone();

    for (file_index, remaining) in pipeline_plan
        .remaining_bundle_dependencies
        .iter()
        .enumerate()
    {
        if *remaining == 0 {
            scheduled_files[file_index] = true;
            ready_files.push_back(file_index);
        }
    }

    emit_progress(
        progress,
        GutchProgressEvent {
            operation: GutchOperation::Fetch,
            phase: GutchProgressPhase::Restore,
            message: "Installing files (0/0)".to_string(),
            current: 0,
            total: pipeline_plan.files.len().max(1),
            transferred_bytes: 0,
            total_bytes: 0,
            detail: Some(target_id.to_string()),
            done: false,
        },
    );

    let reconstruct_worker_count = reconstruct_timings
        .runtime_plan
        .as_ref()
        .map(|plan| plan.worker_count.max(1))
        .unwrap_or(1);
    let bundle_worker_count = pending_bundle_downloads.len();
    if bundle_worker_count == 0
        && pipeline_plan
            .remaining_bundle_dependencies
            .iter()
            .any(|remaining| *remaining > 0)
    {
        return Err(Error::reconstruct(
            "install pipeline requires bundles that are not available locally",
        ));
    }

    let (pipeline_event_tx, pipeline_event_rx) = mpsc::channel::<InstallPipelineEvent>();
    let bundle_task_receiver = Arc::new(Mutex::new(pending_bundle_downloads.to_vec().into_iter()));
    let (file_task_tx, file_task_rx) = mpsc::channel::<usize>();
    let file_task_rx = Arc::new(Mutex::new(file_task_rx));
    let file_plans = Arc::new(pipeline_plan.files.clone());
    let mut bundle_worker_handles = Vec::new();
    if let (Some(download_client), Some(transfer_progress)) = (download_client, transfer_progress) {
        for _ in 0..bundle_worker_count {
            let event_tx = pipeline_event_tx.clone();
            let task_receiver = Arc::clone(&bundle_task_receiver);
            let progress = progress.clone();
            let transfer_progress = Arc::clone(transfer_progress);
            let download_client = download_client.clone();
            let session_id = session_id.map(str::to_string);
            bundle_worker_handles.push(thread::spawn(move || {
                loop {
                    let next_task = match task_receiver.lock() {
                        Ok(mut tasks) => tasks.next(),
                        Err(_) => {
                            let _ = event_tx.send(InstallPipelineEvent::Bundle(
                                BundlePipelineEvent::Failed(
                                    "bundle task state is unavailable".to_string(),
                                ),
                            ));
                            return;
                        }
                    };
                    let Some(bundle_download) = next_task else {
                        let _ = event_tx.send(InstallPipelineEvent::Bundle(
                            BundlePipelineEvent::WorkerDone,
                        ));
                        return;
                    };
                    let result = download_pending_bundle(
                        &download_client,
                        &bundle_download,
                        session_id.as_deref(),
                        &progress,
                        &transfer_progress,
                    );
                    match result {
                        Ok(_) => {
                            let _ = event_tx.send(InstallPipelineEvent::Bundle(
                                BundlePipelineEvent::Ready(bundle_download.bundle_id),
                            ));
                        }
                        Err(error) => {
                            let _ = event_tx.send(InstallPipelineEvent::Bundle(
                                BundlePipelineEvent::Failed(error.to_string()),
                            ));
                            return;
                        }
                    }
                }
            }));
        }
    }

    let mut file_worker_handles = Vec::new();
    for _ in 0..reconstruct_worker_count {
        let event_tx = pipeline_event_tx.clone();
        let file_task_rx = Arc::clone(&file_task_rx);
        let staging_dir = staging_dir.to_path_buf();
        let file_plans = Arc::clone(&file_plans);
        file_worker_handles.push(thread::spawn(move || {
            loop {
                let next_file_index = match file_task_rx.lock() {
                    Ok(receiver) => receiver.recv(),
                    Err(_) => {
                        let _ = event_tx.send(InstallPipelineEvent::FileFailed(
                            "file task state is unavailable".to_string(),
                        ));
                        return;
                    }
                };
                let file_index = match next_file_index {
                    Ok(file_index) => file_index,
                    Err(_) => {
                        let _ = event_tx.send(InstallPipelineEvent::FileWorkerDone);
                        return;
                    }
                };
                match materialize_pipeline_file(&file_plans[file_index], &staging_dir) {
                    Ok(file_timings) => {
                        let _ = event_tx.send(InstallPipelineEvent::FileCompleted(file_timings));
                    }
                    Err(error) => {
                        let _ = event_tx.send(InstallPipelineEvent::FileFailed(error.to_string()));
                        return;
                    }
                }
            }
        }));
    }
    drop(pipeline_event_tx);

    let mut completed_bundle_workers = 0usize;
    let mut completed_file_workers = 0usize;
    let mut pipeline_error: Option<Error> = None;
    let queue_ready_file = |file_index: usize| -> Result<()> {
        file_task_tx
            .send(file_index)
            .map_err(|_| Error::reconstruct("install pipeline file queue closed unexpectedly"))
    };
    while completed_file_count < pipeline_plan.files.len() {
        while let Some(file_index) = ready_files.pop_front() {
            if let Err(error) = queue_ready_file(file_index) {
                pipeline_error = Some(error);
                break;
            }
        }

        if pipeline_error.is_some() || completed_file_count == pipeline_plan.files.len() {
            break;
        }

        match pipeline_event_rx.recv() {
            Ok(InstallPipelineEvent::Bundle(BundlePipelineEvent::Ready(bundle_id))) => {
                if available_bundle_ids.insert(bundle_id.clone())
                    && let Some(file_indices) = pipeline_plan.bundle_to_file_indices.get(&bundle_id)
                {
                    for &file_index in file_indices {
                        if pipeline_plan.remaining_bundle_dependencies[file_index] == 0 {
                            continue;
                        }
                        pipeline_plan.remaining_bundle_dependencies[file_index] -= 1;
                        if pipeline_plan.remaining_bundle_dependencies[file_index] == 0
                            && !scheduled_files[file_index]
                        {
                            scheduled_files[file_index] = true;
                            ready_files.push_back(file_index);
                        }
                    }
                }
            }
            Ok(InstallPipelineEvent::Bundle(BundlePipelineEvent::Failed(error))) => {
                pipeline_error = Some(Error::manifest(error));
                break;
            }
            Ok(InstallPipelineEvent::Bundle(BundlePipelineEvent::WorkerDone)) => {
                completed_bundle_workers += 1;
                if completed_bundle_workers == bundle_worker_count
                    && ready_files.is_empty()
                    && completed_file_count < pipeline_plan.files.len()
                {
                    pipeline_error = Some(Error::reconstruct(
                        "install pipeline stopped before all files became ready",
                    ));
                    break;
                }
            }
            Ok(InstallPipelineEvent::FileCompleted(file_timings)) => {
                completed_file_count += 1;
                accumulate_pipeline_timings(
                    &mut reconstruct_timings,
                    &mut verify_timings,
                    &file_timings,
                );
                emit_progress(
                    progress,
                    GutchProgressEvent {
                        operation: GutchOperation::Fetch,
                        phase: GutchProgressPhase::Restore,
                        message: format!(
                            "Installing files ({}/{})",
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
            Ok(InstallPipelineEvent::FileFailed(error)) => {
                pipeline_error = Some(Error::reconstruct(error));
                break;
            }
            Ok(InstallPipelineEvent::FileWorkerDone) => {
                completed_file_workers += 1;
                if completed_file_workers == reconstruct_worker_count
                    && completed_file_count < pipeline_plan.files.len()
                {
                    pipeline_error = Some(Error::reconstruct(
                        "install pipeline file workers stopped before install completed",
                    ));
                    break;
                }
            }
            Err(_) => {
                pipeline_error = Some(Error::reconstruct(
                    "install pipeline event channel closed unexpectedly",
                ));
                break;
            }
        }
    }

    drop(file_task_tx);
    for handle in bundle_worker_handles {
        handle
            .join()
            .map_err(|_| Error::reconstruct("bundle download worker panicked"))?;
    }
    for handle in file_worker_handles {
        handle
            .join()
            .map_err(|_| Error::reconstruct("file reconstruct worker panicked"))?;
    }

    if let Some(error) = pipeline_error {
        return Err(error);
    }

    reconstruct_timings.total = reconstruct_started_at.elapsed();
    verify_timings.total = reconstruct_timings.file_verify;
    Ok((reconstruct_timings, verify_timings))
}

fn materialize_pipeline_file(
    file_plan: &InstallPipelineFile,
    staging_dir: &Path,
) -> Result<crate::ReconstructTimings> {
    match &file_plan.materialization {
        InstallPipelineMaterialization::CopyFromSource { source_path } => {
            copy_pipeline_file_from_source(file_plan, source_path, staging_dir)
        }
        InstallPipelineMaterialization::ReconstructFromParts { chunks } => {
            reconstruct_pipeline_file_from_parts(file_plan, chunks, staging_dir)
        }
    }
}

fn copy_pipeline_file_from_source(
    file_plan: &InstallPipelineFile,
    source_path: &Path,
    staging_dir: &Path,
) -> Result<crate::ReconstructTimings> {
    let mut timings = crate::ReconstructTimings::default();
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
        let rebuild_started_at = Instant::now();
        let read = std::io::Read::read(&mut input, &mut buffer)?;
        timings.file_rebuild += rebuild_started_at.elapsed();
        if read == 0 {
            break;
        }
        let write_started_at = Instant::now();
        output.write_all(&buffer[..read])?;
        timings.file_write += write_started_at.elapsed();
        timings.file_write_bytes += read as u64;
        timings.file_rebuild_bytes += read as u64;
        hasher.update(&buffer[..read]);
        written += read as u64;
    }

    if written != file_plan.size {
        return Err(Error::reconstruct(format!(
            "copied size {} does not match expected {} for {}",
            written, file_plan.size, file_plan.path
        )));
    }

    let flush_started_at = Instant::now();
    output.flush()?;
    timings.file_write += flush_started_at.elapsed();
    let verify_started_at = Instant::now();
    let actual_hash = crate::hash::hash_hex(&hasher.finalize());
    timings.file_verify += verify_started_at.elapsed();
    timings.file_verify_bytes += written;
    if actual_hash != file_plan.full_file_hash {
        return Err(Error::HashMismatch {
            expected: file_plan.full_file_hash.clone(),
            actual: actual_hash,
        });
    }

    Ok(timings)
}

fn reconstruct_pipeline_file_from_parts(
    file_plan: &InstallPipelineFile,
    chunks: &[InstallPipelineChunkInput],
    staging_dir: &Path,
) -> Result<crate::ReconstructTimings> {
    let mut timings = crate::ReconstructTimings::default();
    let output_path =
        crate::path_guard::resolve_manifest_relative_path(staging_dir, &file_plan.path)?;
    if let Some(parent) = output_path.parent() {
        crate::helpers::create_dir_all(parent)?;
    }
    let mut output = std::io::BufWriter::new(File::create(&output_path)?);
    let mut file_hasher = blake3::Hasher::new();
    let mut written = 0u64;
    let mut bundle_readers: HashMap<
        String,
        (crate::bundle::BundleReader<File>, HashMap<String, usize>),
    > = HashMap::new();
    let mut source_readers: HashMap<PathBuf, std::io::BufReader<File>> = HashMap::new();

    for chunk in chunks {
        let chunk_bytes = match chunk {
            InstallPipelineChunkInput::Source {
                chunk_hash,
                raw_size,
                source_path,
                source_offset,
            } => {
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
                let actual_hash = crate::hash::hash_hex(&crate::hash::hash_chunk(&chunk_bytes));
                if actual_hash != *chunk_hash {
                    return Err(Error::HashMismatch {
                        expected: chunk_hash.clone(),
                        actual: actual_hash,
                    });
                }
                chunk_bytes
            }
            InstallPipelineChunkInput::Bundle(chunk) => {
                let reader_entry = match bundle_readers.entry(chunk.bundle_id.clone()) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let file = File::open(&chunk.bundle_path)?;
                        let reader = crate::bundle::BundleReader::open(file)?;
                        let chunk_index = reader.chunk_index();
                        entry.insert((reader, chunk_index))
                    }
                };
                let chunk_index = reader_entry
                    .1
                    .get(&chunk.chunk_hash)
                    .copied()
                    .ok_or_else(|| Error::MissingChunk(chunk.chunk_hash.clone()))?;
                let read_started_at = Instant::now();
                let chunk_bytes = reader_entry.0.read_chunk_bytes(chunk_index)?.to_vec();
                timings.bundle_read += read_started_at.elapsed();
                timings.bundle_read_compressed_bytes += chunk.compressed_size;
                timings.bundle_read_decompressed_bytes += chunk_bytes.len() as u64;
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
        let write_started_at = Instant::now();
        output.write_all(&chunk_bytes)?;
        timings.file_write += write_started_at.elapsed();
        timings.file_write_bytes += chunk_bytes.len() as u64;
        timings.file_rebuild_bytes += chunk_bytes.len() as u64;
        file_hasher.update(&chunk_bytes);
        written += chunk_bytes.len() as u64;
    }

    if written != file_plan.size {
        return Err(Error::reconstruct(format!(
            "reconstructed size {} does not match expected {} for {}",
            written, file_plan.size, file_plan.path
        )));
    }

    let flush_started_at = Instant::now();
    output.flush()?;
    timings.file_write += flush_started_at.elapsed();
    let verify_started_at = Instant::now();
    let actual_hash = crate::hash::hash_hex(&file_hasher.finalize());
    timings.file_verify += verify_started_at.elapsed();
    timings.file_verify_bytes += written;
    if actual_hash != file_plan.full_file_hash {
        return Err(Error::HashMismatch {
            expected: file_plan.full_file_hash.clone(),
            actual: actual_hash,
        });
    }

    Ok(timings)
}

fn build_source_chunk_locations(
    source: crate::reconstruct::VerifiedSourceInstall<'_>,
) -> Result<HashMap<String, SourceChunkLocation>> {
    let mut chunk_locations = HashMap::new();
    for file_manifest in &source.target_manifest.files {
        let typed_file = crate::manifest::runtime::typed_file_manifest(file_manifest)?;
        let source_path = crate::path_guard::resolve_manifest_relative_path(
            source.install_dir,
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
    reconstruct_timings: &mut crate::ReconstructTimings,
    verify_timings: &mut crate::verify::VerifyTimings,
    file_timings: &crate::ReconstructTimings,
) {
    reconstruct_timings.bundle_read += file_timings.bundle_read;
    reconstruct_timings.bundle_read_compressed_bytes += file_timings.bundle_read_compressed_bytes;
    reconstruct_timings.bundle_read_decompressed_bytes +=
        file_timings.bundle_read_decompressed_bytes;
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
    use crate::manifest::{self, ChunkRef, FileManifest, TargetTagSet};

    fn test_hash(bytes: &[u8]) -> String {
        hash_hex(&blake3::hash(bytes))
    }

    fn target_manifest_with_file(target_id: &str, full_file_hash: &str) -> TargetManifest {
        target_manifest_with_chunk(target_id, full_file_hash, test_hash(b"game"))
    }

    fn target_manifest_with_chunk(
        target_id: &str,
        full_file_hash: &str,
        chunk_hash: String,
    ) -> TargetManifest {
        let mut manifest = TargetManifest::new(target_id.to_string(), TargetTagSet::default());
        manifest.files.push(FileManifest {
            path: "game.exe".to_string(),
            size: 4,
            executable: true,
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
            format_version: "1.0.0".to_string(),
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

    fn target_manifest_with_two_chunks(
        target_id: &str,
        full_file_hash: &str,
        first_chunk_hash: String,
        second_chunk_hash: String,
    ) -> TargetManifest {
        let mut manifest = TargetManifest::new(target_id.to_string(), TargetTagSet::default());
        manifest.files.push(FileManifest {
            path: "game.exe".to_string(),
            size: 8,
            executable: true,
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
        let source_manifest = target_manifest_with_file("source", &test_hash(b"game"));
        let target_manifest = target_manifest_with_file("target", &test_hash(b"game"));
        let repo_root = crate::test_util::temp_path("gutch_pipeline_copy_repo");
        let _ = std::fs::remove_dir_all(&repo_root);
        let repo_paths = crate::GutchRepoPaths::from_repo_root(&repo_root);

        let plan = build_install_pipeline_plan(
            &target_manifest,
            &bundled_chunk_index(),
            &repo_paths,
            &HashSet::new(),
            Some(crate::reconstruct::VerifiedSourceInstall {
                target_manifest: &source_manifest,
                install_dir: &source_root,
            }),
        )
        .unwrap();

        assert_eq!(plan.required_bundle_count, 0);
        assert_eq!(plan.reused_source_chunk_count, 1);
        assert!(matches!(
            plan.files[0].materialization,
            InstallPipelineMaterialization::CopyFromSource { .. }
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
            target_manifest_with_chunk("source", &test_hash(b"old!"), test_hash(b"old!"));
        let target_manifest =
            target_manifest_with_chunk("target", &test_hash(b"game"), test_hash(b"game"));
        let repo_root = crate::test_util::temp_path("gutch_pipeline_rebuild_repo");
        let _ = std::fs::remove_dir_all(&repo_root);
        let repo_paths = crate::GutchRepoPaths::from_repo_root(&repo_root);

        let plan = build_install_pipeline_plan(
            &target_manifest,
            &bundled_chunk_index(),
            &repo_paths,
            &HashSet::new(),
            Some(crate::reconstruct::VerifiedSourceInstall {
                target_manifest: &source_manifest,
                install_dir: &source_root,
            }),
        )
        .unwrap();

        assert_eq!(plan.required_bundle_count, 1);
        assert_eq!(plan.reused_source_chunk_count, 0);
        assert!(matches!(
            plan.files[0].materialization,
            InstallPipelineMaterialization::ReconstructFromParts { .. }
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
        let target_only_chunk_hash = test_hash(b"new!");
        let source_manifest = target_manifest_with_two_chunks(
            "source",
            &test_hash(b"gameold!"),
            shared_chunk_hash.clone(),
            source_only_chunk_hash,
        );
        let target_manifest = target_manifest_with_two_chunks(
            "target",
            &test_hash(b"gamenew!"),
            shared_chunk_hash.clone(),
            target_only_chunk_hash.clone(),
        );
        let repo_root = crate::test_util::temp_path("gutch_pipeline_mixed_repo");
        let _ = std::fs::remove_dir_all(&repo_root);
        let repo_paths = crate::GutchRepoPaths::from_repo_root(&repo_root);
        let chunk_index = crate::manifest::ChunkIndex {
            format_version: "1.0.0".to_string(),
            updated_at: String::new(),
            entries: std::collections::BTreeMap::from([(
                target_only_chunk_hash,
                manifest::ChunkIndexEntry {
                    bundle_id: "bundle-b".to_string(),
                    raw_size: 4,
                    compressed_size: 4,
                    storage_mode: manifest::ArtifactStorageMode::Bundled,
                },
            )]),
        };

        let plan = build_install_pipeline_plan(
            &target_manifest,
            &chunk_index,
            &repo_paths,
            &HashSet::new(),
            Some(crate::reconstruct::VerifiedSourceInstall {
                target_manifest: &source_manifest,
                install_dir: &source_root,
            }),
        )
        .unwrap();

        assert_eq!(plan.required_bundle_count, 1);
        assert_eq!(plan.reused_source_chunk_count, 1);
        assert!(matches!(
            &plan.files[0].materialization,
            InstallPipelineMaterialization::ReconstructFromParts { chunks }
                if matches!(chunks[0], InstallPipelineChunkInput::Source { .. })
                    && matches!(chunks[1], InstallPipelineChunkInput::Bundle(_))
        ));

        let _ = std::fs::remove_dir_all(&source_root);
        let _ = std::fs::remove_dir_all(&repo_root);
    }
}
