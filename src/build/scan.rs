use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rayon::prelude::*;

use crate::build::progress::BuildProgressTracker;
use crate::build::types::{BuildTimings, ChunkPayload, ExistingChunkLocation};
use crate::hash::ChunkHash;
use crate::manifest::{
    self, ChunkRef, DirectoryManifest, FileManifest, SnapshotEntry, SymlinkManifest,
};
use crate::{Error, Result};
use rayon::ThreadPool;

pub(crate) struct FileBuildEntry {
    pub(crate) manifest: FileManifest,
    pub(crate) chunks: Vec<(ChunkHash, ChunkPayload)>,
    pub(crate) new_chunk_offsets: Vec<(ChunkHash, u64)>,
    pub(crate) size: u64,
    pub(crate) file_hash: Duration,
    pub(crate) file_hash_bytes: u64,
    pub(crate) stream_chunk: Duration,
    pub(crate) stream_chunk_bytes: u64,
    pub(crate) chunk_hash_judgment: Duration,
    pub(crate) chunk_hash_judgment_bytes: u64,
    pub(crate) metadata: Duration,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ChunkPlacementKey {
    pub(crate) file_content_hash: ChunkHash,
    pub(crate) file_offset: u64,
}

pub(crate) struct ScanDirectoryOptions<'a> {
    pub previous_file_manifests: &'a HashMap<String, Vec<FileManifest>>,
    pub chunking_profile: &'a manifest::ChunkSizingProfile,
    pub existing_chunk_locations: &'a HashMap<ChunkHash, ExistingChunkLocation>,
    pub worker_count: usize,
    pub worker_pool: &'a ThreadPool,
    pub control: crate::GutchOperationControl,
}

pub(crate) struct SourceFileEntry {
    pub(crate) path: PathBuf,
    pub(crate) normalized_relative: String,
    pub(crate) metadata: std::fs::Metadata,
}

pub(crate) struct SourceInventory {
    pub(crate) metadata_entries: Vec<SnapshotEntry>,
    pub(crate) files: Vec<SourceFileEntry>,
    pub(crate) source_modified_at: std::time::SystemTime,
}

pub(crate) fn accumulate_entry_timings(timings: &mut BuildTimings, entry: &FileBuildEntry) {
    timings.logical_bytes += entry.size;
    timings.file_hash += entry.file_hash;
    timings.file_hash_bytes += entry.file_hash_bytes;
    timings.stream_chunk += entry.stream_chunk;
    timings.stream_chunk_bytes += entry.stream_chunk_bytes;
    timings.chunk_hash_judgment += entry.chunk_hash_judgment;
    timings.chunk_hash_judgment_bytes += entry.chunk_hash_judgment_bytes;
    timings.metadata += entry.metadata;
}

pub(crate) fn collect_scanned_chunks(
    inventory: &SourceInventory,
    options: ScanDirectoryOptions<'_>,
    timings: &mut BuildTimings,
) -> Result<(
    Vec<SnapshotEntry>,
    HashMap<ChunkHash, ChunkPayload>,
    HashMap<ChunkHash, ChunkPlacementKey>,
    u64,
)> {
    let control = options.control.clone();
    let metadata_entries = inventory.metadata_entries.clone();
    let file_entries = scan_directory_into_file_entries(inventory, options)?;

    let mut entries = metadata_entries;
    let mut unique_chunks = HashMap::new();
    let mut chunk_placements = HashMap::new();
    let mut total_logical_size: u64 = 0;

    for entry in file_entries {
        control.checkpoint()?;
        accumulate_entry_timings(timings, &entry);
        total_logical_size += entry.size;
        if !entry.new_chunk_offsets.is_empty() {
            let file_content_hash = ChunkHash::parse_hex(&entry.manifest.full_file_hash)?;
            for (chunk_hash, file_offset) in entry.new_chunk_offsets.iter().copied() {
                let candidate = ChunkPlacementKey {
                    file_content_hash,
                    file_offset,
                };
                chunk_placements
                    .entry(chunk_hash)
                    .and_modify(|current: &mut ChunkPlacementKey| {
                        if (candidate.file_content_hash, candidate.file_offset)
                            < (current.file_content_hash, current.file_offset)
                        {
                            *current = candidate;
                        }
                    })
                    .or_insert(candidate);
            }
        }
        entries.push(SnapshotEntry::File(entry.manifest));
        for (hash, data) in entry.chunks {
            unique_chunks.entry(hash).or_insert(data);
        }
    }

    entries.sort_by(|left, right| left.path().cmp(right.path()));
    manifest::validate_snapshot_entries(&entries)?;
    Ok((entries, unique_chunks, chunk_placements, total_logical_size))
}

