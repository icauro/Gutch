//! Scans a source tree into a stable inventory of files, directories and links.

use crate::profiling::trace_args;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use rayon::prelude::*;

use crate::Error;
use crate::Result;
use crate::artifact::chunk::scan_file_chunks;
use crate::artifact::format::size_u32;
use crate::artifact::hash::ChunkHash;
use crate::artifact::profiles::ChunkSizingProfile;
use crate::capture::build::progress::BuildProgressTracker;
use crate::capture::build::types::BuildTimings;
use crate::capture::build::types::ChunkPayload;
use crate::filesystem::constraints::validate_paths;
use crate::filesystem::paths::manifest_path_from_native;
use crate::manifest;
use crate::manifest::ChunkRef;
use crate::manifest::DirectoryManifest;
use crate::manifest::FileManifest;
use crate::manifest::SnapshotEntry;
use crate::manifest::SymlinkManifest;
use crate::manifest::constraints::RepoConstraints;
use crate::profiling::file_trace_args;
use crate::progress::OperationControl;
use crate::progress::ProgressPhase;
use rayon::ThreadPool;
use std::fs;
use std::fs::Metadata;
use std::path::Component;
use std::sync::Arc;
use std::time::SystemTime;

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
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ChunkPlacementKey {
    pub(crate) file_content_hash: ChunkHash,
    pub(crate) file_offset: u64,
}

pub(crate) struct ScanDirectoryOptions<'a> {
    pub chunking_profile: &'a ChunkSizingProfile,
    pub available_chunk_sizes: &'a HashMap<ChunkHash, u64>,
    pub worker_count: usize,
    pub worker_pool: &'a ThreadPool,
    pub control: OperationControl,
}

pub(crate) struct SourceFileEntry {
    pub(crate) path: PathBuf,
    pub(crate) normalized_relative: String,
    pub(crate) metadata: Metadata,
}

pub(crate) struct SourceInventory {
    pub(crate) metadata_entries: Vec<SnapshotEntry>,
    pub(crate) files: Vec<SourceFileEntry>,
    pub(crate) source_modified_at: SystemTime,
}

pub(crate) fn accumulate_entry_timings(timings: &mut BuildTimings, entry: &FileBuildEntry) {
    timings.logical_bytes += entry.size;
    timings.file_hash += entry.file_hash;
    timings.file_hash_bytes += entry.file_hash_bytes;
    timings.stream_chunk += entry.stream_chunk;
    timings.stream_chunk_bytes += entry.stream_chunk_bytes;
    timings.chunk_hash_judgment += entry.chunk_hash_judgment;
    timings.chunk_hash_judgment_bytes += entry.chunk_hash_judgment_bytes;
}

pub(crate) struct ScannedChunks {
    pub(crate) entries: Vec<SnapshotEntry>,
    pub(crate) unique_chunks: HashMap<ChunkHash, ChunkPayload>,
    pub(crate) chunk_placements: HashMap<ChunkHash, ChunkPlacementKey>,
    pub(crate) total_logical_size: u64,
}

pub(crate) fn collect_scanned_chunks(
    inventory: &SourceInventory,
    options: ScanDirectoryOptions<'_>,
    timings: &mut BuildTimings,
) -> Result<ScannedChunks> {
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
            let file_content_hash = entry.manifest.full_file_hash;
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
    Ok(ScannedChunks {
        entries,
        unique_chunks,
        chunk_placements,
        total_logical_size,
    })
}

fn normalized_relative(root: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|error| Error::invalid_path(path, error.to_string()))?;
    manifest_path_from_native(relative)
}

fn normalize_link_target(link_path: &Path, raw_target: &Path, root: &Path) -> Result<String> {
    if raw_target.is_absolute() {
        return Err(Error::capture(format!(
            "symbolic link '{}' has an absolute target",
            link_path.display()
        )));
    }
    let link_relative = link_path
        .strip_prefix(root)
        .map_err(|error| Error::invalid_path(link_path, error.to_string()))?;
    let combined = link_relative
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(raw_target);
    let mut parts = Vec::new();
    for component in combined.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(Error::capture(format!(
                        "symbolic link '{}' escapes the snapshot root",
                        link_path.display()
                    )));
                }
            }
            _ => return Err(Error::capture("symbolic link target is not relative")),
        }
    }
    let normalized = parts.iter().collect::<PathBuf>();
    manifest_path_from_native(&normalized)
}

