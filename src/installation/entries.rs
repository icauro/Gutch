use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use rayon::ThreadPool;
use rayon::prelude::*;

use crate::Error;
use crate::Result;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::GutchOperationControl;

pub(super) fn prepare_snapshot_entries(
    snapshot_manifest: &SnapshotManifest,
    output_dir: &Path,
    control: &GutchOperationControl,
    reconstruct_pool: Option<&ThreadPool>,
) -> Result<Arc<Vec<PathBuf>>> {
    {
        let _validate_span =
            control.profile_span("installation.reconstruct.prepare.entries.validate");
        crate::manifest::validate_snapshot_entries(&snapshot_manifest.entries)?;
    }
    {
        let _root_span =
            control.profile_span("installation.reconstruct.prepare.entries.create_root");
        crate::filesystem::create_dir_all(output_dir)?;
    }
    let path_resolver = {
        let _resolver_span =
            control.profile_span("installation.reconstruct.prepare.entries.path_resolver");
        crate::filesystem::paths::ManifestPathResolver::new(output_dir)?
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
    control: &GutchOperationControl,
    path_resolver: &crate::filesystem::paths::ManifestPathResolver,
    reconstruct_pool: Option<&ThreadPool>,
) -> Result<Vec<PathBuf>> {
    let files = snapshot_manifest.files().collect::<Vec<_>>();
    let _resolve_paths_span = control.profile_span_with_args(
        "installation.reconstruct.prepare.outputs.resolve_paths",
        || {
            std::collections::BTreeMap::from([(
                "path_count".to_string(),
                serde_json::json!(files.len()),
            )])
        },
    );
    // Bound trace volume while keeping enough batches for workers to share the work.
    // Indexed batch collection preserves manifest file order and error ordering.
    let resolve_batch = |batch: &[&crate::manifest::FileManifest]| {
        let _batch_span = control
            .profile_span_with_args("installation.worker.resolve_output_paths", || {
                BTreeMap::from([("path_count".into(), serde_json::json!(batch.len()))])
            });
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
    control: &GutchOperationControl,
    path_resolver: &crate::filesystem::paths::ManifestPathResolver,
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
                "installation.reconstruct.prepare.entries.directory",
                || prepare_directory_trace_args(path, *read_only),
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
            if std::fs::metadata(path)
                .map_err(|e| Error::io_context("checking output directory", path, e))?
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

pub(super) fn finalize_snapshot_entries(
    snapshot_manifest: &SnapshotManifest,
    output_dir: &Path,
    control: &GutchOperationControl,
    metadata_worker_count: usize,
    reconstruct_pool: Option<&ThreadPool>,
) -> Result<()> {
    let path_resolver = crate::filesystem::paths::ManifestPathResolver::new(output_dir)?;
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
                    .map_err(|e| Error::io_context("reading directory permissions", &path, e))?
                    .permissions();
                if permissions.readonly() != directory.read_only {
                    permissions.set_readonly(directory.read_only);
                    std::fs::set_permissions(&path, permissions)
                        .map_err(|e| Error::io_context("setting entry permissions", &path, e))?;
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

/// Apply the metadata represented by the manifest, including clearing old flags.
pub(super) fn apply_entry_permissions(
    path: &Path,
    read_only: bool,
    executable: Option<bool>,
) -> Result<()> {
    let mut permissions = std::fs::symlink_metadata(path)
        .map_err(|e| Error::io_context("reading entry permissions", path, e))?
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
        permissions.set_readonly(read_only);
    }
    if permissions != original {
        std::fs::set_permissions(&path, permissions)
            .map_err(|e| Error::io_context("setting entry permissions", &path, e))?;
    }
    Ok(())
}