fn normalized_relative(root: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(root).map_err(|error| Error::PathError {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn normalize_link_target(link_path: &Path, raw_target: &Path, root: &Path) -> Result<String> {
    if raw_target.is_absolute() {
        return Err(Error::manifest(format!(
            "symbolic link '{}' has an absolute target",
            link_path.display()
        )));
    }
    let link_relative = link_path
        .strip_prefix(root)
        .map_err(|error| Error::PathError {
            path: link_path.to_path_buf(),
            reason: error.to_string(),
        })?;
    let combined = link_relative
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(raw_target);
    let mut parts = Vec::new();
    for component in combined.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(part.to_os_string()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(Error::manifest(format!(
                        "symbolic link '{}' escapes the snapshot root",
                        link_path.display()
                    )));
                }
            }
            _ => return Err(Error::manifest("symbolic link target is not relative")),
        }
    }
    let normalized = parts.iter().collect::<PathBuf>();
    let normalized = normalized.to_string_lossy().replace('\\', "/");
    crate::validate_manifest_relative_path(&normalized, false)?;
    Ok(normalized)
}

pub(crate) fn scan_source_inventory(
    root: &Path,
    control: &crate::GutchOperationControl,
    worker_count: usize,
    worker_pool: &ThreadPool,
) -> Result<SourceInventory> {
    struct MetadataEntry {
        path: PathBuf,
        normalized_relative: String,
        kind: u8,
        metadata: std::fs::Metadata,
    }

    let mut metadata_entries = Vec::new();
    let mut files = Vec::new();
    let mut max_file_modified_at = std::time::SystemTime::UNIX_EPOCH;
    let mut found_file = false;
    {
        let _enumerate_span = control.profile_span("capture.scan_source.inventory");
        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .sort_by_file_name()
        {
            control.checkpoint()?;
            let entry = entry?;
            if entry.depth() == 0 {
                continue;
            }
            let path = entry.path().to_path_buf();
            let metadata = std::fs::symlink_metadata(&path)?;
            let normalized_relative = normalized_relative(root, &path)?;
            if entry.file_type().is_file() {
                let modified_at = metadata.modified()?;
                if !found_file || modified_at > max_file_modified_at {
                    max_file_modified_at = modified_at;
                    found_file = true;
                }
                files.push(SourceFileEntry {
                    path,
                    normalized_relative,
                    metadata,
                });
                continue;
            }
            let kind = if entry.file_type().is_dir() {
                0u8
            } else if entry.file_type().is_symlink() {
                1u8
            } else {
                return Err(Error::manifest(format!(
                    "unsupported filesystem object '{}'",
                    entry.path().display()
                )));
            };
            metadata_entries.push(MetadataEntry {
                path,
                normalized_relative,
                kind,
                metadata,
            });
        }
    }
    let build_entry = |entry: &MetadataEntry| -> Result<SnapshotEntry> {
        control.checkpoint()?;
        match entry.kind {
            0 => Ok(SnapshotEntry::Directory(DirectoryManifest {
                path: entry.normalized_relative.clone(),
                read_only: entry.metadata.permissions().readonly(),
            })),
            1 => {
                let raw_target = std::fs::read_link(&entry.path)?;
                Ok(SnapshotEntry::Symlink(SymlinkManifest {
                    path: entry.normalized_relative.clone(),
                    target_path: normalize_link_target(&entry.path, &raw_target, root)?,
                }))
            }
            _ => Err(Error::manifest("unsupported filesystem object")),
        }
    };
    let metadata_entries = if worker_count <= 1 {
        metadata_entries
            .iter()
            .map(build_entry)
            .collect::<Result<Vec<_>>>()
    } else {
        worker_pool.install(|| {
            metadata_entries
                .par_iter()
                .map(build_entry)
                .collect::<Result<Vec<_>>>()
        })
    }?;
    let source_modified_at = if found_file {
        max_file_modified_at
    } else {
        std::fs::metadata(root)?.modified()?
    };

    Ok(SourceInventory {
        metadata_entries,
        files,
        source_modified_at,
    })
}

