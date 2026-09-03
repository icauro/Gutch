use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::io::{Read, Seek, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rayon::ThreadPool;
use rayon::prelude::*;

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
    jobs: Vec<ReconstructJob>,
    missing_artifact_to_file_indices: HashMap<crate::ArtifactRef, Vec<usize>>,
    missing_artifact_counts: Vec<usize>,
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
    trace_metadata: ReconstructFileTraceMetadata,
    #[allow(dead_code)]
    reconstruction: ReconstructPipelineReconstruction,
}

#[derive(Clone, Copy, Default)]
struct ReconstructFileTraceMetadata {
    required_distinct_chunks: usize,
    required_distinct_artifacts: usize,
    required_distinct_bundles: usize,
    required_distinct_loose_chunks: usize,
}

#[derive(Clone)]
enum ReconstructPipelineReconstruction {
    CopyFromSource,
    ReconstructFromParts {
        chunks: Vec<ReconstructPipelineChunkInput>,
    },
}

#[derive(Clone)]
enum ReconstructPipelineChunkInput {
    Source { chunk_hash: String },
    Artifact(ReconstructPipelineChunk),
}

#[derive(Clone)]
struct ReconstructPipelineChunk {
    chunk_hash: String,
    artifact: crate::ArtifactRef,
}

#[derive(Clone)]
enum ReconstructJob {
    Artifact(ReconstructArtifactJob),
    SourceChunk(ReconstructSourceChunkJob),
    CopySourceFile {
        file_index: usize,
        source_path: PathBuf,
    },
}

#[derive(Clone)]
struct ReconstructArtifactJob {
    artifact: crate::ArtifactRef,
    artifact_path: PathBuf,
    chunks: HashMap<String, ReconstructArtifactChunk>,
}

#[derive(Clone)]
struct ReconstructArtifactChunk {
    chunk_hash: String,
    raw_size: u64,
    compressed_size: u64,
    targets: Vec<ReconstructTarget>,
}

#[derive(Clone)]
struct ReconstructSourceChunkJob {
    chunk_hash: String,
    raw_size: u64,
    source_path: PathBuf,
    source_offset: u64,
    targets: Vec<ReconstructTarget>,
}

#[derive(Clone)]
struct ReconstructTarget {
    file_index: usize,
    file_offset: u64,
}

struct SourceChunkLocation {
    path: PathBuf,
    offset: u64,
    raw_size: u64,
}

pub(super) fn prepare_snapshot_entries(
    snapshot_manifest: &SnapshotManifest,
    output_dir: &Path,
    control: &GutchOperationControl,
    reconstruct_pool: Option<&ThreadPool>,
) -> Result<Arc<Vec<PathBuf>>> {
    {
        let _validate_span =
            control.profile_span("gutch.snapshot.reconstruct.prepare.entries.validate");
        crate::manifest::validate_snapshot_entries(&snapshot_manifest.entries)?;
    }
    {
        let _root_span =
            control.profile_span("gutch.snapshot.reconstruct.prepare.entries.create_root");
        crate::helpers::create_dir_all(output_dir)?;
    }
    let path_resolver = {
        let _resolver_span =
            control.profile_span("gutch.snapshot.reconstruct.prepare.entries.path_resolver");
        crate::path_guard::ManifestPathResolver::new(output_dir)?
    };
    {
        let _directories_span =
            control.profile_span("gutch.snapshot.reconstruct.prepare.entries.directories");
        create_snapshot_directories(snapshot_manifest, control, &path_resolver, reconstruct_pool)?;
    }
    Ok(Arc::new(resolve_snapshot_file_paths(
        snapshot_manifest,
        control,
        &path_resolver,
        reconstruct_pool,
    )?))
}

fn resolve_snapshot_file_paths(
    snapshot_manifest: &SnapshotManifest,
    control: &GutchOperationControl,
    path_resolver: &crate::path_guard::ManifestPathResolver,
    reconstruct_pool: Option<&ThreadPool>,
) -> Result<Vec<PathBuf>> {
    let files = snapshot_manifest.files().collect::<Vec<_>>();
    let _resolve_paths_span = control.profile_span_with_args(
        "gutch.snapshot.reconstruct.prepare.outputs.resolve_paths",
        std::collections::BTreeMap::from([(
            "path_count".to_string(),
            serde_json::json!(files.len()),
        )]),
    );
    let resolve = |file: &&crate::manifest::FileManifest| path_resolver.resolve(&file.path);
    if let Some(reconstruct_pool) = reconstruct_pool {
        reconstruct_pool.install(|| files.par_iter().map(resolve).collect::<Vec<_>>())
    } else {
        files.iter().map(resolve).collect::<Vec<_>>()
    }
    .into_iter()
    .collect()
}

fn create_snapshot_directories(
    snapshot_manifest: &SnapshotManifest,
    control: &GutchOperationControl,
    path_resolver: &crate::path_guard::ManifestPathResolver,
    reconstruct_pool: Option<&ThreadPool>,
) -> Result<()> {
    let mut directories = BTreeMap::<String, Option<bool>>::new();
    for entry in &snapshot_manifest.entries {
        if let crate::manifest::SnapshotEntry::Directory(directory) = entry {
            add_manifest_directory_prefixes(&mut directories, &directory.path);
            directories.insert(directory.path.clone(), Some(directory.read_only));
        }
    }
    for entry in &snapshot_manifest.entries {
        let crate::manifest::SnapshotEntry::File(file) = entry else {
            continue;
        };
        let parent = file.path.rsplit_once('/').map(|(parent, _)| parent);
        if let Some(parent) = parent {
            add_manifest_directory_prefixes(&mut directories, parent);
        }
    }
    let mut directories_by_depth = BTreeMap::<usize, Vec<_>>::new();
    for (path, read_only) in directories {
        directories_by_depth
            .entry(path.matches('/').count())
            .or_default()
            .push((path, read_only));
    }
    for directories in directories_by_depth.values_mut() {
        directories.sort_by(|left, right| left.0.cmp(&right.0));
    }
    for directories in directories_by_depth.values() {
        let create_directory = |(path, read_only): &(String, Option<bool>)| {
            let _directory_span = control.profile_span_with_args(
                "gutch.snapshot.reconstruct.prepare.entries.directory",
                prepare_directory_trace_args(path, *read_only),
            );
            let resolved_path = path_resolver.resolve(path)?;
            create_snapshot_directory(&resolved_path)
        };
        if directories.len() >= 2 {
            if let Some(reconstruct_pool) = reconstruct_pool {
                reconstruct_pool
                    .install(|| directories.par_iter().try_for_each(create_directory))?;
            } else {
                directories.iter().try_for_each(create_directory)?;
            }
        } else {
            directories.iter().try_for_each(create_directory)?;
        }
    }
    Ok(())
}

fn add_manifest_directory_prefixes(directories: &mut BTreeMap<String, Option<bool>>, path: &str) {
    let components = path.split('/').collect::<Vec<_>>();
    let mut prefix = String::new();
    for component in components {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(component);
        directories.entry(prefix.clone()).or_insert(None);
    }
}

fn create_snapshot_directory(path: &Path) -> Result<()> {
    match std::fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if std::fs::metadata(path)?.is_dir() {
                Ok(())
            } else {
                Err(error.into())
            }
        }
        Err(error) => Err(error.into()),
    }
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
    control: &GutchOperationControl,
    metadata_worker_count: usize,
    reconstruct_pool: Option<&ThreadPool>,
) -> Result<()> {
    let path_resolver = crate::path_guard::ManifestPathResolver::new(output_dir)?;
    let symlinks = snapshot_manifest
        .entries
        .iter()
        .filter_map(|entry| match entry {
            crate::manifest::SnapshotEntry::Symlink(link) => Some(link),
            _ => None,
        })
        .collect::<Vec<_>>();
    {
        let _symlink_phase = control.profile_span_with_args(
            "gutch.snapshot.reconstruct.finalize.entries.symlinks",
            finalize_phase_trace_args(
                symlinks.len(),
                symlinks.len(),
                metadata_worker_count,
                should_parallelize_finalize(symlinks.len(), metadata_worker_count),
            ),
        );
        run_finalize_entries(&symlinks, metadata_worker_count, reconstruct_pool, |link| {
            let _entry_span = control.profile_span_with_args(
                "gutch.snapshot.reconstruct.finalize.entries.symlink",
                finalize_symlink_trace_args(link),
            );
            let link_path = path_resolver.resolve(&link.path)?;
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
            Ok(())
        })?;
    }

    let file_count = snapshot_manifest.files().count();
    let files_needing_metadata = snapshot_manifest
        .files()
        .filter(|file| file.read_only || (cfg!(unix) && file.executable))
        .collect::<Vec<_>>();
    {
        let _file_phase = control.profile_span_with_args(
            "gutch.snapshot.reconstruct.finalize.entries.files",
            finalize_phase_trace_args(
                file_count,
                files_needing_metadata.len(),
                metadata_worker_count,
                should_parallelize_finalize(files_needing_metadata.len(), metadata_worker_count),
            ),
        );
        run_finalize_entries(
            &files_needing_metadata,
            metadata_worker_count,
            reconstruct_pool,
            |file| {
                let _entry_span = control.profile_span_with_args(
                    "gutch.snapshot.reconstruct.finalize.entries.file",
                    finalize_file_trace_args(file),
                );
                let path = path_resolver.resolve(&file.path)?;
                let mut permissions = std::fs::metadata(&path)?.permissions();
                let mut changed = false;
                #[cfg(unix)]
                if file.executable {
                    use std::os::unix::fs::PermissionsExt;
                    if permissions.mode() & 0o111 == 0 {
                        permissions.set_mode(permissions.mode() | 0o111);
                        changed = true;
                    }
                }
                if permissions.readonly() != file.read_only {
                    permissions.set_readonly(file.read_only);
                    changed = true;
                }
                if changed {
                    std::fs::set_permissions(&path, permissions)?;
                }
                Ok(())
            },
        )?;
    }

    let directory_count = snapshot_manifest
        .entries
        .iter()
        .filter(|entry| matches!(entry, crate::manifest::SnapshotEntry::Directory(_)))
        .count();
    let mut directories_by_depth = BTreeMap::<usize, Vec<_>>::new();
    for directory in snapshot_manifest
        .entries
        .iter()
        .filter_map(|entry| match entry {
            crate::manifest::SnapshotEntry::Directory(directory) if directory.read_only => {
                Some(directory)
            }
            _ => None,
        })
    {
        directories_by_depth
            .entry(directory.path.matches('/').count())
            .or_default()
            .push(directory);
    }
    let directories_needing_metadata = directories_by_depth.values().map(Vec::len).sum();
    let directories_parallel = directories_by_depth
        .values()
        .any(|directories| should_parallelize_finalize(directories.len(), metadata_worker_count));
    {
        let _directory_phase = control.profile_span_with_args(
            "gutch.snapshot.reconstruct.finalize.entries.directories",
            finalize_phase_trace_args(
                directory_count,
                directories_needing_metadata,
                metadata_worker_count,
                directories_parallel,
            ),
        );
        run_finalize_directory_levels(
            &directories_by_depth,
            metadata_worker_count,
            reconstruct_pool,
            |directory| {
                let _entry_span = control.profile_span_with_args(
                    "gutch.snapshot.reconstruct.finalize.entries.directory",
                    finalize_directory_trace_args(directory),
                );
                let path = path_resolver.resolve(&directory.path)?;
                let mut permissions = std::fs::metadata(&path)?.permissions();
                if permissions.readonly() != directory.read_only {
                    permissions.set_readonly(directory.read_only);
                    std::fs::set_permissions(path, permissions)?;
                }
                Ok(())
            },
        )?;
    }
    Ok(())
}

