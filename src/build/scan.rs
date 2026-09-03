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
    pub(crate) size: u64,
    pub(crate) file_hash: Duration,
    pub(crate) file_hash_bytes: u64,
    pub(crate) stream_chunk: Duration,
    pub(crate) stream_chunk_bytes: u64,
    pub(crate) chunk_hash_judgment: Duration,
    pub(crate) chunk_hash_judgment_bytes: u64,
    pub(crate) metadata: Duration,
}

pub(crate) struct ScanDirectoryOptions<'a> {
    pub previous_file_manifests: &'a HashMap<String, Vec<FileManifest>>,
    pub chunking_profile: &'a manifest::ChunkSizingProfile,
    pub existing_chunk_locations: &'a HashMap<ChunkHash, ExistingChunkLocation>,
    pub worker_count: usize,
    pub worker_pool: &'a ThreadPool,
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
    let metadata_entries = {
        let _metadata_span = options
            .control
            .profile_span("gutch.capture.scan_source.read_metadata_entries");
        scan_metadata_entries(
            dir,
            &options.control,
            options.worker_count,
            options.worker_pool,
        )?
    };
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
    worker_count: usize,
    worker_pool: &ThreadPool,
) -> Result<Vec<SnapshotEntry>> {
    let mut paths = Vec::new();
    {
        let _enumerate_span =
            control.profile_span("gutch.capture.scan_source.enumerate_metadata_paths");
        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .sort_by_file_name()
        {
            control.checkpoint()?;
            let entry = entry?;
            if entry.depth() == 0 || entry.file_type().is_file() {
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
            paths.push((entry.path().to_path_buf(), kind));
        }
    }
    let build_entry = |(path, kind): &(PathBuf, u8)| -> Result<SnapshotEntry> {
        control.checkpoint()?;
        let normalized = normalized_relative(root, path)?;
        let metadata = std::fs::symlink_metadata(path)?;
        match kind {
            0 => Ok(SnapshotEntry::Directory(DirectoryManifest {
                path: normalized,
                read_only: metadata.permissions().readonly(),
            })),
            1 => {
                let raw_target = std::fs::read_link(path)?;
                Ok(SnapshotEntry::Symlink(SymlinkManifest {
                    path: normalized,
                    target_path: normalize_link_target(path, &raw_target, root)?,
                }))
            }
            _ => Err(Error::manifest("unsupported filesystem object")),
        }
    };
    if worker_count <= 1 {
        paths.iter().map(build_entry).collect()
    } else {
        worker_pool.install(|| paths.par_iter().map(build_entry).collect())
    }
}

pub(crate) fn scan_directory_into_file_entries(
    dir: &Path,
    options: ScanDirectoryOptions<'_>,
) -> Result<Vec<FileBuildEntry>> {
    let mut file_paths: Vec<PathBuf> = Vec::new();
    {
        let _enumerate_span = options
            .control
            .profile_span("gutch.capture.scan_source.enumerate_file_paths");
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
    }
    options.control.profile_event_with_args(
        "gutch.capture.scan_source.enumerate_file_paths.complete",
        std::collections::BTreeMap::from([(
            "file_count".to_string(),
            serde_json::json!(file_paths.len()),
        )]),
    );

    let progress = BuildProgressTracker::new(
        &options.control,
        crate::GutchProgressPhase::Scan,
        "Scanning files",
    );
    progress.start(file_paths.len());

    let progress_for_scan = progress.clone();
    let control_for_scan = options.control.clone();
    let scan_one = |path: &PathBuf| -> Result<FileBuildEntry> {
        let result = (|| -> Result<FileBuildEntry> {
            control_for_scan.checkpoint()?;
            let relative = path.strip_prefix(dir).map_err(|e| Error::PathError {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;
            let normalized_relative = relative.to_string_lossy().replace('\\', "/");

            let (source_metadata, metadata) = {
                let _metadata_span = control_for_scan.profile_span_with_args(
                    "gutch.capture.scan_source.file.read_metadata",
                    crate::profile::file_trace_args(&normalized_relative, 0),
                );
                let metadata_started_at = std::time::Instant::now();
                let source_metadata = std::fs::symlink_metadata(path)?;
                (source_metadata, metadata_started_at.elapsed())
            };
            let previous_candidates = options
                .previous_file_manifests
                .get(&normalized_relative)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let _file_span = control_for_scan.profile_span_with_args(
                "gutch.capture.scan_source.file",
                crate::profile::file_trace_args(&normalized_relative, source_metadata.len()),
            );

            scan_changed_file(ScanChangedFileRequest {
                path,
                normalized_relative,
                source_metadata: &source_metadata,
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
        file_paths.iter().map(scan_one).collect::<Result<Vec<_>>>()
    } else {
        options.worker_pool.install(|| {
            file_paths
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
            "gutch.capture.scan_source.file.chunk_and_hash",
            std::collections::BTreeMap::from([(
                "size_mib".to_string(),
                serde_json::json!(source_metadata.len() as f64 / (1024.0 * 1024.0)),
            )]),
        );
        crate::chunk::chunk_file_streaming_with_file_hash_and_control_on_pool(
            path,
            options.chunking_profile,
            control,
            options.worker_pool,
        )?
    };
    let file_size = chunk_scan.file_size;
    let full_file_hash = chunk_scan.full_file_hash;
    let file_hash = chunk_scan.full_file_hash_duration;

    let previous = {
        let _reuse_span = control.profile_span_with_args(
            "gutch.capture.scan_source.file.check_previous_snapshot",
            crate::profile::file_trace_args(&normalized_relative, file_size),
        );
        previous_candidates.iter().find(|candidate| {
            candidate.size == file_size
                && candidate.full_file_hash == full_file_hash
                && candidate.executable == executable
                && candidate.read_only == read_only
        })
    };
    if let Some(previous) = previous {
        control.profile_event_with_args(
            "gutch.capture.scan_source.file.reused_previous_manifest",
            crate::profile::file_trace_args(&normalized_relative, file_size),
        );
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

    let (chunk_refs, chunks) = {
        let _refs_span = control.profile_span_with_args(
            "gutch.capture.scan_source.file.build_chunk_references",
            crate::profile::file_trace_args(&normalized_relative, file_size),
        );
        let mut chunk_refs = Vec::with_capacity(chunk_scan.chunks.len());
        let mut chunks = Vec::with_capacity(chunk_scan.chunks.len());
        for cr in chunk_scan.chunks {
            control.checkpoint()?;
            let chunk_hash = ChunkHash::from_blake3(cr.hash);
            chunk_refs.push(ChunkRef {
                chunk_hash: chunk_hash.to_hex(),
                raw_size: cr.data.len() as u64,
            });
            if !options.existing_chunk_locations.contains_key(&chunk_hash) {
                let payload = ChunkPayload::InMemory {
                    bytes: std::sync::Arc::<[u8]>::from(cr.data),
                };
                chunks.push((chunk_hash, payload));
            }
        }
        (chunk_refs, chunks)
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
