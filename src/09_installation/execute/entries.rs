//! Creates directories and symlinks before writing and finalizes entry attributes after.

use crate::error::IoContext;
use crate::profiling::Arguments;
use crate::profiling::trace_args;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use rayon::ThreadPool;
use rayon::prelude::*;

use crate::Error;
use crate::Result;
use crate::filesystem;
use crate::filesystem::paths::ManifestPathResolver;
use crate::filesystem::set_read_only;
use crate::manifest::DirectoryManifest;
use crate::manifest::FileManifest;
use crate::manifest::SnapshotEntry;
use crate::manifest::SymlinkManifest;
use crate::manifest::snapshot::SnapshotManifest;
use crate::manifest::validate_snapshot_entries;
use crate::profiling::file_trace_args;
use crate::progress::OperationControl;
use std::io::ErrorKind;

/// Creates the directories of `snapshot_manifest` and resolves its file paths.
/// `complete_manifest` is the whole snapshot, which is validated instead: a
/// repair subset may hold a link without the intact target it points at.
pub(crate) fn prepare_snapshot_entries(
    snapshot_manifest: &SnapshotManifest,
    complete_manifest: &SnapshotManifest,
    output_dir: &Path,
    control: &OperationControl,
    reconstruct_pool: Option<&ThreadPool>,
) -> Result<Arc<Vec<PathBuf>>> {
    {
        let _validate_span =
            control.profile_span("installation.reconstruct.prepare.entries.validate");
        validate_snapshot_entries(&complete_manifest.entries)?;
    }
    {
        let _root_span =
            control.profile_span("installation.reconstruct.prepare.entries.create_root");
        filesystem::create_dir_all(output_dir)?;
    }
    let path_resolver = {
        let _resolver_span =
            control.profile_span("installation.reconstruct.prepare.entries.path_resolver");
        ManifestPathResolver::new(output_dir)?
    };
    {
        let _directories_span =
            control.profile_span("installation.reconstruct.prepare.entries.directories");
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
    control: &OperationControl,
    path_resolver: &ManifestPathResolver,
    reconstruct_pool: Option<&ThreadPool>,
) -> Result<Vec<PathBuf>> {
    let files = snapshot_manifest.files().collect::<Vec<_>>();
    let _resolve_paths_span = control.profile_span_with_args(
        "installation.reconstruct.prepare.outputs.resolve_paths",
        || trace_args!("path_count" => files.len()),
    );
    // Bound trace volume while keeping enough batches for workers to share the work.
    // Indexed batch collection preserves manifest file order and error ordering.
    let resolve_batch = |batch: &[&FileManifest]| {
        let _batch_span = control.profile_span_with_args(
            "installation.worker.resolve_output_paths",
            || trace_args!("path_count" => batch.len()),
        );
        batch
            .iter()
            .map(|file| path_resolver.resolve(&file.path))
            .collect::<Vec<_>>()
    };
    let batches = if let Some(reconstruct_pool) = reconstruct_pool {
        reconstruct_pool.install(|| files.par_chunks(64).map(resolve_batch).collect::<Vec<_>>())
    } else {
        files.chunks(64).map(resolve_batch).collect::<Vec<_>>()
    };
    batches.into_iter().flatten().collect()
}

fn create_snapshot_directories(
    snapshot_manifest: &SnapshotManifest,
    control: &OperationControl,
    path_resolver: &ManifestPathResolver,
    reconstruct_pool: Option<&ThreadPool>,
) -> Result<()> {
    let mut directories = BTreeMap::<String, Option<bool>>::new();
    for entry in &snapshot_manifest.entries {
        if let SnapshotEntry::Directory(directory) = entry {
            add_manifest_directory_prefixes(&mut directories, &directory.path);
            directories.insert(directory.path.clone(), Some(directory.read_only));
        }
    }
    for entry in &snapshot_manifest.entries {
        // Files and symlinks may rely on implicit parent directories.
        if matches!(entry, SnapshotEntry::Directory(_)) {
            continue;
        }
        let parent = entry.path().rsplit_once('/').map(|(parent, _)| parent);
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
                "installation.reconstruct.prepare.entries.directory",
                || prepare_directory_trace_args(path, *read_only),
            );
            let resolved_path = path_resolver.resolve(path)?;
            create_snapshot_directory(&resolved_path)
        };
        let pool = reconstruct_pool.filter(|_| directories.len() >= 2);
        try_for_each_in(pool, directories, create_directory)?;
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
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            if std::fs::metadata(path)
                .io_context("checking output directory", path)?
                .is_dir()
            {
                Ok(())
            } else {
                Err(Error::io_context("creating output directory", path, error))
            }
        }
        Err(error) => Err(Error::io_context("creating output directory", path, error)),
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