const FINALIZE_ENTRIES_PARALLEL_MIN: usize = 8;

fn run_finalize_entries<T, F>(
    entries: &[T],
    worker_count: usize,
    reconstruct_pool: Option<&ThreadPool>,
    work: F,
) -> Result<()>
where
    T: Sync,
    F: Fn(&T) -> Result<()> + Send + Sync,
{
    if !should_parallelize_finalize(entries.len(), worker_count) || reconstruct_pool.is_none() {
        return entries.iter().try_for_each(|entry| work(entry));
    }
    reconstruct_pool
        .expect("parallel finalization pool should exist")
        .install(|| entries.par_iter().try_for_each(|entry| work(entry)))
}

fn run_finalize_directory_levels<'a, F>(
    levels: &BTreeMap<usize, Vec<&'a crate::manifest::DirectoryManifest>>,
    worker_count: usize,
    reconstruct_pool: Option<&ThreadPool>,
    work: F,
) -> Result<()>
where
    F: Fn(&crate::manifest::DirectoryManifest) -> Result<()> + Send + Sync,
{
    for directories in levels.values().rev() {
        if should_parallelize_finalize(directories.len(), worker_count)
            && let Some(reconstruct_pool) = reconstruct_pool
        {
            reconstruct_pool.install(|| {
                directories
                    .par_iter()
                    .try_for_each(|directory| work(directory))
            })?;
        } else {
            directories
                .iter()
                .try_for_each(|directory| work(directory))?;
        }
    }
    Ok(())
}

fn should_parallelize_finalize(entry_count: usize, worker_count: usize) -> bool {
    entry_count >= FINALIZE_ENTRIES_PARALLEL_MIN && worker_count >= 2
}

fn finalize_phase_trace_args(
    total_entries: usize,
    changed_entries: usize,
    worker_count: usize,
    parallel: bool,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    std::collections::BTreeMap::from([
        (
            "total_entries".to_string(),
            serde_json::json!(total_entries),
        ),
        (
            "changed_entries".to_string(),
            serde_json::json!(changed_entries),
        ),
        ("parallel".to_string(), serde_json::json!(parallel)),
        (
            "parallel_workers".to_string(),
            serde_json::json!(if parallel { worker_count } else { 1 }),
        ),
    ])
}

fn prepare_directory_trace_args(
    path: &str,
    read_only: Option<bool>,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    std::collections::BTreeMap::from([
        ("path".to_string(), serde_json::json!(path)),
        (
            "depth".to_string(),
            serde_json::json!(path.matches('/').count()),
        ),
        ("read_only".to_string(), serde_json::json!(read_only)),
        (
            "implicit".to_string(),
            serde_json::json!(read_only.is_none()),
        ),
    ])
}

fn finalize_symlink_trace_args(
    link: &crate::manifest::SymlinkManifest,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    std::collections::BTreeMap::from([
        ("path".to_string(), serde_json::json!(link.path)),
        (
            "target_path".to_string(),
            serde_json::json!(link.target_path),
        ),
    ])
}

fn finalize_file_trace_args(
    file: &crate::manifest::FileManifest,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut args = crate::profile::file_trace_args(&file.path, file.size);
    args.insert("read_only".to_string(), serde_json::json!(file.read_only));
    args.insert("executable".to_string(), serde_json::json!(file.executable));
    args
}

fn finalize_directory_trace_args(
    directory: &crate::manifest::DirectoryManifest,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    std::collections::BTreeMap::from([
        ("path".to_string(), serde_json::json!(directory.path)),
        (
            "read_only".to_string(),
            serde_json::json!(directory.read_only),
        ),
    ])
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
    repo_paths: &crate::GutchRepoPaths,
    existing_artifacts: &HashSet<crate::ArtifactRef>,
    verified_source: Option<SnapshotReconstructSource<'_>>,
) -> Result<ReconstructPipelinePlan> {
    let mut files = Vec::with_capacity(snapshot_manifest.file_count());
    let mut jobs = Vec::new();
    let mut artifact_job_indices: HashMap<crate::ArtifactRef, usize> = HashMap::new();
    let mut source_chunk_job_indices: HashMap<(PathBuf, u64, u64, String), usize> = HashMap::new();
    let mut missing_artifact_to_file_indices: HashMap<crate::ArtifactRef, Vec<usize>> =
        HashMap::new();
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
            let source_path = crate::path_guard::resolve_manifest_relative_path(
                source_root.destination_dir,
                &file_manifest.path,
            )?;
            files.push(ReconstructPipelineFile {
                path: file_manifest.path.clone(),
                size: file_manifest.size,
                full_file_hash: file_manifest.full_file_hash.clone(),
                trace_metadata: ReconstructFileTraceMetadata::default(),
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
                .filter(|source_chunk| source_chunk.raw_size == chunk_ref.raw_size)
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
                bundle_artifacts.insert(artifact.clone());
            }
            let job_index = if let Some(&job_index) = artifact_job_indices.get(&artifact) {
                job_index
            } else {
                let job_index = jobs.len();
                jobs.push(ReconstructJob::Artifact(ReconstructArtifactJob {
                    artifact: artifact.clone(),
                    artifact_path: artifact.local_path(repo_paths)?,
                    chunks: HashMap::new(),
                }));
                artifact_job_indices.insert(artifact.clone(), job_index);
                job_index
            };
            let ReconstructJob::Artifact(artifact_job) = &mut jobs[job_index] else {
                unreachable!("artifact job index points to another job kind");
            };
            let artifact_chunk = artifact_job
                .chunks
                .entry(chunk_ref.chunk_hash.clone())
                .or_insert_with(|| ReconstructArtifactChunk {
                    chunk_hash: chunk_ref.chunk_hash.clone(),
                    raw_size: chunk_ref.raw_size,
                    compressed_size: entry.compressed_size,
                    targets: Vec::new(),
                });
            if artifact_chunk.raw_size != chunk_ref.raw_size
                || artifact_chunk.compressed_size != entry.compressed_size
            {
                return Err(Error::manifest(format!(
                    "chunk '{}' has inconsistent sizes in artifact '{}'",
                    chunk_ref.chunk_hash,
                    artifact.id()
                )));
            }
            artifact_chunk.targets.push(ReconstructTarget {
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
            full_file_hash: file_manifest.full_file_hash.clone(),
            trace_metadata,
            reconstruction,
        });
    }

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

pub(super) fn apply_reconstruct_pipeline(
    request: ReconstructPipelineApplyRequest<'_>,
    pipeline_plan: ReconstructPipelinePlan,
) -> Result<SnapshotReconstructFromRepoResult> {
    request.control.checkpoint()?;
    let operation_started_at = Instant::now();
    let _operation_span = request
        .control
        .profile_span("gutch.snapshot.reconstruct.apply");
    let prepare_started_at = Instant::now();
    let reconstruct_worker_count = plan_reconstruct_runtime(request.runtime_hints).worker_count;
    let reconstruct_pool = if request.snapshot_manifest.entries.is_empty() {
        None
    } else {
        let _pool_span = request
            .control
            .profile_span("gutch.snapshot.reconstruct.prepare.pool");
        Some(build_reconstruct_pool(reconstruct_worker_count)?)
    };
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
        let output_paths = {
            let _prepare_span = request
                .control
                .profile_span("gutch.snapshot.reconstruct.prepare.entries");
            prepare_snapshot_entries(
                request.snapshot_manifest,
                &staging_dir,
                request.control,
                reconstruct_pool.as_ref(),
            )?
        };
        let apply_prepare_wall = prepare_started_at.elapsed();
        let required_chunk_count = pipeline_plan.required_chunk_count;
        let required_bundle_count = pipeline_plan.required_bundle_count;
        let required_artifact_count = pipeline_plan.required_artifact_count;
        let reused_source_chunk_count = pipeline_plan.reused_source_chunk_count;
        let (mut reconstruct_timings, verify_timings) =
            execute_reconstruct_pipeline_reconstruction(ExecuteReconstructPipelineRequest {
                pipeline_plan,
                output_paths,
                snapshot_id: &request.snapshot_manifest.snapshot_id,
                compression_profile: &request.compression_profile,
                runtime_hints: request.runtime_hints,
                existing_artifacts: request.existing_artifacts,
                pending_bundle_downloads: request.pending_bundle_downloads,
                transfer_policy: request.transfer_policy,
                control: request.control,
                download_client: request.download_client,
                transfer_progress: request.transfer_progress,
                reconstruct_pool: reconstruct_pool.as_ref(),
            })?;
        reconstruct_timings.prepare_wall += apply_prepare_wall;
        reconstruct_timings.total = operation_started_at.elapsed();
        let finalize_started_at = Instant::now();
        {
            let _finalize_span = request
                .control
                .profile_span("gutch.snapshot.reconstruct.finalize.entries");
            let metadata_worker_count =
                plan_reconstruct_runtime(request.runtime_hints).worker_count;
            finalize_snapshot_entries(
                request.snapshot_manifest,
                &staging_dir,
                request.control,
                metadata_worker_count,
                reconstruct_pool.as_ref(),
            )?;
        }
        reconstruct_timings.finalize_wall = finalize_started_at.elapsed();
        // Promotion is a short consistency-critical section. Observe cancellation immediately
        // before it, then finish the swap without interruption.
        let promote_started_at = Instant::now();
        {
            let _promote_span = request
                .control
                .profile_span("gutch.snapshot.reconstruct.finalize.promote");
            request.control.checkpoint()?;
            crate::helpers::replace_directory(&staging_dir, request.destination_dir)?;
        }
        reconstruct_timings.promote_wall = promote_started_at.elapsed();
        reconstruct_timings.total = operation_started_at.elapsed();
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

pub(super) fn build_reconstruct_pool(worker_count: usize) -> Result<ThreadPool> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(worker_count)
        .thread_name(|index| format!("gutch-reconstruct-{index}"))
        .build()
        .map_err(|error| {
            Error::reconstruct(format!(
                "failed to create reconstruction thread pool: {error}"
            ))
        })
}

pub(super) struct ExecuteReconstructPipelineRequest<'a> {
    pub(super) pipeline_plan: ReconstructPipelinePlan,
    pub(super) output_paths: Arc<Vec<PathBuf>>,
    pub(super) snapshot_id: &'a str,
    pub(super) compression_profile: &'a crate::manifest::CompressionProfile,
    pub(super) runtime_hints: &'a RuntimeResourceHints,
    pub(super) existing_artifacts: &'a HashSet<crate::ArtifactRef>,
    pub(super) pending_bundle_downloads: &'a [PendingBundleDownload],
    pub(super) transfer_policy: super::types::TransferConcurrencyPolicy,
    pub(super) control: &'a GutchOperationControl,
    pub(super) download_client: Option<&'a reqwest::blocking::Client>,
    pub(super) transfer_progress: Option<&'a Arc<FetchTransferProgress>>,
    pub(super) reconstruct_pool: Option<&'a ThreadPool>,
}

