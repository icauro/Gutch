use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rayon::prelude::*;

use crate::build_types::ExistingChunkLocation;
use crate::hash::hash_hex;
use crate::manifest::{self, ChunkRef, FileManifest};
use crate::{Error, Result};

pub(crate) struct FileBuildEntry {
    pub(crate) manifest: FileManifest,
    pub(crate) chunks: Vec<(String, Vec<u8>)>,
    pub(crate) size: u64,
    pub(crate) scan_read: Duration,
    pub(crate) file_hash: Duration,
    pub(crate) chunking: Duration,
    pub(crate) chunking_bytes: u64,
    pub(crate) chunk_hash_judgment: Duration,
    pub(crate) chunk_hash_judgment_bytes: u64,
    pub(crate) metadata: Duration,
}

fn file_mode(metadata: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return metadata.permissions().mode();
    }
    #[cfg(not(unix))]
    {
        if metadata.permissions().readonly() {
            0o444
        } else {
            0o644
        }
    }
}

pub(crate) fn scan_directory_into_file_entries(
    dir: &Path,
    previous_file_manifests: &HashMap<String, FileManifest>,
    chunking_profile: &manifest::ChunkSizingProfile,
    existing_chunk_locations: &HashMap<String, ExistingChunkLocation>,
) -> Result<Vec<FileBuildEntry>> {
    let mut file_paths: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry?;
        if entry.path().is_file() {
            file_paths.push(entry.path().to_path_buf());
        }
    }

    file_paths
        .par_iter()
        .map(|path| -> Result<FileBuildEntry> {
            let relative = path.strip_prefix(dir).map_err(|e| Error::PathError {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;

            let t = Instant::now();
            let file_data = std::fs::read(path)?;
            let scan_read = t.elapsed();
            let file_size = file_data.len() as u64;

            let t = Instant::now();
            let full_file_hash = hash_hex(&crate::hash::hash_chunk(&file_data));
            let file_hash = t.elapsed();

            if let Some(previous) =
                previous_file_manifests.get(&relative.to_string_lossy().replace('\\', "/"))
            {
                if previous.size == file_size && previous.full_file_hash == full_file_hash {
                    return Ok(FileBuildEntry {
                        manifest: previous.clone(),
                        chunks: Vec::new(),
                        size: file_size,
                        scan_read,
                        file_hash,
                        chunking: Duration::ZERO,
                        chunking_bytes: 0,
                        chunk_hash_judgment: Duration::ZERO,
                        chunk_hash_judgment_bytes: 0,
                        metadata: Duration::ZERO,
                    });
                }
            }

            let (chunk_results, chunk_timings) =
                crate::chunk::chunk_slices(&file_data, chunking_profile);
            let chunking = chunk_timings.boundary_scan;

            let mut chunk_refs = Vec::with_capacity(chunk_results.len());
            let mut chunks = Vec::with_capacity(chunk_results.len());
            for cr in &chunk_results {
                let ch = hash_hex(&cr.hash);
                chunk_refs.push(ChunkRef {
                    chunk_hash: ch.clone(),
                    raw_size: cr.len as u64,
                });
                if !existing_chunk_locations.contains_key(&ch) {
                    let start = cr.offset as usize;
                    chunks.push((ch, file_data[start..start + cr.len].to_vec()));
                }
            }

            let t = Instant::now();
            let metadata = std::fs::metadata(path)?;
            let mode = file_mode(&metadata);
            let metadata = t.elapsed();

            Ok(FileBuildEntry {
                manifest: FileManifest {
                    path: relative.to_string_lossy().replace('\\', "/"),
                    size: file_size,
                    mode,
                    executable: false,
                    symlink_target: None,
                    full_file_hash,
                    chunks: chunk_refs,
                },
                chunks,
                size: file_size,
                scan_read,
                file_hash,
                chunking,
                chunking_bytes: chunk_timings.boundary_scan_bytes,
                chunk_hash_judgment: chunk_timings.hash_judgment,
                chunk_hash_judgment_bytes: chunk_timings.hash_judgment_bytes,
                metadata,
            })
        })
        .collect::<Result<Vec<_>>>()
}