pub(crate) fn scan_directory_into_file_entries(
    inventory: &SourceInventory,
    options: ScanDirectoryOptions<'_>,
) -> Result<Vec<FileBuildEntry>> {
    options.control.profile_event_with_args(
        "capture.scan_source.inventory.complete",
        std::collections::BTreeMap::from([(
            "file_count".to_string(),
            serde_json::json!(inventory.files.len()),
        )]),
    );

    let progress = BuildProgressTracker::new(
        &options.control,
        crate::GutchProgressPhase::Scan,
        "Scanning files",
    );
    progress.start(inventory.files.len());

    let progress_for_scan = progress.clone();
    let control_for_scan = options.control.clone();
    let scan_one = |source_file: &SourceFileEntry| -> Result<FileBuildEntry> {
        let result = (|| -> Result<FileBuildEntry> {
            control_for_scan.checkpoint()?;
            let path = &source_file.path;
            let normalized_relative = source_file.normalized_relative.clone();
            let source_metadata = &source_file.metadata;
            let metadata = Duration::ZERO;
            let previous_candidates = options
                .previous_file_manifests
                .get(&normalized_relative)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let _file_span = control_for_scan.profile_span_with_args(
                "capture.scan_source.file",
                crate::profile::file_trace_args(&normalized_relative, source_metadata.len()),
            );

            scan_changed_file(ScanChangedFileRequest {
                path,
                normalized_relative,
                source_metadata,
                metadata,
                previous_candidates,
                options: &options,
                control: &control_for_scan,
            })
        })();

        if result.is_ok() {
            progress_for_scan.completed();
        }

        result
    };

    if options.worker_count <= 1 {
        inventory
            .files
            .iter()
            .map(scan_one)
            .collect::<Result<Vec<_>>>()
    } else {
        options.worker_pool.install(|| {
            inventory
                .files
                .par_iter()
                .map(scan_one)
                .collect::<Result<Vec<_>>>()
        })
    }
}

struct ScanChangedFileRequest<'a> {
    path: &'a Path,
    normalized_relative: String,
    source_metadata: &'a std::fs::Metadata,
    metadata: Duration,
    previous_candidates: &'a [FileManifest],
    options: &'a ScanDirectoryOptions<'a>,
    control: &'a crate::GutchOperationControl,
}