enum ReconstructWorkItem {
    Job(usize),
    VerifyFile(usize),
}

enum ReconstructJobEvent {
    Completed {
        job_index: usize,
        timings: ReconstructTimings,
    },
    Verified(ReconstructedFileVerification),
    Failed(Error),
}

struct ReconstructWorkContext {
    jobs: Arc<Vec<ReconstructJob>>,
    file_plans: Arc<Vec<ReconstructPipelineFile>>,
    output_paths: Arc<Vec<PathBuf>>,
    output_handles: Arc<OutputHandleCache>,
    compression_profile: Arc<crate::manifest::CompressionProfile>,
    control: GutchOperationControl,
    reconstruct_worker_count: usize,
}

fn spawn_reconstruct_work(
    pool: &ThreadPool,
    work_item: ReconstructWorkItem,
    context: &Arc<ReconstructWorkContext>,
    event_tx: &mpsc::Sender<ReconstructJobEvent>,
) {
    let context = Arc::clone(context);
    let event_tx = event_tx.clone();
    pool.spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| match work_item {
            ReconstructWorkItem::Job(job_index) => reconstruct_job(
                &context.jobs[job_index],
                &context.file_plans,
                &context.output_paths,
                &context.output_handles,
                &context.compression_profile,
                &context.control,
            )
            .map(|timings| ReconstructJobEvent::Completed { job_index, timings }),
            ReconstructWorkItem::VerifyFile(file_index) => verify_reconstructed_output_file(
                file_index,
                &context.file_plans[file_index],
                &context.output_paths,
                &context.control,
                context.reconstruct_worker_count,
            )
            .map(ReconstructJobEvent::Verified),
        }));
        let result = match result {
            Ok(result) => result,
            Err(_) => Err(Error::reconstruct("reconstruction worker panicked")),
        };
        match result {
            Ok(event) => {
                let _ = event_tx.send(event);
            }
            Err(error) => {
                let _ = event_tx.send(ReconstructJobEvent::Failed(error));
            }
        }
    });
}

