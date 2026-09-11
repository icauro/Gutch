use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::OpenOptions;
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
use crate::runtime::{RuntimeResourceHints, plan_runtime};
use crate::{Error, Result};

#[path = "reconstruct_io.rs"]
mod reconstruct_io;
#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;

use reconstruct_io::{
    OutputHandleCache, ReconstructedFileVerification, SourceHandleCache,
    build_source_chunk_locations, reconstruct_file_trace_metadata, reconstruct_job,
    verify_reconstructed_output_file,
};

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
    full_file_hash: crate::ChunkHash,
    trace_metadata: ReconstructFileTraceMetadata,
    #[cfg(test)]
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
    chunks: Vec<ReconstructArtifactChunk>,
    targets: Vec<ReconstructTarget>,
}

#[derive(Clone)]
struct ReconstructArtifactChunk {
    chunk_hash: crate::ChunkHash,
    raw_size: u32,
    compressed_size: u32,
    targets: std::ops::Range<usize>,
}

#[derive(Clone)]
struct ReconstructSourceChunkJob {
    chunk_hash: String,
    raw_size: u32,
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
        let _validate_span = control.profile_span("snapshot.reconstruct.prepare.entries.validate");
        crate::manifest::validate_snapshot_entries(&snapshot_manifest.entries)?;
    }
    {
        let _root_span = control.profile_span("snapshot.reconstruct.prepare.entries.create_root");
        crate::helpers::create_dir_all(output_dir)?;
    }
    let path_resolver = {
        let _resolver_span =
            control.profile_span("snapshot.reconstruct.prepare.entries.path_resolver");
        crate::path_guard::ManifestPathResolver::new(output_dir)?
    };
    {
        let _directories_span =
            control.profile_span("snapshot.reconstruct.prepare.entries.directories");
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
        "snapshot.reconstruct.prepare.outputs.resolve_paths",
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
                "snapshot.reconstruct.prepare.entries.directory",
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
            "snapshot.reconstruct.finalize.entries.symlinks",
            finalize_phase_trace_args(
                symlinks.len(),
                symlinks.len(),
                metadata_worker_count,
                should_parallelize_finalize(symlinks.len(), metadata_worker_count),
            ),
        );
        run_finalize_entries(&symlinks, metadata_worker_count, reconstruct_pool, |link| {
            let _entry_span = control.profile_span_with_args(
                "snapshot.reconstruct.finalize.entries.symlink",
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
            "snapshot.reconstruct.finalize.entries.files",
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
                    "snapshot.reconstruct.finalize.entries.file",
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
            "snapshot.reconstruct.finalize.entries.directories",
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
                    "snapshot.reconstruct.finalize.entries.directory",
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
    let mut chunk_positions: HashMap<(usize, crate::ChunkHash), usize> = HashMap::new();
    let mut chunk_targets: HashMap<(usize, usize), Vec<ReconstructTarget>> = HashMap::new();
    let mut artifact_job_indices: HashMap<crate::ArtifactRef, usize> = HashMap::new();
    let mut source_chunk_job_indices: HashMap<(PathBuf, u64, u32, String), usize> = HashMap::new();
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
                full_file_hash: crate::ChunkHash::parse_hex(&file_manifest.full_file_hash)?,
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
                    chunks: Vec::new(),
                    targets: Vec::new(),
                }));
                artifact_job_indices.insert(artifact.clone(), job_index);
                job_index
            };
            let ReconstructJob::Artifact(artifact_job) = &mut jobs[job_index] else {
                unreachable!("artifact job index points to another job kind");
            };
            let hash = crate::ChunkHash::parse_hex(&chunk_ref.chunk_hash)?;
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
            full_file_hash: crate::ChunkHash::parse_hex(&file_manifest.full_file_hash)?,
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

