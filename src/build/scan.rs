use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rayon::prelude::*;

use crate::build::types::{BuildProgressSink, BuildTimings, ChunkPayload, ExistingChunkLocation};
use crate::hash::ChunkHash;
use crate::manifest::{
    self, ChunkRef, DirectoryManifest, FileManifest, SnapshotEntry, SymlinkManifest,
};
use crate::runtime::{ChunkStorageMode, run_in_thread_pool};
use crate::{Error, Result};

pub struct FileBuildEntry {
    pub manifest: FileManifest,
    pub chunks: Vec<(ChunkHash, ChunkPayload)>,
    pub size: u64,
    pub file_hash: Duration,
    pub file_hash_bytes: u64,
    pub stream_chunk: Duration,
    pub stream_chunk_bytes: u64,
    pub chunk_hash_judgment: Duration,
    pub chunk_hash_judgment_bytes: u64,
    pub metadata: Duration,
}

pub(crate) struct ScanDirectoryOptions<'a> {
    pub previous_file_manifests: &'a HashMap<String, Vec<FileManifest>>,
    pub chunking_profile: &'a manifest::ChunkSizingProfile,
    pub existing_chunk_locations: &'a HashMap<ChunkHash, ExistingChunkLocation>,
    pub chunk_storage_mode: ChunkStorageMode,
    pub chunk_spool_root: Option<&'a Path>,
    pub worker_count: usize,
    pub progress: Option<Arc<dyn BuildProgressSink>>,
    pub control: crate::GutchOperationControl,
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
    dir: &Path,
    options: ScanDirectoryOptions<'_>,
    timings: &mut BuildTimings,
) -> Result<(Vec<SnapshotEntry>, HashMap<ChunkHash, ChunkPayload>, u64)> {
    let control = options.control.clone();
    let metadata_entries = scan_metadata_entries(dir, &options.control)?;
    let file_entries = scan_directory_into_file_entries(dir, options)?;

    let mut entries = metadata_entries;
    let mut unique_chunks = HashMap::new();
    let mut total_logical_size: u64 = 0;

    for entry in file_entries {
        control.checkpoint()?;
        accumulate_entry_timings(timings, &entry);
        total_logical_size += entry.size;
        entries.push(SnapshotEntry::File(entry.manifest));
        for (hash, data) in entry.chunks {
            unique_chunks.entry(hash).or_insert(data);
        }
    }

    entries.sort_by(|left, right| left.path().cmp(right.path()));
    manifest::validate_snapshot_entries(&entries)?;
    Ok((entries, unique_chunks, total_logical_size))
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

fn scan_metadata_entries(
    root: &Path,
    control: &crate::GutchOperationControl,
) -> Result<Vec<SnapshotEntry>> {
    let mut entries = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
    {
        control.checkpoint()?;
        let entry = entry?;
        if entry.depth() == 0 || entry.file_type().is_file() {
            continue;
        }
        let path = normalized_relative(root, entry.path())?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if entry.file_type().is_dir() {
            entries.push(SnapshotEntry::Directory(DirectoryManifest {
                path,
                read_only: metadata.permissions().readonly(),
            }));
        } else if entry.file_type().is_symlink() {
            let raw_target = std::fs::read_link(entry.path())?;
            entries.push(SnapshotEntry::Symlink(SymlinkManifest {
                path,
                target_path: normalize_link_target(entry.path(), &raw_target, root)?,
            }));
        } else {
            return Err(Error::manifest(format!(
                "unsupported filesystem object '{}'",
                entry.path().display()
            )));
        }
    }
    Ok(entries)
}

pub(crate) fn scan_directory_into_file_entries(
    dir: &Path,
    options: ScanDirectoryOptions<'_>,
) -> Result<Vec<FileBuildEntry>> {
    let mut file_paths: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        options.control.checkpoint()?;
        let entry = entry?;
        if entry.file_type().is_file() {
            file_paths.push(entry.path().to_path_buf());
        } else if !entry.file_type().is_dir() && !entry.file_type().is_symlink() {
            return Err(Error::manifest(format!(
                "unsupported filesystem object '{}'",
                entry.path().display()
            )));
        }
    }

    if let Some(progress) = &options.progress {
        progress.on_file_scan_started(file_paths.len());
    }

    let spool_root = options.chunk_spool_root.map(PathBuf::from);
    let progress_for_scan = options.progress.clone();
    let control_for_scan = options.control.clone();
    let scan_one = |path: &PathBuf| -> Result<FileBuildEntry> {
        let result = (|| -> Result<FileBuildEntry> {
            control_for_scan.checkpoint()?;
            let relative = path.strip_prefix(dir).map_err(|e| Error::PathError {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;
            let normalized_relative = relative.to_string_lossy().replace('\\', "/");

            let metadata_started_at = std::time::Instant::now();
            let source_metadata = std::fs::symlink_metadata(path)?;
            let metadata = metadata_started_at.elapsed();
            let previous_candidates = options
                .previous_file_manifests
                .get(&normalized_relative)
                .map(Vec::as_slice)
                .unwrap_or_default();

            scan_changed_file(
                path,
                relative,
                normalized_relative,
                &source_metadata,
                metadata,
                previous_candidates,
                &options,
                &control_for_scan,
                spool_root.as_ref(),
            )
        })();

        if result.is_ok()
            && let Some(progress) = &progress_for_scan
        {
            progress.on_file_scanned();
        }

        result
    };

    if options.worker_count <= 1 {
        file_paths.iter().map(scan_one).collect::<Result<Vec<_>>>()
    } else {
        run_in_thread_pool(options.worker_count, || {
            file_paths
                .par_iter()
                .map(scan_one)
                .collect::<Result<Vec<_>>>()
        })?
    }
}

#[allow(clippy::too_many_arguments)]
fn scan_changed_file(
    path: &Path,
    relative: &Path,
    normalized_relative: String,
    source_metadata: &std::fs::Metadata,
    metadata: Duration,
    previous_candidates: &[FileManifest],
    options: &ScanDirectoryOptions<'_>,
    control: &crate::GutchOperationControl,
    spool_root: Option<&PathBuf>,
) -> Result<FileBuildEntry> {
    let read_only = source_metadata.permissions().readonly();
    #[cfg(unix)]
    let executable = {
        use std::os::unix::fs::PermissionsExt;
        source_metadata.permissions().mode() & 0o111 != 0
    };
    #[cfg(not(unix))]
    let executable = false;
    let (chunk_scan, chunk_timings) =
        crate::chunk::chunk_file_streaming_with_file_hash_and_control(
            path,
            options.chunking_profile,
            control,
        )?;
    let file_size = chunk_scan.file_size;
    let full_file_hash = chunk_scan.full_file_hash;
    let file_hash = chunk_scan.full_file_hash_duration;

    if let Some(previous) = previous_candidates.iter().find(|candidate| {
        candidate.size == file_size
            && candidate.full_file_hash == full_file_hash
            && candidate.executable == executable
            && candidate.read_only == read_only
    }) {
        return Ok(FileBuildEntry {
            manifest: previous.clone(),
            chunks: Vec::new(),
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

    let mut chunk_refs = Vec::with_capacity(chunk_scan.chunks.len());
    let mut chunks = Vec::with_capacity(chunk_scan.chunks.len());
    let mut spill_file = None;
    let mut spill_path = None;
    let mut spill_offset: u64 = 0;
    for cr in chunk_scan.chunks {
        control.checkpoint()?;
        let chunk_hash = ChunkHash::from_blake3(cr.hash);
        chunk_refs.push(ChunkRef {
            chunk_hash: chunk_hash.to_hex(),
            raw_size: cr.data.len() as u64,
        });
        if !options.existing_chunk_locations.contains_key(&chunk_hash) {
            let payload = match options.chunk_storage_mode {
                ChunkStorageMode::Memory => ChunkPayload::InMemory {
                    bytes: std::sync::Arc::<[u8]>::from(cr.data),
                },
                ChunkStorageMode::TempSpill => {
                    let spool_root = spool_root.as_ref().ok_or_else(|| {
                        Error::manifest("temp spill mode requires a chunk spool directory")
                    })?;
                    if spill_path.is_none() {
                        let relative_name = relative.to_string_lossy().replace(['\\', '/'], "__");
                        spill_path = Some(spool_root.join(format!("{relative_name}.spill")));
                    }
                    if spill_file.is_none() {
                        let path = spill_path.as_ref().ok_or_else(|| {
                            Error::manifest("temp spill path missing after initialization")
                        })?;
                        spill_file = Some(std::fs::File::create(path)?);
                    }
                    let local_spill_path = spill_path.as_ref().ok_or_else(|| {
                        Error::manifest("temp spill path missing after file creation")
                    })?;
                    let file = spill_file
                        .as_mut()
                        .ok_or_else(|| Error::manifest("temp spill file missing after creation"))?;
                    let chunk_offset = spill_offset;
                    file.write_all(&cr.data)?;
                    spill_offset += cr.data.len() as u64;
                    ChunkPayload::TempFile {
                        path: local_spill_path.clone(),
                        offset: chunk_offset,
                        raw_size: cr.data.len() as u64,
                    }
                }
            };
            chunks.push((chunk_hash, payload));
        }
    }

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