pub(super) fn execute_reconstruct_pipeline_reconstruction(
    request: ExecuteReconstructPipelineRequest<'_>,
) -> Result<(ReconstructTimings, crate::verify::VerifyTimings)> {
    let ExecuteReconstructPipelineRequest {
        pipeline_plan,
        output_paths,
        snapshot_id,
        compression_profile,
        runtime_hints,
        existing_artifacts,
        pending_bundle_downloads,
        transfer_policy,
        control,
        download_client,
        transfer_progress,
        reconstruct_pool,
    } = request;

    control.checkpoint()?;
    let _pipeline_span = control.profile_span("gutch.snapshot.reconstruct.pipeline");
    let pipeline_started_at = Instant::now();
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

    let artifact_priorities = {
        let _priorities_span =
            control.profile_span("gutch.snapshot.reconstruct.prepare.priorities");
        build_artifact_priorities(&pipeline_plan, pending_bundle_downloads)
    };
    let file_plans = Arc::new(pipeline_plan.files);
    let output_sizes = Arc::new(file_plans.iter().map(|file| file.size).collect::<Vec<_>>());
    let output_handles = Arc::new(OutputHandleCache::new(
        Arc::clone(&output_paths),
        Arc::clone(&output_sizes),
    ));
    let jobs = Arc::new(pipeline_plan.jobs);
    let (artifact_job_indices, artifact_scheduler) = {
        let _artifacts_span = control.profile_span("gutch.snapshot.reconstruct.prepare.artifacts");
        let pending_artifacts = pending_bundle_downloads
            .iter()
            .map(|download| download.artifact.clone())
            .collect::<HashSet<_>>();
        let mut artifact_job_indices = HashMap::new();
        for (job_index, job) in jobs.iter().enumerate() {
            let ReconstructJob::Artifact(job) = job else {
                continue;
            };
            artifact_job_indices.insert(job.artifact.clone(), job_index);
            if !existing_artifacts.contains(&job.artifact)
                && !pending_artifacts.contains(&job.artifact)
            {
                return Err(Error::reconstruct(format!(
                    "required artifact '{}' is neither local nor scheduled for download",
                    job.artifact.id()
                )));
            }
            if existing_artifacts.contains(&job.artifact) && !job.artifact_path.is_file() {
                return Err(Error::reconstruct(format!(
                    "required artifact '{}' is not available at {}",
                    job.artifact.id(),
                    job.artifact_path.display()
                )));
            }
        }
        let artifact_scheduler = if pending_bundle_downloads.is_empty() {
            None
        } else {
            let Some(download_client) = download_client else {
                return Err(Error::reconstruct(
                    "bundled reconstruction requires a download client for pending bundles",
                ));
            };
            let Some(transfer_progress) = transfer_progress else {
                return Err(Error::reconstruct(
                    "bundled reconstruction requires transfer progress for pending bundles",
                ));
            };
            Some(ArtifactDownloadScheduler::new(
                pending_bundle_downloads,
                artifact_priorities,
                transfer_policy,
                download_client,
                control,
                Arc::clone(transfer_progress),
            )?)
        };
        (artifact_job_indices, artifact_scheduler)
    };
    let artifact_worker_count = artifact_scheduler
        .as_ref()
        .map(ArtifactDownloadScheduler::worker_count)
        .unwrap_or(0);

    emit_progress(
        control.progress(),
        GutchProgressEvent {
            operation: GutchOperation::Fetch,
            phase: GutchProgressPhase::Reconstruct,
            message: format!("Preparing files (0/{})", file_plans.len().max(1)),
            current: 0,
            total: file_plans.len().max(1),
            transferred_bytes: 0,
            total_bytes: 0,
            detail: Some(snapshot_id.to_string()),
            done: false,
        },
    );

    let (event_tx, event_rx) = mpsc::channel::<ReconstructJobEvent>();
    let compression_profile = Arc::new((*compression_profile).clone());
    let work_context = Arc::new(ReconstructWorkContext {
        jobs: Arc::clone(&jobs),
        file_plans: Arc::clone(&file_plans),
        output_paths: Arc::clone(&output_paths),
        output_handles,
        compression_profile,
        control: control.clone(),
        reconstruct_worker_count: runtime_plan.worker_count,
    });
    let mut submitted_work_count = 0usize;
    let mut submitted_job_count = 0usize;
    let mut downloads_done = artifact_scheduler.is_none();
    let mut completed_download_worker_count = 0usize;

    let mut completed_job_count = 0usize;
    let mut completed_file_count = 0usize;
    let mut scheduled_job_indices = HashSet::new();
    let mut verification_queued = vec![false; file_plans.len()];
    let mut remaining_file_jobs = vec![0usize; file_plans.len()];
    let job_file_indices = jobs
        .iter()
        .map(|job| reconstruct_job_file_indices(job))
        .collect::<Vec<_>>();
    for file_indices in &job_file_indices {
        for &file_index in file_indices {
            if let Some(remaining) = remaining_file_jobs.get_mut(file_index) {
                *remaining += 1;
            }
        }
    }
    let mut verify_started_at = None;
    let mut verify_finished_at = None;
    let mut verify_span = None;
    let mut verified_file_bytes = 0u64;
    let mut pipeline_error = None;
    let mut finished_work_count = 0usize;
    let reconstruct_started_at = Instant::now();
    let mut reconstruct_finished_at = jobs.is_empty().then_some(reconstruct_started_at);
    reconstruct_timings.prepare_wall = pipeline_started_at.elapsed();

    let pool = match reconstruct_pool {
        Some(pool) => pool,
        None if jobs.is_empty() && file_plans.is_empty() => {
            reconstruct_timings.total = pipeline_started_at.elapsed();
            return Ok((reconstruct_timings, verify_timings));
        }
        None => {
            return Err(Error::reconstruct(
                "reconstruction thread pool is unavailable",
            ));
        }
    };
    {
        let _dispatch_span = control.profile_span("gutch.snapshot.reconstruct.dispatch.ready");
        for (job_index, job) in jobs.iter().enumerate() {
            let ready = match job {
                ReconstructJob::Artifact(job) => existing_artifacts.contains(&job.artifact),
                ReconstructJob::SourceChunk(_) | ReconstructJob::CopySourceFile { .. } => true,
            };
            if ready {
                spawn_reconstruct_work(
                    pool,
                    ReconstructWorkItem::Job(job_index),
                    &work_context,
                    &event_tx,
                );
                scheduled_job_indices.insert(job_index);
                submitted_work_count += 1;
                submitted_job_count += 1;
            }
        }
    }
    update_reconstruct_backpressure(
        transfer_progress,
        submitted_job_count,
        completed_job_count,
        runtime_plan.worker_count,
    );
    for file_index in 0..file_plans.len() {
        if remaining_file_jobs[file_index] == 0 {
            ensure_empty_reconstruct_output_file(
                &file_plans[file_index],
                &output_paths[file_index],
                control,
            )?;
            spawn_reconstruct_work(
                pool,
                ReconstructWorkItem::VerifyFile(file_index),
                &work_context,
                &event_tx,
            );
            verification_queued[file_index] = true;
            submitted_work_count += 1;
            if verify_started_at.is_none() {
                verify_started_at = Some(Instant::now());
                verify_span = control.profile_span("gutch.snapshot.reconstruct.verify");
            }
        }
    }
    while completed_job_count < jobs.len() || completed_file_count < file_plans.len() {
        if let Err(error) = control.checkpoint() {
            pipeline_error = Some(error);
            break;
        }
        if !downloads_done && let Some(scheduler) = artifact_scheduler.as_ref() {
            loop {
                match scheduler.try_recv() {
                    Ok(ArtifactDownloadEvent::Ready(artifact)) => {
                        let Some(&job_index) = artifact_job_indices.get(&artifact) else {
                            pipeline_error = Some(Error::reconstruct(format!(
                                "downloaded unexpected artifact '{}'",
                                artifact.id()
                            )));
                            break;
                        };
                        let ReconstructJob::Artifact(job) = &jobs[job_index] else {
                            pipeline_error = Some(Error::reconstruct(format!(
                                "downloaded artifact '{}' is not an artifact job",
                                artifact.id()
                            )));
                            break;
                        };
                        if !job.artifact_path.is_file() {
                            pipeline_error = Some(Error::reconstruct(format!(
                                "downloaded artifact '{}' is missing at {}",
                                artifact.id(),
                                job.artifact_path.display()
                            )));
                            break;
                        }
                        if scheduled_job_indices.insert(job_index) {
                            let _dispatch_span = control
                                .profile_span("gutch.snapshot.reconstruct.dispatch.downloaded");
                            spawn_reconstruct_work(
                                pool,
                                ReconstructWorkItem::Job(job_index),
                                &work_context,
                                &event_tx,
                            );
                            submitted_work_count += 1;
                            submitted_job_count += 1;
                            update_reconstruct_backpressure(
                                transfer_progress,
                                submitted_job_count,
                                completed_job_count,
                                runtime_plan.worker_count,
                            );
                        }
                    }
                    Ok(ArtifactDownloadEvent::Failed(error)) => {
                        pipeline_error = Some(error);
                        break;
                    }
                    Ok(ArtifactDownloadEvent::WorkerDone) => {
                        completed_download_worker_count += 1;
                        if completed_download_worker_count == artifact_worker_count {
                            downloads_done = true;
                            if let Some(transfer_progress) = transfer_progress {
                                transfer_progress.finish();
                            }
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        pipeline_error = Some(Error::reconstruct(
                            "artifact download scheduler channel closed unexpectedly",
                        ));
                        break;
                    }
                }
            }
        }

        if pipeline_error.is_some() {
            break;
        }
        if completed_job_count == jobs.len() && completed_file_count == file_plans.len() {
            break;
        }
        match event_rx.recv_timeout(Duration::from_millis(10)) {
            Ok(ReconstructJobEvent::Completed {
                job_index,
                timings: job_timings,
            }) => {
                finished_work_count += 1;
                completed_job_count += 1;
                update_reconstruct_backpressure(
                    transfer_progress,
                    submitted_job_count,
                    completed_job_count,
                    runtime_plan.worker_count,
                );
                accumulate_pipeline_timings(
                    &mut reconstruct_timings,
                    &mut verify_timings,
                    &job_timings,
                );
                emit_progress(
                    control.progress(),
                    GutchProgressEvent {
                        operation: GutchOperation::Fetch,
                        phase: GutchProgressPhase::Reconstruct,
                        message: format!(
                            "Reconstructing jobs ({}/{})",
                            completed_job_count,
                            jobs.len().max(1)
                        ),
                        current: completed_job_count,
                        total: jobs.len().max(1),
                        transferred_bytes: 0,
                        total_bytes: 0,
                        detail: None,
                        done: false,
                    },
                );
                if reconstruct_finished_at.is_none() && completed_job_count == jobs.len() {
                    reconstruct_finished_at = Some(Instant::now());
                }
                for &file_index in &job_file_indices[job_index] {
                    remaining_file_jobs[file_index] =
                        remaining_file_jobs[file_index].saturating_sub(1);
                    if remaining_file_jobs[file_index] == 0 && !verification_queued[file_index] {
                        if verify_started_at.is_none() {
                            verify_started_at = Some(Instant::now());
                            verify_span = control.profile_span("gutch.snapshot.reconstruct.verify");
                        }
                        spawn_reconstruct_work(
                            pool,
                            ReconstructWorkItem::VerifyFile(file_index),
                            &work_context,
                            &event_tx,
                        );
                        verification_queued[file_index] = true;
                        submitted_work_count += 1;
                    }
                }
            }
            Ok(ReconstructJobEvent::Verified(file_verification)) => {
                finished_work_count += 1;
                completed_file_count += 1;
                if verify_finished_at.is_none() && completed_file_count == file_plans.len() {
                    verify_finished_at = Some(Instant::now());
                }
                reconstruct_timings.file_verify += file_verification.elapsed;
                reconstruct_timings.file_verify_bytes = reconstruct_timings
                    .file_verify_bytes
                    .saturating_add(file_verification.bytes);
                verified_file_bytes = verified_file_bytes.saturating_add(file_verification.bytes);
                emit_progress(
                    control.progress(),
                    GutchProgressEvent {
                        operation: GutchOperation::Fetch,
                        phase: GutchProgressPhase::Reconstruct,
                        message: format!(
                            "Reconstructing files ({}/{})",
                            completed_file_count,
                            file_plans.len().max(1)
                        ),
                        current: completed_file_count,
                        total: file_plans.len().max(1),
                        transferred_bytes: 0,
                        total_bytes: 0,
                        detail: None,
                        done: false,
                    },
                );
            }
            Ok(ReconstructJobEvent::Failed(error)) => {
                finished_work_count += 1;
                pipeline_error = Some(error);
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                pipeline_error = Some(Error::reconstruct(
                    "reconstruction job event channel closed unexpectedly",
                ));
                break;
            }
        }
    }
    if pipeline_error.is_some()
        && let Some(scheduler) = artifact_scheduler.as_ref()
    {
        scheduler.cancel();
    }
    if let Some(scheduler) = artifact_scheduler {
        scheduler.join()?;
    }
    while pipeline_error.is_some() && finished_work_count < submitted_work_count {
        match event_rx.recv() {
            Ok(_) => finished_work_count += 1,
            Err(_) => break,
        }
    }
    if let Some(error) = pipeline_error {
        return Err(error);
    }
    if completed_job_count != jobs.len() {
        return Err(Error::reconstruct(format!(
            "reconstruction completed {completed_job_count} of {} jobs",
            jobs.len()
        )));
    }
    if completed_file_count != file_plans.len() {
        return Err(Error::reconstruct(format!(
            "verification completed {completed_file_count} of {} files",
            file_plans.len()
        )));
    }

    let verify_elapsed = verify_started_at
        .zip(verify_finished_at)
        .map(|(started_at, finished_at)| finished_at.saturating_duration_since(started_at))
        .unwrap_or_default();
    drop(verify_span);
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

    reconstruct_timings.reconstruct_wall = reconstruct_finished_at
        .map(|finished_at| finished_at.saturating_duration_since(reconstruct_started_at))
        .unwrap_or_default();
    reconstruct_timings.total = pipeline_started_at.elapsed();
    verify_timings.verify_files_wall = verify_elapsed;
    verify_timings.total = verify_elapsed;
    Ok((reconstruct_timings, verify_timings))
}

fn reconstruct_job_file_indices(job: &ReconstructJob) -> Vec<usize> {
    let mut file_indices = match job {
        ReconstructJob::Artifact(job) => job
            .chunks
            .values()
            .flat_map(|chunk| chunk.targets.iter().map(|target| target.file_index))
            .collect::<Vec<_>>(),
        ReconstructJob::SourceChunk(job) => job
            .targets
            .iter()
            .map(|target| target.file_index)
            .collect::<Vec<_>>(),
        ReconstructJob::CopySourceFile { file_index, .. } => vec![*file_index],
    };
    file_indices.sort_unstable();
    file_indices.dedup();
    file_indices
}

fn update_reconstruct_backpressure(
    transfer_progress: Option<&Arc<FetchTransferProgress>>,
    submitted_job_count: usize,
    completed_job_count: usize,
    worker_count: usize,
) {
    let Some(transfer_progress) = transfer_progress else {
        return;
    };
    let queue_limit = worker_count.saturating_mul(2).max(1);
    let queued_jobs = submitted_job_count.saturating_sub(completed_job_count);
    transfer_progress.set_downstream_backpressure(queued_jobs > queue_limit);
}

fn ensure_empty_reconstruct_output_file(
    file_plan: &ReconstructPipelineFile,
    output_path: &Path,
    control: &GutchOperationControl,
) -> Result<()> {
    let _file_span = control.profile_span_with_args(
        "gutch.snapshot.reconstruct.prepare.file",
        crate::profile::file_trace_args(file_plan.path.clone(), file_plan.size),
    );
    let output = {
        let _create_span = control.profile_span_with_args(
            "gutch.snapshot.reconstruct.prepare.file.create",
            crate::profile::file_trace_args(file_plan.path.clone(), file_plan.size),
        );
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(output_path)?
    };
    {
        let _size_span = control.profile_span_with_args(
            "gutch.snapshot.reconstruct.prepare.file.set_len",
            crate::profile::file_trace_args(file_plan.path.clone(), file_plan.size),
        );
        output.set_len(file_plan.size)?;
    }
    Ok(())
}

fn reconstruct_job(
    job: &ReconstructJob,
    file_plans: &[ReconstructPipelineFile],
    output_paths: &[PathBuf],
    output_handles: &OutputHandleCache,
    compression_profile: &crate::manifest::CompressionProfile,
    control: &GutchOperationControl,
) -> Result<ReconstructTimings> {
    match job {
        ReconstructJob::Artifact(job) => {
            reconstruct_artifact_job(job, output_handles, compression_profile, control)
        }
        ReconstructJob::SourceChunk(job) => {
            reconstruct_source_chunk_job(job, file_plans, output_handles, control)
        }
        ReconstructJob::CopySourceFile {
            file_index,
            source_path,
        } => copy_source_file_job(*file_index, source_path, file_plans, output_paths, control),
    }
}

fn reconstruct_artifact_job(
    job: &ReconstructArtifactJob,
    output_handles: &OutputHandleCache,
    compression_profile: &crate::manifest::CompressionProfile,
    control: &GutchOperationControl,
) -> Result<ReconstructTimings> {
    control.checkpoint()?;
    let artifact_span_name = match &job.artifact {
        crate::ArtifactRef::Bundle { .. } => "gutch.snapshot.reconstruct.bundle",
        crate::ArtifactRef::LooseChunk { .. } => "gutch.snapshot.reconstruct.job.loose_chunk",
    };
    let _artifact_span =
        control.profile_span_with_args(artifact_span_name, reconstruct_artifact_trace_args(job));
    let profiler = control.profiler();
    let compressed_bytes = std::fs::metadata(&job.artifact_path)?.len();
    let mut timings = ReconstructTimings::default();
    let mut target_handles = HashMap::new();

    match &job.artifact {
        crate::ArtifactRef::Bundle { .. } => {
            let mut reader = crate::bundle::BundleReader::open(
                File::open(&job.artifact_path)?,
                compression_profile,
            )?;
            let mut required = vec![false; reader.entries().len()];
            let mut required_chunks = vec![None; reader.entries().len()];
            for (index, entry) in reader.entries().iter().enumerate() {
                if let Some(chunk) = job.chunks.get(&entry.chunk_hash).cloned() {
                    required[index] = true;
                    required_chunks[index] = Some((entry.clone(), chunk));
                }
            }
            let required_count = required.iter().filter(|required| **required).count();
            if required_count != job.chunks.len() {
                return Err(Error::reconstruct(format!(
                    "bundle '{}' is missing {} required chunks",
                    job.artifact.id(),
                    job.chunks.len().saturating_sub(required_count)
                )));
            }
            // Bundle jobs are parallelized by the outer reconstruction pool.
            // Each worker reads one bundle once and streams decoded chunks
            // directly into verification and positional fan-out. At most one
            // required chunk is retained while the next range is decoded.
            let decoded_payload_bytes = reader
                .entries()
                .iter()
                .try_fold(0u64, |total, entry| total.checked_add(entry.raw_size))
                .ok_or_else(|| Error::reconstruct("bundle raw size overflows"))?;
            let _fanout_span = profiler.map(|profiler| {
                profiler.span_with_args(
                    "gutch.snapshot.reconstruct.bundle.fanout_chunks",
                    bundle_fanout_trace_args(job, decoded_payload_bytes),
                )
            });
            let read_metrics = reader.stream_bundle_chunks_with_profiler(
                profiler,
                &required,
                |index, bytes| {
                    control.checkpoint()?;
                    let (entry, chunk) = required_chunks
                        .get(index)
                        .and_then(Option::as_ref)
                        .ok_or_else(|| {
                            Error::reconstruct(format!(
                                "bundle streamed unexpected unselected chunk {}",
                                index
                            ))
                        })?;
                    if chunk.raw_size != entry.raw_size
                        || chunk.compressed_size != entry.compressed_size
                    {
                        return Err(Error::reconstruct(format!(
                            "bundle chunk '{}' size mismatch in '{}': expected raw {}, compressed {}; table has raw {}, compressed {}",
                            entry.chunk_hash,
                            job.artifact.id(),
                            chunk.raw_size,
                            chunk.compressed_size,
                            entry.raw_size,
                            entry.compressed_size
                        )));
                    }
                    let hash_started_at = Instant::now();
                    let actual_hash =
                        crate::hash::ChunkHash::from_blake3(crate::hash::hash_chunk(bytes));
                    timings.chunk_verify += hash_started_at.elapsed();
                    timings.chunk_verify_bytes = timings
                        .chunk_verify_bytes
                        .saturating_add(bytes.len() as u64);
                    if actual_hash.to_string() != chunk.chunk_hash {
                        return Err(Error::HashMismatch {
                            expected: chunk.chunk_hash.clone(),
                            actual: actual_hash.to_string(),
                        });
                    }
                    write_reconstruct_targets(
                        chunk,
                        bytes,
                        output_handles,
                        &mut target_handles,
                        control,
                        &mut timings,
                    )?;
                    Ok(())
                },
            )?;
            let artifact_read_elapsed = read_metrics.read + read_metrics.decompress;
            timings.artifact_read = artifact_read_elapsed;
            timings.artifact_read_compressed_bytes = read_metrics.compressed_bytes;
            timings.artifact_read_decompressed_bytes = read_metrics.raw_bytes;
            timings.bundle_read = artifact_read_elapsed;
            drop(_fanout_span);
            timings.artifact_read_compressed_bytes = compressed_bytes;
            timings.bundle_read_compressed_bytes = compressed_bytes;
            timings.bundle_read_decompressed_bytes = timings.artifact_read_decompressed_bytes;
        }
        crate::ArtifactRef::LooseChunk { hash } => {
            if job.chunks.len() != 1 || !job.chunks.contains_key(hash) {
                return Err(Error::reconstruct(format!(
                    "loose artifact '{}' does not contain exactly its requested chunk",
                    hash
                )));
            }
            let chunk = job.chunks.get(hash).expect("validated loose chunk entry");
            if compressed_bytes != chunk.compressed_size {
                return Err(Error::reconstruct(format!(
                    "loose artifact '{}' has compressed size {}, expected {}",
                    hash, compressed_bytes, chunk.compressed_size
                )));
            }
            let (bytes, read_metrics) = crate::artifact::store::read_loose_chunk_with_metrics(
                &job.artifact_path,
                hash,
                chunk.raw_size,
                compression_profile,
                profiler,
            )?;
            timings.artifact_read = read_metrics.read
                + read_metrics.decompress
                + read_metrics.size_verify
                + read_metrics.hash_verify;
            timings.artifact_read_compressed_bytes = read_metrics.compressed_bytes;
            timings.artifact_read_decompressed_bytes = read_metrics.raw_bytes;
            timings.chunk_verify = read_metrics.hash_verify;
            timings.chunk_verify_bytes = read_metrics.raw_bytes;
            write_reconstruct_targets(
                chunk,
                &bytes,
                output_handles,
                &mut target_handles,
                control,
                &mut timings,
            )?;
        }
    }
    Ok(timings)
}

fn reconstruct_source_chunk_job(
    job: &ReconstructSourceChunkJob,
    file_plans: &[ReconstructPipelineFile],
    output_handles: &OutputHandleCache,
    control: &GutchOperationControl,
) -> Result<ReconstructTimings> {
    control.checkpoint()?;
    let _source_span = control.profile_span_with_args(
        "gutch.snapshot.reconstruct.job.source_chunk",
        source_chunk_trace_args(job),
    );
    let mut input = {
        let _open_span = control.profile_span("gutch.snapshot.reconstruct.source_chunk.open");
        File::open(&job.source_path)?
    };
    let mut bytes = vec![
        0u8;
        usize::try_from(job.raw_size).map_err(|_| {
            Error::reconstruct(format!(
                "source chunk '{}' is too large for this platform",
                job.chunk_hash
            ))
        })?
    ];
    let read_started_at = Instant::now();
    {
        let _read_span = control.profile_span_with_args(
            "gutch.snapshot.reconstruct.source_chunk.read",
            source_chunk_trace_args(job),
        );
        input.seek(SeekFrom::Start(job.source_offset))?;
        input.read_exact(&mut bytes)?;
    }
    let read_elapsed = read_started_at.elapsed();
    let hash_started_at = Instant::now();
    let actual_hash = {
        let _hash_span = control.profile_span_with_args(
            "gutch.snapshot.reconstruct.source_chunk.hash_verify",
            source_chunk_trace_args(job),
        );
        crate::hash::hash_hex(&crate::hash::hash_chunk(&bytes))
    };
    let hash_elapsed = hash_started_at.elapsed();
    if actual_hash != job.chunk_hash {
        return Err(Error::HashMismatch {
            expected: job.chunk_hash.clone(),
            actual: actual_hash,
        });
    }

    let mut timings = ReconstructTimings {
        artifact_read: read_elapsed,
        artifact_read_decompressed_bytes: bytes.len() as u64,
        chunk_verify: hash_elapsed,
        chunk_verify_bytes: bytes.len() as u64,
        ..ReconstructTimings::default()
    };
    let mut target_handles = HashMap::new();
    let rebuild_started_at = Instant::now();
    for target in &job.targets {
        control.checkpoint()?;
        let _target_span = control.profile_span_with_args(
            "gutch.snapshot.reconstruct.file.source_chunk",
            source_target_trace_args(job, target, &file_plans[target.file_index]),
        );
        let _write_span = control.profile_span_with_args(
            "gutch.snapshot.reconstruct.file.source_chunk.write",
            source_target_trace_args(job, target, &file_plans[target.file_index]),
        );
        let write_started_at = Instant::now();
        let output = cached_target_handle(output_handles, &mut target_handles, target.file_index)?;
        write_file_at(&output, target.file_offset, &bytes)?;
        timings.file_write += write_started_at.elapsed();
        timings.file_write_bytes = timings.file_write_bytes.saturating_add(bytes.len() as u64);
        timings.file_rebuild_bytes = timings
            .file_rebuild_bytes
            .saturating_add(bytes.len() as u64);
    }
    timings.file_rebuild = rebuild_started_at.elapsed();
    Ok(timings)
}

fn copy_source_file_job(
    file_index: usize,
    source_path: &Path,
    file_plans: &[ReconstructPipelineFile],
    output_paths: &[PathBuf],
    control: &GutchOperationControl,
) -> Result<ReconstructTimings> {
    let file_plan = file_plans.get(file_index).ok_or_else(|| {
        Error::reconstruct(format!("source file job references file {file_index}"))
    })?;
    control.checkpoint()?;
    let _source_span = control.profile_span_with_args(
        "gutch.snapshot.reconstruct.job.source_file",
        source_file_trace_args(file_plan, source_path),
    );
    let mut input = {
        let _open_span = control.profile_span_with_args(
            "gutch.snapshot.reconstruct.source_file.open",
            source_file_trace_args(file_plan, source_path),
        );
        File::open(source_path)?
    };
    let mut output = {
        let _open_span = control.profile_span_with_args(
            "gutch.snapshot.reconstruct.source_file.output_open",
            crate::profile::file_trace_args(file_plan.path.clone(), file_plan.size),
        );
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&output_paths[file_index])?
    };
    let mut timings = ReconstructTimings::default();
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut copied = 0u64;
    loop {
        control.checkpoint()?;
        let read = {
            let _read_span = control.profile_span_with_args(
                "gutch.snapshot.reconstruct.source_file.read",
                source_file_trace_args(file_plan, source_path),
            );
            input.read(&mut buffer)?
        };
        if read == 0 {
            break;
        }
        let _write_span = control.profile_span_with_args(
            "gutch.snapshot.reconstruct.file.source_file.write",
            file_write_trace_args(file_plan, read as u64),
        );
        let write_started_at = Instant::now();
        output.write_all(&buffer[..read])?;
        timings.file_write += write_started_at.elapsed();
        timings.file_write_bytes = timings.file_write_bytes.saturating_add(read as u64);
        timings.file_rebuild_bytes = timings.file_rebuild_bytes.saturating_add(read as u64);
        copied = copied.saturating_add(read as u64);
    }
    {
        let _flush_span = control.profile_span_with_args(
            "gutch.snapshot.reconstruct.file.source_file.flush",
            crate::profile::file_trace_args(file_plan.path.clone(), file_plan.size),
        );
        let flush_started_at = Instant::now();
        output.flush()?;
        timings.file_write += flush_started_at.elapsed();
    }
    timings.file_rebuild = timings.file_write;
    if copied != file_plan.size {
        return Err(Error::reconstruct(format!(
            "copied source file '{}' has size {}, expected {}",
            file_plan.path, copied, file_plan.size
        )));
    }
    Ok(timings)
}