pub(super) fn apply_reconstruct_pipeline(
    request: ReconstructPipelineApplyRequest<'_>,
    pipeline_plan: ReconstructPipelinePlan,
) -> Result<SnapshotReconstructFromRepoResult> {
    request.control.checkpoint()?;
    let operation_started_at = Instant::now();
    let _operation_span = request.control.profile_span("snapshot.reconstruct.apply");
    let prepare_started_at = Instant::now();
    let reconstruct_worker_count = plan_runtime(request.runtime_hints).worker_count;
    let reconstruct_pool = if request.snapshot_manifest.entries.is_empty() {
        None
    } else {
        let _pool_span = request
            .control
            .profile_span("snapshot.reconstruct.prepare.pool");
        Some(build_reconstruct_pool(reconstruct_worker_count)?)
    };
    let staging = {
        let _staging_span = request
            .control
            .profile_span("snapshot.reconstruct.prepare.staging");
        let staging_dir = request
            .staging_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(|| {
                super::apply::default_snapshot_staging_dir(
                    request.destination_dir,
                    &request.snapshot_manifest.snapshot_id,
                )
            });
        super::apply::create_snapshot_staging(
            request.destination_dir,
            &staging_dir,
            request.repo_root,
            request.source_destination_dir.as_deref(),
        )?
    };
    let staging_dir = staging.path().to_path_buf();

    let main_result = (|| -> Result<SnapshotReconstructFromRepoResult> {
        let output_paths = {
            let _prepare_span = request
                .control
                .profile_span("snapshot.reconstruct.prepare.entries");
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
                .profile_span("snapshot.reconstruct.finalize.entries");
            let metadata_worker_count = plan_runtime(request.runtime_hints).worker_count;
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
                .profile_span("snapshot.reconstruct.finalize.promote");
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
    memory: crate::runtime::MemoryBudget,
    source_handles: SourceHandleCache,
    jobs: Arc<Vec<ReconstructJob>>,
    file_plans: Arc<Vec<ReconstructPipelineFile>>,
    output_paths: Arc<Vec<PathBuf>>,
    precomputed_file_hashes: Arc<Vec<Mutex<Option<crate::ChunkHash>>>>,
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
            ReconstructWorkItem::Job(job_index) => {
                let working_bytes = match &context.jobs[job_index] {
                    ReconstructJob::Artifact(_) => 136 * 1024 * 1024,
                    ReconstructJob::SourceChunk(chunk) => u64::from(chunk.raw_size) + 1024 * 1024,
                    ReconstructJob::CopySourceFile { .. } => 1024 * 1024,
                };
                let _memory = context.memory.acquire(working_bytes, &context.control)?;
                reconstruct_job(
                    &context.jobs[job_index],
                    &context.file_plans,
                    &context.output_paths,
                    &context.precomputed_file_hashes,
                    &context.output_handles,
                    &context.source_handles,
                    &context.compression_profile,
                    &context.control,
                )
                .map(|timings| ReconstructJobEvent::Completed { job_index, timings })
            }
            ReconstructWorkItem::VerifyFile(file_index) => verify_reconstructed_output_file(
                file_index,
                &context.file_plans[file_index],
                &context.output_paths,
                &context.precomputed_file_hashes,
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
    let _pipeline_span = control.profile_span("snapshot.reconstruct.pipeline");
    let pipeline_started_at = Instant::now();
    let runtime_plan = plan_runtime(runtime_hints);
    let mut reconstruct_timings = ReconstructTimings {
        runtime_plan: Some(runtime_plan.clone()),
        verify_files: true,
        ..ReconstructTimings::default()
    };
    let mut verify_timings = crate::verify::VerifyTimings {
        verify_mode: crate::verify::VerifyMode::Fast,
        runtime_plan: Some(plan_runtime(runtime_hints)),
        ..crate::verify::VerifyTimings::default()
    };

    let artifact_priorities = {
        let _priorities_span = control.profile_span("snapshot.reconstruct.prepare.priorities");
        build_artifact_priorities(&pipeline_plan, pending_bundle_downloads)
    };
    let file_plans = Arc::new(pipeline_plan.files);
    let output_sizes = Arc::new(file_plans.iter().map(|file| file.size).collect::<Vec<_>>());
    let output_handles = Arc::new(OutputHandleCache::new(
        Arc::clone(&output_paths),
        Arc::clone(&output_sizes),
    ));
    let precomputed_file_hashes = Arc::new(
        (0..file_plans.len())
            .map(|_| Mutex::new(None))
            .collect::<Vec<Mutex<Option<crate::ChunkHash>>>>(),
    );
    let jobs = Arc::new(pipeline_plan.jobs);
    let (artifact_job_indices, artifact_scheduler) = {
        let _artifacts_span = control.profile_span("snapshot.reconstruct.prepare.artifacts");
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
        memory: crate::runtime::MemoryBudget::new(),
        source_handles: SourceHandleCache::default(),
        jobs: Arc::clone(&jobs),
        file_plans: Arc::clone(&file_plans),
        output_paths: Arc::clone(&output_paths),
        precomputed_file_hashes,
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
        let _dispatch_span = control.profile_span("snapshot.reconstruct.dispatch.ready");
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
                verify_span = control.profile_span("snapshot.reconstruct.verify");
            }
        }
    }
    while completed_job_count < jobs.len() || completed_file_count < file_plans.len() {
        if let Err(error) = control.checkpoint() {
            pipeline_error = Some(error);
            break;
        }
        if !downloads_done && let Some(scheduler) = artifact_scheduler.as_ref() {
            while !downloads_done {
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
                            let _dispatch_span =
                                control.profile_span("snapshot.reconstruct.dispatch.downloaded");
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
                        pipeline_error = Some(Error::reconstruct(format!(
                            "artifact download scheduler channel closed unexpectedly after scheduling {}/{} artifact jobs; {completed_download_worker_count}/{artifact_worker_count} workers reported completion",
                            scheduled_job_indices.len(),
                            artifact_job_indices.len(),
                        )));
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
                            verify_span = control.profile_span("snapshot.reconstruct.verify");
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
            .iter()
            .flat_map(|chunk| {
                job.targets[chunk.targets.clone()]
                    .iter()
                    .map(|target| target.file_index)
            })
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
        "snapshot.reconstruct.prepare.file",
        crate::profile::file_trace_args(file_plan.path.clone(), file_plan.size),
    );
    let output = {
        let _create_span = control.profile_span_with_args(
            "snapshot.reconstruct.prepare.file.create",
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
            "snapshot.reconstruct.prepare.file.set_len",
            crate::profile::file_trace_args(file_plan.path.clone(), file_plan.size),
        );
        output.set_len(file_plan.size)?;
    }
    Ok(())
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