/// Explicit directory entries plus every implicit parent directory.
#[cfg(windows)]
fn directory_paths(manifest: &SnapshotManifest) -> std::collections::HashSet<&str> {
    let mut directories = std::collections::HashSet::new();
    for entry in &manifest.entries {
        if matches!(entry, SnapshotEntry::Directory(_)) {
            directories.insert(entry.path());
        }
        for (offset, _) in entry.path().match_indices('/') {
            directories.insert(&entry.path()[..offset]);
        }
    }
    directories
}

/// Applies links and attributes for `snapshot_manifest`. `complete_manifest` is
/// the whole snapshot, which may be larger during repair; Windows needs it to
/// tell whether a link points at a directory that is not itself being repaired.
pub(crate) fn finalize_snapshot_entries(
    snapshot_manifest: &SnapshotManifest,
    complete_manifest: &SnapshotManifest,
    output_dir: &Path,
    control: &OperationControl,
    metadata_worker_count: usize,
    reconstruct_pool: Option<&ThreadPool>,
) -> Result<()> {
    let path_resolver = ManifestPathResolver::new(output_dir)?;
    #[cfg(windows)]
    let directories = directory_paths(complete_manifest);
    #[cfg(not(windows))]
    let _ = complete_manifest;
    let symlinks = snapshot_manifest
        .entries
        .iter()
        .filter_map(|entry| match entry {
            SnapshotEntry::Symlink(link) => Some(link),
            _ => None,
        })
        .collect::<Vec<_>>();
    {
        let _symlink_phase = control.profile_span_with_args(
            "installation.reconstruct.finalize.entries.symlinks",
            || {
                finalize_phase_trace_args(
                    symlinks.len(),
                    symlinks.len(),
                    metadata_worker_count,
                    should_parallelize_finalize(symlinks.len(), metadata_worker_count),
                )
            },
        );
        run_finalize_entries(&symlinks, metadata_worker_count, reconstruct_pool, |link| {
            let _entry_span = control.profile_span_with_args(
                "installation.reconstruct.finalize.entries.symlink",
                || finalize_symlink_trace_args(link),
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
                let result = if directories.contains(link.target_path.as_str()) {
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
            "installation.reconstruct.finalize.entries.files",
            || {
                finalize_phase_trace_args(
                    file_count,
                    files_needing_metadata.len(),
                    metadata_worker_count,
                    should_parallelize_finalize(
                        files_needing_metadata.len(),
                        metadata_worker_count,
                    ),
                )
            },
        );
        run_finalize_entries(
            &files_needing_metadata,
            metadata_worker_count,
            reconstruct_pool,
            |file| {
                let _entry_span = control.profile_span_with_args(
                    "installation.reconstruct.finalize.entries.file",
                    || finalize_file_trace_args(file),
                );
                let path = path_resolver.resolve(&file.path)?;
                apply_entry_permissions(&path, file.read_only, Some(file.executable))?;
                Ok(())
            },
        )?;
    }

    let directory_count = snapshot_manifest
        .entries
        .iter()
        .filter(|entry| matches!(entry, SnapshotEntry::Directory(_)))
        .count();
    let mut directories_by_depth = BTreeMap::<usize, Vec<_>>::new();
    for directory in snapshot_manifest
        .entries
        .iter()
        .filter_map(|entry| match entry {
            SnapshotEntry::Directory(directory) if directory.read_only => Some(directory),
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
            "installation.reconstruct.finalize.entries.directories",
            || {
                finalize_phase_trace_args(
                    directory_count,
                    directories_needing_metadata,
                    metadata_worker_count,
                    directories_parallel,
                )
            },
        );
        run_finalize_directory_levels(
            &directories_by_depth,
            metadata_worker_count,
            reconstruct_pool,
            |directory| {
                let _entry_span = control.profile_span_with_args(
                    "installation.reconstruct.finalize.entries.directory",
                    || finalize_directory_trace_args(directory),
                );
                let path = path_resolver.resolve(&directory.path)?;
                let mut permissions = std::fs::metadata(&path)
                    .io_context("reading directory permissions", &path)?
                    .permissions();
                if permissions.readonly() != directory.read_only {
                    set_read_only(&mut permissions, directory.read_only);
                    fs::set_permissions(&path, permissions)
                        .io_context("setting entry permissions", &path)?;
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
    let pool =
        reconstruct_pool.filter(|_| should_parallelize_finalize(entries.len(), worker_count));
    try_for_each_in(pool, entries, work)
}

fn run_finalize_directory_levels<F>(
    levels: &BTreeMap<usize, Vec<&DirectoryManifest>>,
    worker_count: usize,
    reconstruct_pool: Option<&ThreadPool>,
    work: F,
) -> Result<()>
where
    F: Fn(&DirectoryManifest) -> Result<()> + Send + Sync,
{
    for directories in levels.values().rev() {
        let pool = reconstruct_pool
            .filter(|_| should_parallelize_finalize(directories.len(), worker_count));
        try_for_each_in(pool, directories, |directory| work(directory))?;
    }
    Ok(())
}

/// Run `work` over `items` on `pool`, or in order on this thread without one.
fn try_for_each_in<T, F>(pool: Option<&ThreadPool>, items: &[T], work: F) -> Result<()>
where
    T: Sync,
    F: Fn(&T) -> Result<()> + Send + Sync,
{
    match pool {
        Some(pool) => pool.install(|| items.par_iter().try_for_each(work)),
        None => items.iter().try_for_each(work),
    }
}

fn should_parallelize_finalize(entry_count: usize, worker_count: usize) -> bool {
    entry_count >= FINALIZE_ENTRIES_PARALLEL_MIN && worker_count >= 2
}

fn finalize_phase_trace_args(
    total_entries: usize,
    changed_entries: usize,
    worker_count: usize,
    parallel: bool,
) -> Arguments {
    trace_args!(
        "total_entries" => total_entries,
        "changed_entries" => changed_entries,
        "parallel" => parallel,
        "parallel_workers" => if parallel { worker_count } else { 1 },
    )
}

fn prepare_directory_trace_args(path: &str, read_only: Option<bool>) -> Arguments {
    trace_args!(
        "path" => path,
        "depth" => path.matches('/').count(),
        "read_only" => read_only,
        "implicit" => read_only.is_none(),
    )
}

fn finalize_symlink_trace_args(link: &SymlinkManifest) -> Arguments {
    trace_args!("path" => link.path, "target_path" => link.target_path)
}

fn finalize_file_trace_args(file: &FileManifest) -> Arguments {
    let mut args = file_trace_args(&file.path, file.size);
    args.insert("read_only".to_string(), serde_json::json!(file.read_only));
    args.insert("executable".to_string(), serde_json::json!(file.executable));
    args
}

fn finalize_directory_trace_args(directory: &DirectoryManifest) -> Arguments {
    trace_args!("path" => directory.path, "read_only" => directory.read_only)
}

/// Apply the metadata represented by the manifest, including clearing old flags.
pub(crate) fn apply_entry_permissions(
    path: &Path,
    read_only: bool,
    executable: Option<bool>,
) -> Result<()> {
    let mut permissions = std::fs::symlink_metadata(path)
        .io_context("reading entry permissions", path)?
        .permissions();
    let original = permissions.clone();
    #[cfg(unix)]
    if let Some(executable) = executable {
        use std::os::unix::fs::PermissionsExt;
        let mode = permissions.mode();
        permissions.set_mode(if executable {
            mode | 0o111
        } else {
            mode & !0o111
        });
    }
    #[cfg(not(unix))]
    let _ = executable;
    if permissions.readonly() != read_only {
        set_read_only(&mut permissions, read_only);
    }
    if permissions != original {
        fs::set_permissions(path, permissions).io_context("setting entry permissions", path)?;
    }
    Ok(())
}