fn write_reconstruct_targets(
    chunk: &ReconstructArtifactChunk,
    bytes: &[u8],
    output_handles: &OutputHandleCache,
    target_handles: &mut HashMap<usize, Arc<File>>,
    control: &GutchOperationControl,
    timings: &mut ReconstructTimings,
) -> Result<()> {
    if bytes.len() as u64 != chunk.raw_size {
        return Err(Error::reconstruct(format!(
            "artifact chunk '{}' has size {}, expected {}",
            chunk.chunk_hash,
            bytes.len(),
            chunk.raw_size
        )));
    }
    let rebuild_started_at = Instant::now();
    for target in &chunk.targets {
        control.checkpoint()?;
        let write_started_at = Instant::now();
        let output = cached_target_handle(output_handles, target_handles, target.file_index)?;
        write_file_at(&output, target.file_offset, bytes)?;
        timings.file_write += write_started_at.elapsed();
        timings.file_write_bytes = timings.file_write_bytes.saturating_add(bytes.len() as u64);
        timings.file_rebuild_bytes = timings
            .file_rebuild_bytes
            .saturating_add(bytes.len() as u64);
    }
    timings.file_rebuild += rebuild_started_at.elapsed();
    Ok(())
}

struct OutputHandleCache {
    output_paths: Arc<Vec<PathBuf>>,
    output_sizes: Arc<Vec<u64>>,
    state: Mutex<OutputHandleCacheState>,
}