pub(crate) fn scan_source_inventory(
    root: &Path,
    control: &OperationControl,
    worker_count: usize,
    worker_pool: &ThreadPool,
) -> Result<SourceInventory> {
    enum MetadataEntryKind {
        Directory,
        Symlink,
    }

    struct MetadataEntry {
        path: PathBuf,
        normalized_relative: String,
        kind: MetadataEntryKind,
        metadata: Metadata,
    }

    let mut metadata_entries = Vec::new();
    let mut files = Vec::new();
    let mut newest_file_time = NewestFileTime::default();
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
            // WalkDir already holds metadata on Windows. Each inventory is a
            // new walk, so post-capture validation still observes the source anew.
            let metadata = entry.metadata()?;
            let normalized_relative = normalized_relative(root, &path)?;
            if entry.file_type().is_file() {
                newest_file_time.observe(metadata.modified()?);
                files.push(SourceFileEntry {
                    path,
                    normalized_relative,
                    metadata,
                });
                continue;
            }
            let kind = if entry.file_type().is_dir() {
                MetadataEntryKind::Directory
            } else if entry.file_type().is_symlink() {
                MetadataEntryKind::Symlink
            } else {
                return Err(Error::capture(format!(
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
            MetadataEntryKind::Directory => Ok(SnapshotEntry::Directory(DirectoryManifest {
                path: entry.normalized_relative.clone(),
                read_only: entry.metadata.permissions().readonly(),
            })),
            MetadataEntryKind::Symlink => {
                let raw_target = fs::read_link(&entry.path)?;
                Ok(SnapshotEntry::Symlink(SymlinkManifest {
                    path: entry.normalized_relative.clone(),
                    target_path: normalize_link_target(&entry.path, &raw_target, root)?,
                }))
            }
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
    Ok(SourceInventory {
        metadata_entries,
        files,
        source_modified_at: newest_file_time.resolve(root)?,
    })
}

/// Tracks a source tree's newest file modification time. Timestamps describe
/// the source only; they never contribute to snapshot identity.
#[derive(Default)]
struct NewestFileTime(Option<SystemTime>);

impl NewestFileTime {
    fn observe(&mut self, modified_at: SystemTime) {
        if self.0.is_none_or(|newest| modified_at > newest) {
            self.0 = Some(modified_at);
        }
    }

    /// A tree without files falls back to its root directory's own time.
    fn resolve(self, root: &Path) -> Result<SystemTime> {
        match self.0 {
            Some(newest) => Ok(newest),
            None => Ok(fs::metadata(root)?.modified()?),
        }
    }
}

/// Returns the newest file modification time under `dir`, as `scan` records it.
#[cfg(test)]
pub(crate) fn source_modified_at(dir: &Path, control: &OperationControl) -> Result<SystemTime> {
    if !dir.is_dir() {
        return Err(Error::invalid_input(format!(
            "source path is not a directory: {}",
            dir.display()
        )));
    }
    let mut newest_file_time = NewestFileTime::default();
    for entry in walkdir::WalkDir::new(dir) {
        control.checkpoint()?;
        let entry = entry?;
        if entry.file_type().is_file() {
            newest_file_time.observe(entry.metadata()?.modified()?);
        }
    }
    newest_file_time.resolve(dir)
}

pub(crate) fn format_system_time_utc(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

pub(crate) fn scan_directory_into_file_entries(
    inventory: &SourceInventory,
    options: ScanDirectoryOptions<'_>,
) -> Result<Vec<FileBuildEntry>> {
    options.control.profile_event_with_args(
        "capture.scan_source.inventory.complete",
        || trace_args!("file_count" => inventory.files.len()),
    );

    let progress =
        BuildProgressTracker::new(&options.control, ProgressPhase::Scan, "Scanning files");
    progress.start(inventory.files.len());

    let progress_for_scan = progress.clone();
    let control_for_scan = options.control.clone();
    let scan_one = |source_file: &SourceFileEntry| -> Result<FileBuildEntry> {
        let result = (|| -> Result<FileBuildEntry> {
            control_for_scan.checkpoint()?;
            let path = &source_file.path;
            let normalized_relative = source_file.normalized_relative.clone();
            let source_metadata = &source_file.metadata;
            let _file_span = control_for_scan.profile_span_with_args("capture.scan.file", || {
                file_trace_args(&normalized_relative, source_metadata.len())
            });

            scan_file(ScanFileRequest {
                path,
                normalized_relative,
                source_metadata,
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
                // File sizes vary widely; keep remaining files available to idle workers.
                // Indexed collection still preserves inventory order for deterministic packing.
                .with_max_len(1)
                .map(scan_one)
                .collect::<Result<Vec<_>>>()
        })
    }
}

struct ScanFileRequest<'a> {
    path: &'a Path,
    normalized_relative: String,
    source_metadata: &'a Metadata,
    options: &'a ScanDirectoryOptions<'a>,
    control: &'a OperationControl,
}

fn scan_file(request: ScanFileRequest<'_>) -> Result<FileBuildEntry> {
    let ScanFileRequest {
        path,
        normalized_relative,
        source_metadata,
        options,
        control,
    } = request;
    let read_only = source_metadata.permissions().readonly();
    let executable = crate::filesystem::is_executable(source_metadata);
    let (chunk_scan, chunk_timings) = scan_file_chunks(
        path,
        source_metadata.len(),
        options.chunking_profile,
        control,
    )?;
    let file_size = chunk_scan.file_size;
    let full_file_hash = chunk_scan.full_file_hash;
    let file_hash = chunk_scan.full_file_hash_duration;

    let stream_chunk = chunk_timings.boundary_scan;

    let (chunk_refs, chunks, new_chunk_offsets) = {
        let mut chunk_refs = Vec::with_capacity(chunk_scan.chunks.len());
        let mut chunks = Vec::with_capacity(chunk_scan.chunks.len());
        let mut new_chunk_offsets = Vec::with_capacity(chunk_scan.chunks.len());
        let mut file_offset = 0u64;
        let source = Arc::new(path.to_path_buf());
        for cr in chunk_scan.chunks {
            control.checkpoint()?;
            let chunk_hash = cr.hash;
            let raw_size = cr.size;
            chunk_refs.push(ChunkRef {
                chunk_hash,
                raw_size: size_u32(raw_size)?,
            });
            if !options.available_chunk_sizes.contains_key(&chunk_hash) {
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
                .ok_or_else(|| Error::capture("file chunk offsets overflow"))?;
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
    })
}

/// Apply repository filename constraints to the complete pre-read inventory.
pub(crate) fn validate_source_inventory(
    inventory: &SourceInventory,
    constraints: &RepoConstraints,
) -> Result<()> {
    validate_paths(
        &constraints.targets,
        inventory
            .metadata_entries
            .iter()
            .map(|entry| entry.path())
            .chain(
                inventory
                    .files
                    .iter()
                    .map(|file| file.normalized_relative.as_str()),
            ),
        "capture",
        None,
    )
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
    // Compare the metadata captured during each inventory's scan, never a
    // fresh read: re-reading current state for both sides would always agree
    // with itself. Both walks sort by file name, so equal trees list entries
    // in the same order.
    #[derive(PartialEq)]
    struct FileObservation<'a> {
        path: &'a str,
        size: u64,
        modified_at: SystemTime,
        read_only: bool,
        executable: bool,
    }
    fn observe_files(inventory: &SourceInventory) -> Result<Vec<FileObservation<'_>>> {
        inventory
            .files
            .iter()
            .map(|entry| {
                Ok(FileObservation {
                    path: &entry.normalized_relative,
                    size: entry.metadata.len(),
                    modified_at: entry.metadata.modified()?,
                    read_only: entry.metadata.permissions().readonly(),
                    executable: crate::filesystem::is_executable(&entry.metadata),
                })
            })
            .collect()
    }

    if before.metadata_entries != after.metadata_entries
        || observe_files(before)? != observe_files(after)?
    {
        return Err(Error::SourceChanged { path: None });
    }
    Ok(())
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