fn scan_changed_file(request: ScanChangedFileRequest<'_>) -> Result<FileBuildEntry> {
    let ScanChangedFileRequest {
        path,
        normalized_relative,
        source_metadata,
        metadata,
        previous_candidates,
        options,
        control,
    } = request;
    let read_only = source_metadata.permissions().readonly();
    #[cfg(unix)]
    let executable = {
        use std::os::unix::fs::PermissionsExt;
        source_metadata.permissions().mode() & 0o111 != 0
    };
    #[cfg(not(unix))]
    let executable = false;
    let (chunk_scan, chunk_timings) = {
        let _chunk_span = control.profile_span_with_args(
            "capture.scan_source.file.chunk_and_hash",
            std::collections::BTreeMap::from([(
                "size_mib".to_string(),
                serde_json::json!(source_metadata.len() as f64 / (1024.0 * 1024.0)),
            )]),
        );
        crate::chunk::scan_file_chunks(path, options.chunking_profile, control)?
    };
    let file_size = chunk_scan.file_size;
    let full_file_hash = chunk_scan.full_file_hash;
    let file_hash = chunk_scan.full_file_hash_duration;

    let previous = {
        let _reuse_span = control.profile_span_with_args(
            "capture.scan_source.file.check_previous_snapshot",
            crate::profile::file_trace_args(&normalized_relative, file_size),
        );
        previous_candidates.iter().find(|candidate| {
            candidate.size == file_size
                && candidate.full_file_hash == full_file_hash
                && candidate.executable == executable
                && candidate.read_only == read_only
                && candidate.chunks.iter().all(|chunk| {
                    ChunkHash::parse_hex(&chunk.chunk_hash)
                        .is_ok_and(|hash| options.existing_chunk_locations.contains_key(&hash))
                })
        })
    };
    if let Some(previous) = previous {
        control.profile_event_with_args(
            "capture.scan_source.file.reused_previous_manifest",
            crate::profile::file_trace_args(&normalized_relative, file_size),
        );
        return Ok(FileBuildEntry {
            manifest: previous.clone(),
            chunks: Vec::new(),
            new_chunk_offsets: Vec::new(),
            size: file_size,
            file_hash,
            file_hash_bytes: file_size,
            stream_chunk: chunk_timings.boundary_scan,
            stream_chunk_bytes: chunk_timings.boundary_scan_bytes,
            chunk_hash_judgment: chunk_timings.hash_judgment,
            chunk_hash_judgment_bytes: chunk_timings.hash_judgment_bytes,
            metadata,
        });
    }

    let stream_chunk = chunk_timings.boundary_scan;

    let (chunk_refs, chunks, new_chunk_offsets) = {
        let _refs_span = control.profile_span_with_args(
            "capture.scan_source.file.build_chunk_references",
            crate::profile::file_trace_args(&normalized_relative, file_size),
        );
        let mut chunk_refs = Vec::with_capacity(chunk_scan.chunks.len());
        let mut chunks = Vec::with_capacity(chunk_scan.chunks.len());
        let mut new_chunk_offsets = Vec::with_capacity(chunk_scan.chunks.len());
        let mut file_offset = 0u64;
        let source = std::sync::Arc::new(path.to_path_buf());
        for cr in chunk_scan.chunks {
            control.checkpoint()?;
            let chunk_hash = ChunkHash::from_blake3(cr.hash);
            let raw_size = cr.size;
            chunk_refs.push(ChunkRef {
                chunk_hash: chunk_hash.to_hex(),
                raw_size: crate::artifact::format::size_u32(raw_size)?,
            });
            if !options.existing_chunk_locations.contains_key(&chunk_hash) {
                let payload = ChunkPayload {
                    source: source.clone(),
                    offset: cr.offset,
                    size: cr.size,
                    hash: chunk_hash,
                };
                chunks.push((chunk_hash, payload));
                new_chunk_offsets.push((chunk_hash, file_offset));
            }
            file_offset = file_offset
                .checked_add(raw_size)
                .ok_or_else(|| Error::manifest("file chunk offsets overflow"))?;
        }
        (chunk_refs, chunks, new_chunk_offsets)
    };

    Ok(FileBuildEntry {
        manifest: FileManifest {
            path: normalized_relative,
            size: file_size,
            executable,
            read_only,
            full_file_hash,
            chunks: chunk_refs,
        },
        chunks,
        new_chunk_offsets,
        size: file_size,
        file_hash,
        file_hash_bytes: file_size,
        stream_chunk,
        stream_chunk_bytes: chunk_timings.boundary_scan_bytes,
        chunk_hash_judgment: chunk_timings.hash_judgment,
        chunk_hash_judgment_bytes: chunk_timings.hash_judgment_bytes,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_dir(root: &std::path::Path) -> SourceInventory {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        scan_source_inventory(root, &crate::GutchOperationControl::default(), 1, &pool).unwrap()
    }

    fn write_source(root: &std::path::Path, name: &str, content: &[u8]) -> std::path::PathBuf {
        let path = root.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn stable_source_verifies() {
        let temp = tempfile::tempdir().unwrap();
        write_source(temp.path(), "game.exe", b"content");
        write_source(temp.path(), "sub/readme.txt", b"readme");
        let before = scan_dir(temp.path());
        let after = scan_dir(temp.path());
        verify_source_inventory_stable(&before, &after).unwrap();
    }

    #[test]
    fn modified_truncated_added_and_removed_files_are_unstable() {
        use std::time::{Duration, SystemTime};

        fn set_mtime(path: &std::path::Path, secs: u64) {
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
                .unwrap();
        }

        let temp = tempfile::tempdir().unwrap();
        write_source(temp.path(), "game.exe", b"content-1234");
        write_source(temp.path(), "keep.txt", b"keep");
        set_mtime(&temp.path().join("game.exe"), 1_700_000_000);
        set_mtime(&temp.path().join("keep.txt"), 1_700_000_000);
        let before = scan_dir(temp.path());

        // Same-size modification with a visibly newer timestamp.
        write_source(temp.path(), "game.exe", b"content-5678");
        set_mtime(&temp.path().join("game.exe"), 1_700_000_100);
        let after = scan_dir(temp.path());
        assert!(verify_source_inventory_stable(&before, &after).is_err());

        // Truncation.
        write_source(temp.path(), "game.exe", b"content-1234");
        std::fs::File::options()
            .write(true)
            .open(temp.path().join("game.exe"))
            .unwrap()
            .set_len(0)
            .unwrap();
        let after = scan_dir(temp.path());
        assert!(verify_source_inventory_stable(&before, &after).is_err());

        // Addition.
        write_source(temp.path(), "game.exe", b"content-1234");
        write_source(temp.path(), "extra.dll", b"extra");
        let after = scan_dir(temp.path());
        assert!(verify_source_inventory_stable(&before, &after).is_err());

        // Removal.
        std::fs::remove_file(temp.path().join("extra.dll")).unwrap();
        std::fs::remove_file(temp.path().join("keep.txt")).unwrap();
        let after = scan_dir(temp.path());
        assert!(verify_source_inventory_stable(&before, &after).is_err());
    }

    #[test]
    fn replaced_files_and_permission_flips_are_unstable() {
        let temp = tempfile::tempdir().unwrap();
        let path = write_source(temp.path(), "game.exe", b"content");
        let before = scan_dir(temp.path());

        // Permission flip without touching content or timestamps.
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(!permissions.readonly());
        std::fs::set_permissions(&path, permissions).unwrap();
        let after = scan_dir(temp.path());
        assert!(verify_source_inventory_stable(&before, &after).is_err());

        // Replace with different content under the same name.
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(&path, permissions).unwrap();
        std::fs::remove_file(&path).unwrap();
        write_source(temp.path(), "game.exe", b"replacement-bytes!!");
        let after = scan_dir(temp.path());
        assert!(verify_source_inventory_stable(&before, &after).is_err());
    }
}

/// Compares normalized source observations from two inventories: entry paths,
/// entry kinds, file sizes, high-resolution modification times, and supported
/// permissions. Timestamps may be coarse or move backward on some filesystems,
/// so equality only proves no observed change, never content identity (which
/// the content-derived snapshot ID establishes).
pub(crate) fn verify_source_inventory_stable(
    before: &SourceInventory,
    after: &SourceInventory,
) -> Result<()> {
    fn metadata_observation(entry: &SnapshotEntry) -> (String, String, String) {
        match entry {
            SnapshotEntry::File(file) => (
                file.path.clone(),
                String::from("file"),
                format!("{}:{}:{}", file.size, file.executable, file.read_only),
            ),
            SnapshotEntry::Directory(directory) => (
                directory.path.clone(),
                String::from("directory"),
                directory.read_only.to_string(),
            ),
            SnapshotEntry::Symlink(link) => (
                link.path.clone(),
                String::from("symlink"),
                link.target_path.clone(),
            ),
        }
    }

    fn file_observation(
        entry: &SourceFileEntry,
    ) -> Result<(String, u64, std::time::SystemTime, bool, bool)> {
        // Compare the metadata captured during that inventory's scan, never a
        // fresh read: re-reading current state for both sides would always
        // agree with itself.
        let metadata = &entry.metadata;
        let executable = is_executable(metadata);
        Ok((
            entry.normalized_relative.clone(),
            metadata.len(),
            metadata
                .modified()
                .map_err(|_| Error::manifest("source changed during capture".to_string()))?,
            metadata.permissions().readonly(),
            executable,
        ))
    }

    #[cfg(unix)]
    fn is_executable(metadata: &std::fs::Metadata) -> bool {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    fn is_executable(_metadata: &std::fs::Metadata) -> bool {
        false
    }

    let before_metadata: Vec<_> = before
        .metadata_entries
        .iter()
        .map(metadata_observation)
        .collect();
    let after_metadata: Vec<_> = after
        .metadata_entries
        .iter()
        .map(metadata_observation)
        .collect();
    if before_metadata != after_metadata {
        return Err(Error::manifest("source changed during capture"));
    }

    let mut before_files: Vec<_> = before
        .files
        .iter()
        .map(file_observation)
        .collect::<Result<_>>()?;
    let mut after_files: Vec<_> = after
        .files
        .iter()
        .map(file_observation)
        .collect::<Result<_>>()?;
    before_files.sort();
    after_files.sort();
    if before_files != after_files {
        return Err(Error::manifest("source changed during capture"));
    }
    Ok(())
}