struct OutputHandleCacheState {
    handles: HashMap<usize, Arc<File>>,
    lru: VecDeque<usize>,
    max_open: usize,
}

const LOCAL_OUTPUT_HANDLE_CACHE_LIMIT: usize = 8;

impl OutputHandleCache {
    fn new(output_paths: Arc<Vec<PathBuf>>, output_sizes: Arc<Vec<u64>>) -> Self {
        Self {
            output_paths,
            output_sizes,
            state: Mutex::new(OutputHandleCacheState {
                handles: HashMap::new(),
                lru: VecDeque::new(),
                max_open: 256,
            }),
        }
    }

    fn get_or_open(&self, file_index: usize) -> Result<Arc<File>> {
        if file_index >= self.output_paths.len() {
            return Err(Error::reconstruct(format!(
                "bundle target references unknown file index {}",
                file_index
            )));
        }
        let file = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| Error::reconstruct("output handle cache is poisoned"))?;
            if let Some(file) = state.handles.get(&file_index).cloned() {
                state.lru.retain(|index| *index != file_index);
                state.lru.push_back(file_index);
                file
            } else {
                while state.handles.len() >= state.max_open {
                    if let Some(oldest) = state.lru.pop_front() {
                        state.handles.remove(&oldest);
                    }
                }
                let file = OpenOptions::new()
                    .write(true)
                    .create(true)
                    .open(&self.output_paths[file_index])?;
                file.set_len(self.output_sizes[file_index])?;
                let file = Arc::new(file);
                state.handles.insert(file_index, Arc::clone(&file));
                state.lru.push_back(file_index);
                file
            }
        };
        Ok(file)
    }
}

fn cached_target_handle(
    output_handles: &OutputHandleCache,
    target_handles: &mut HashMap<usize, Arc<File>>,
    file_index: usize,
) -> Result<Arc<File>> {
    if let Some(file) = target_handles.get(&file_index) {
        return Ok(Arc::clone(file));
    }
    let file = output_handles.get_or_open(file_index)?;
    if target_handles.len() >= LOCAL_OUTPUT_HANDLE_CACHE_LIMIT
        && let Some(oldest_file_index) = target_handles.keys().next().copied()
    {
        target_handles.remove(&oldest_file_index);
    }
    target_handles.insert(file_index, Arc::clone(&file));
    Ok(file)
}

#[cfg(windows)]
fn write_file_at(file: &File, offset: u64, bytes: &[u8]) -> Result<()> {
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
fn write_file_at(file: &File, offset: u64, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::FileExt;

    file.write_all_at(bytes, offset)?;
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn write_file_at(file: &File, offset: u64, bytes: &[u8]) -> Result<()> {
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(bytes)?;
    Ok(())
}

fn reconstruct_artifact_trace_args(
    job: &ReconstructArtifactJob,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let logical_bytes = job
        .chunks
        .values()
        .map(|chunk| chunk.raw_size.saturating_mul(chunk.targets.len() as u64))
        .sum::<u64>();
    std::collections::BTreeMap::from([
        (
            "artifact_kind".to_string(),
            serde_json::json!(match &job.artifact {
                crate::ArtifactRef::Bundle { .. } => "bundle",
                crate::ArtifactRef::LooseChunk { .. } => "loose_chunk",
            }),
        ),
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

fn bundle_fanout_trace_args(
    job: &ReconstructArtifactJob,
    decoded_payload_bytes: u64,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut args = reconstruct_artifact_trace_args(job);
    args.insert(
        "decoded_payload_mib".to_string(),
        serde_json::json!(decoded_payload_bytes as f64 / (1024.0 * 1024.0)),
    );
    args.insert(
        "fanout_chunk_count".to_string(),
        serde_json::json!(job.chunks.len()),
    );
    args
}

fn source_chunk_trace_args(
    job: &ReconstructSourceChunkJob,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    std::collections::BTreeMap::from([
        ("chunk_hash".to_string(), serde_json::json!(job.chunk_hash)),
        (
            "source_path".to_string(),
            serde_json::json!(job.source_path.display().to_string()),
        ),
        (
            "source_offset".to_string(),
            serde_json::json!(job.source_offset),
        ),
        (
            "raw_size_mib".to_string(),
            serde_json::json!(job.raw_size as f64 / (1024.0 * 1024.0)),
        ),
        (
            "target_count".to_string(),
            serde_json::json!(job.targets.len()),
        ),
    ])
}

fn source_target_trace_args(
    job: &ReconstructSourceChunkJob,
    target: &ReconstructTarget,
    file_plan: &ReconstructPipelineFile,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut args = crate::profile::file_trace_args(file_plan.path.clone(), file_plan.size);
    args.insert("chunk_hash".to_string(), serde_json::json!(job.chunk_hash));
    args.insert(
        "source_path".to_string(),
        serde_json::json!(job.source_path.display().to_string()),
    );
    args.insert(
        "source_offset".to_string(),
        serde_json::json!(job.source_offset),
    );
    args.insert(
        "raw_size_mib".to_string(),
        serde_json::json!(job.raw_size as f64 / (1024.0 * 1024.0)),
    );
    args.insert(
        "file_offset".to_string(),
        serde_json::json!(target.file_offset),
    );
    args
}

fn source_file_trace_args(
    file_plan: &ReconstructPipelineFile,
    source_path: &Path,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut args = crate::profile::file_trace_args(file_plan.path.clone(), file_plan.size);
    args.insert(
        "source_path".to_string(),
        serde_json::json!(source_path.display().to_string()),
    );
    args
}

fn file_write_trace_args(
    file_plan: &ReconstructPipelineFile,
    write_size: u64,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut args = crate::profile::file_trace_args(&file_plan.path, file_plan.size);
    args.insert(
        "write_size_mib".to_string(),
        serde_json::json!(write_size as f64 / (1024.0 * 1024.0)),
    );
    args
}

struct ReconstructedFileVerification {
    elapsed: Duration,
    bytes: u64,
}

const PARALLEL_FULL_HASH_MIN_BYTES: u64 = 16 * 1024 * 1024;

fn verify_reconstructed_output_file(
    file_index: usize,
    file_plan: &ReconstructPipelineFile,
    output_paths: &[PathBuf],
    control: &GutchOperationControl,
    reconstruct_worker_count: usize,
) -> Result<ReconstructedFileVerification> {
    control.checkpoint()?;
    let _file_span = control.profile_span_with_args(
        "gutch.snapshot.reconstruct.file",
        reconstruct_file_trace_args(file_plan),
    );
    let hash_mode = if file_plan.size < PARALLEL_FULL_HASH_MIN_BYTES {
        "stream"
    } else if reconstruct_worker_count <= 1 {
        "mmap_rayon"
    } else {
        "mmap"
    };
    let started_at = Instant::now();
    let output_path = &output_paths[file_index];
    let actual_size = std::fs::metadata(output_path)?.len();
    if actual_size != file_plan.size {
        return Err(Error::reconstruct(format!(
            "reconstructed size {} does not match expected {} for {}",
            actual_size, file_plan.size, file_plan.path
        )));
    }
    let _hash_span = control.profile_span_with_args(
        "gutch.snapshot.reconstruct.file.full_hash",
        full_hash_trace_args(file_plan, hash_mode, reconstruct_worker_count),
    );
    let hash_started_at = Instant::now();
    let actual_hash = if hash_mode == "mmap_rayon" {
        let _parallel_hash_span = control.profile_span_with_args(
            "gutch.snapshot.reconstruct.file.full_hash.parallel",
            full_hash_trace_args(file_plan, hash_mode, reconstruct_worker_count),
        );
        let mut hasher = blake3::Hasher::new();
        hasher.update_mmap_rayon(output_path)?;
        crate::hash::hash_hex(&hasher.finalize())
    } else if hash_mode == "mmap" {
        let _mmap_hash_span = control.profile_span_with_args(
            "gutch.snapshot.reconstruct.file.full_hash.mmap",
            full_hash_trace_args(file_plan, hash_mode, reconstruct_worker_count),
        );
        let mut hasher = blake3::Hasher::new();
        hasher.update_mmap(output_path)?;
        crate::hash::hash_hex(&hasher.finalize())
    } else {
        let mut input = File::open(output_path)?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            control.checkpoint()?;
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        crate::hash::hash_hex(&hasher.finalize())
    };
    let hash_elapsed = hash_started_at.elapsed();
    let hash_elapsed_seconds = hash_elapsed.as_secs_f64();
    let mut hash_complete_args =
        full_hash_trace_args(file_plan, hash_mode, reconstruct_worker_count);
    hash_complete_args.insert(
        "hash_elapsed_ms".to_string(),
        serde_json::json!(hash_elapsed_seconds * 1000.0),
    );
    hash_complete_args.insert(
        "hash_speed_mib_s".to_string(),
        serde_json::json!(if hash_elapsed_seconds > 0.0 {
            (file_plan.size as f64 / (1024.0 * 1024.0)) / hash_elapsed_seconds
        } else {
            0.0
        }),
    );
    control.profile_event_with_args(
        "gutch.snapshot.reconstruct.file.full_hash.complete",
        hash_complete_args,
    );
    if actual_hash != file_plan.full_file_hash {
        return Err(Error::HashMismatch {
            expected: file_plan.full_file_hash.clone(),
            actual: actual_hash,
        });
    }
    Ok(ReconstructedFileVerification {
        elapsed: started_at.elapsed(),
        bytes: actual_size,
    })
}

fn full_hash_trace_args(
    file_plan: &ReconstructPipelineFile,
    hash_mode: &str,
    reconstruct_worker_count: usize,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    std::collections::BTreeMap::from([
        (
            "size_mib".to_string(),
            serde_json::json!(file_plan.size as f64 / (1024.0 * 1024.0)),
        ),
        ("hash_mode".to_string(), serde_json::json!(hash_mode)),
        (
            "hash_threshold_mib".to_string(),
            serde_json::json!(PARALLEL_FULL_HASH_MIN_BYTES as f64 / (1024.0 * 1024.0)),
        ),
        (
            "configured_workers".to_string(),
            serde_json::json!(reconstruct_worker_count),
        ),
    ])
}

fn reconstruct_file_trace_args(
    file_plan: &ReconstructPipelineFile,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut args = crate::profile::file_trace_args(&file_plan.path, file_plan.size);
    args.insert(
        "required_distinct_chunks".to_string(),
        serde_json::json!(file_plan.trace_metadata.required_distinct_chunks),
    );
    args.insert(
        "required_distinct_artifacts".to_string(),
        serde_json::json!(file_plan.trace_metadata.required_distinct_artifacts),
    );
    args.insert(
        "required_distinct_bundles".to_string(),
        serde_json::json!(file_plan.trace_metadata.required_distinct_bundles),
    );
    args.insert(
        "required_distinct_loose_chunks".to_string(),
        serde_json::json!(file_plan.trace_metadata.required_distinct_loose_chunks),
    );
    args
}

fn reconstruct_file_trace_metadata(
    reconstruction: &ReconstructPipelineReconstruction,
) -> ReconstructFileTraceMetadata {
    let mut chunk_hashes = HashSet::new();
    let mut artifacts = HashSet::new();
    let mut bundle_artifacts = HashSet::new();
    let mut loose_chunk_artifacts = HashSet::new();
    let ReconstructPipelineReconstruction::ReconstructFromParts { chunks } = reconstruction else {
        return ReconstructFileTraceMetadata::default();
    };

    for chunk in chunks {
        match chunk {
            ReconstructPipelineChunkInput::Source { chunk_hash } => {
                chunk_hashes.insert(chunk_hash.as_str());
            }
            ReconstructPipelineChunkInput::Artifact(chunk) => {
                chunk_hashes.insert(chunk.chunk_hash.as_str());
                artifacts.insert(&chunk.artifact);
                match &chunk.artifact {
                    crate::ArtifactRef::Bundle { .. } => {
                        bundle_artifacts.insert(&chunk.artifact);
                    }
                    crate::ArtifactRef::LooseChunk { .. } => {
                        loose_chunk_artifacts.insert(&chunk.artifact);
                    }
                }
            }
        }
    }

    ReconstructFileTraceMetadata {
        required_distinct_chunks: chunk_hashes.len(),
        required_distinct_artifacts: artifacts.len(),
        required_distinct_bundles: bundle_artifacts.len(),
        required_distinct_loose_chunks: loose_chunk_artifacts.len(),
    }
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
                    raw_size: chunk_ref.raw_size,
                });
            offset = offset.checked_add(chunk_ref.raw_size).ok_or_else(|| {
                Error::manifest(format!(
                    "source file '{}' chunk sizes overflow u64",
                    file_manifest.path
                ))
            })?;
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
        let destination = crate::test_util::temp_path("gutch_pipeline_copy_destination");
        let _ = std::fs::remove_dir_all(&source_root);
        let _ = std::fs::remove_dir_all(&destination);
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
            ReconstructPipelineReconstruction::CopyFromSource
        ));

        apply_reconstruct_pipeline(
            ReconstructPipelineApplyRequest {
                repo_root: &repo_root,
                compression_profile: crate::manifest::CompressionProfile::default(),
                snapshot_manifest: &snapshot_manifest,
                destination_dir: &destination,
                staging_dir: None,
                source_destination_dir: Some(source_root.clone()),
                runtime_hints: &RuntimeResourceHints::default(),
                existing_artifacts: &HashSet::new(),
                pending_bundle_downloads: &[],
                transfer_policy: crate::snapshot_ops::types::TransferConcurrencyPolicy::default(),
                control: &crate::GutchOperationControl::default(),
                download_client: None,
                transfer_progress: None,
            },
            plan,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(destination.join("game.exe")).unwrap(),
            b"game"
        );

        let _ = std::fs::remove_dir_all(&source_root);
        let _ = std::fs::remove_dir_all(&repo_root);
        let _ = std::fs::remove_dir_all(&destination);
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
    fn pipeline_reconstructs_mixed_source_and_loose_chunks_with_shared_executor() {
        let source_root = crate::test_util::temp_path("gutch_pipeline_mixed_loose_source");
        let repo_root = crate::test_util::temp_path("gutch_pipeline_mixed_loose_repo");
        let destination = crate::test_util::temp_path("gutch_pipeline_mixed_loose_destination");
        let _ = std::fs::remove_dir_all(&source_root);
        let _ = std::fs::remove_dir_all(&repo_root);
        let _ = std::fs::remove_dir_all(&destination);
        std::fs::create_dir_all(&source_root).unwrap();
        std::fs::write(source_root.join("game.exe"), b"gameold!").unwrap();
        let repo_paths = crate::GutchRepoPaths::from_repo_root(&repo_root);
        crate::ensure_gutch_repo_layout(&repo_paths, "mixed-loose-test").unwrap();

        let shared_chunk_hash = test_hash(b"game");
        let source_only_chunk_hash = test_hash(b"old!");
        let replacement_chunk_hash = test_hash(b"new!");
        let compression_profile =
            manifest::CompressionProfile::for_preset(manifest::CompressionPreset::Zstd3);
        let replacement_artifact = crate::ArtifactRef::LooseChunk {
            hash: replacement_chunk_hash.clone(),
        };
        let replacement_path = replacement_artifact.local_path(&repo_paths).unwrap();
        std::fs::create_dir_all(replacement_path.parent().unwrap()).unwrap();
        std::fs::write(
            &replacement_path,
            crate::compress::compress_with_level(b"new!", compression_profile.zstd_level).unwrap(),
        )
        .unwrap();

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
            replacement_chunk_hash.clone(),
        );
        let chunk_index = manifest::ChunkIndex {
            format_version: "1".to_string(),
            updated_at: String::new(),
            entries: std::collections::BTreeMap::from([(
                replacement_chunk_hash.clone(),
                manifest::ChunkIndexEntry {
                    bundle_id: replacement_chunk_hash.clone(),
                    raw_size: 4,
                    compressed_size: std::fs::metadata(&replacement_path).unwrap().len(),
                    storage_mode: manifest::ArtifactStorageMode::LooseChunks,
                },
            )]),
        };
        let existing_artifacts = HashSet::from([replacement_artifact]);
        let plan = build_reconstruct_pipeline_plan(
            &snapshot_manifest,
            &chunk_index,
            &repo_paths,
            &existing_artifacts,
            Some(SnapshotReconstructSource {
                snapshot_manifest: &source_manifest,
                destination_dir: &source_root,
            }),
        )
        .unwrap();
        assert_eq!(plan.jobs.len(), 2);
        assert!(
            plan.jobs
                .iter()
                .any(|job| matches!(job, ReconstructJob::SourceChunk(_)))
        );
        assert!(plan.jobs.iter().any(|job| matches!(
            job,
            ReconstructJob::Artifact(ReconstructArtifactJob {
                artifact: crate::ArtifactRef::LooseChunk { .. },
                ..
            })
        )));

        apply_reconstruct_pipeline(
            ReconstructPipelineApplyRequest {
                repo_root: &repo_root,
                compression_profile,
                snapshot_manifest: &snapshot_manifest,
                destination_dir: &destination,
                staging_dir: None,
                source_destination_dir: Some(source_root.clone()),
                runtime_hints: &RuntimeResourceHints { jobs: Some(2) },
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
        assert_eq!(
            std::fs::read(destination.join("game.exe")).unwrap(),
            b"gamenew!"
        );

        let _ = std::fs::remove_dir_all(&source_root);
        let _ = std::fs::remove_dir_all(&repo_root);
        let _ = std::fs::remove_dir_all(&destination);
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
    fn bundled_pipeline_reads_bundle_once_and_writes_shared_chunks_to_ranges() {
        let repo_root = crate::test_util::temp_path("gutch_pipeline_bundled_repo");
        let destination = crate::test_util::temp_path("gutch_pipeline_bundled_destination");
        let _ = std::fs::remove_dir_all(&repo_root);
        let _ = std::fs::remove_dir_all(&destination);
        let repo_paths = crate::GutchRepoPaths::from_repo_root(&repo_root);
        crate::ensure_gutch_repo_layout(&repo_paths, "bundled-test").unwrap();

        let first = b"1111";
        let second = b"2222";
        let third = b"3333";
        let first_hash = test_hash(first);
        let second_hash = test_hash(second);
        let third_hash = test_hash(third);
        let compression_profile = manifest::CompressionProfile::default();
        let mut builder = crate::bundle::BundleBuilder::new(compression_profile.clone());
        builder.add_chunk(
            crate::hash::ChunkHash::from_blake3(crate::hash::hash_chunk(first)),
            first.to_vec(),
        );
        builder.add_chunk(
            crate::hash::ChunkHash::from_blake3(crate::hash::hash_chunk(second)),
            second.to_vec(),
        );
        builder.add_chunk(
            crate::hash::ChunkHash::from_blake3(crate::hash::hash_chunk(third)),
            third.to_vec(),
        );
        let (bundle_id, bundle_bytes) = builder.build_with_id().unwrap();
        let bundle_reader = crate::bundle::BundleReader::open(
            std::io::Cursor::new(bundle_bytes.as_slice()),
            &compression_profile,
        )
        .unwrap();
        let compressed_sizes = bundle_reader
            .entries()
            .iter()
            .map(|entry| (entry.chunk_hash.clone(), entry.compressed_size))
            .collect::<HashMap<_, _>>();
        let bundle = crate::ArtifactRef::Bundle {
            id: bundle_id.clone(),
        };
        let bundle_path = bundle.local_path(&repo_paths).unwrap();
        std::fs::write(bundle_path, bundle_bytes).unwrap();

        let mut snapshot_manifest =
            SnapshotManifest::new("bundled-snapshot".to_string(), SnapshotTagSet::default());
        snapshot_manifest.push_file(FileManifest {
            path: "first.bin".to_string(),
            size: 8,
            executable: false,
            read_only: false,
            full_file_hash: test_hash(b"11112222"),
            chunks: vec![
                ChunkRef {
                    chunk_hash: first_hash.clone(),
                    raw_size: 4,
                },
                ChunkRef {
                    chunk_hash: second_hash.clone(),
                    raw_size: 4,
                },
            ],
        });
        snapshot_manifest.push_file(FileManifest {
            path: "second.bin".to_string(),
            size: 8,
            executable: false,
            read_only: false,
            full_file_hash: test_hash(b"33331111"),
            chunks: vec![
                ChunkRef {
                    chunk_hash: third_hash.clone(),
                    raw_size: 4,
                },
                ChunkRef {
                    chunk_hash: first_hash.clone(),
                    raw_size: 4,
                },
            ],
        });
        let chunk_index = manifest::ChunkIndex {
            format_version: "1".to_string(),
            updated_at: String::new(),
            entries: std::collections::BTreeMap::from([
                (
                    first_hash,
                    manifest::ChunkIndexEntry {
                        bundle_id: bundle_id.clone(),
                        raw_size: 4,
                        compressed_size: compressed_sizes[&test_hash(first)],
                        storage_mode: manifest::ArtifactStorageMode::Bundled,
                    },
                ),
                (
                    second_hash,
                    manifest::ChunkIndexEntry {
                        bundle_id: bundle_id.clone(),
                        raw_size: 4,
                        compressed_size: compressed_sizes[&test_hash(second)],
                        storage_mode: manifest::ArtifactStorageMode::Bundled,
                    },
                ),
                (
                    third_hash,
                    manifest::ChunkIndexEntry {
                        bundle_id,
                        raw_size: 4,
                        compressed_size: compressed_sizes[&test_hash(third)],
                        storage_mode: manifest::ArtifactStorageMode::Bundled,
                    },
                ),
            ]),
        };
        let existing_artifacts = HashSet::from([bundle]);
        let plan = build_reconstruct_pipeline_plan(
            &snapshot_manifest,
            &chunk_index,
            &repo_paths,
            &existing_artifacts,
            None,
        )
        .unwrap();
        assert_eq!(plan.jobs.len(), 1);
        assert!(matches!(
            &plan.jobs[0],
            ReconstructJob::Artifact(job)
                if matches!(job.artifact, crate::ArtifactRef::Bundle { .. })
                    && job.chunks.len() == 3
        ));

        apply_reconstruct_pipeline(
            ReconstructPipelineApplyRequest {
                repo_root: &repo_root,
                compression_profile,
                snapshot_manifest: &snapshot_manifest,
                destination_dir: &destination,
                staging_dir: None,
                source_destination_dir: None,
                runtime_hints: &RuntimeResourceHints { jobs: Some(2) },
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

        assert_eq!(
            std::fs::read(destination.join("first.bin")).unwrap(),
            b"11112222"
        );
        assert_eq!(
            std::fs::read(destination.join("second.bin")).unwrap(),
            b"33331111"
        );
        let _ = std::fs::remove_dir_all(&repo_root);
        let _ = std::fs::remove_dir_all(&destination);
    }
}
