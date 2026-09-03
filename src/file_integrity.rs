use std::cell::RefCell;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::hash::ChunkHash;
use crate::{Error, GutchOperationControl, Result};

pub(crate) const PARALLEL_FULL_HASH_MIN_BYTES: u64 = 16 * 1024 * 1024;
const STREAM_BUFFER_BYTES: usize = 1024 * 1024;

thread_local! {
    static STREAM_BUFFER: RefCell<Vec<u8>> = RefCell::new(vec![0u8; STREAM_BUFFER_BYTES]);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FullFileHashMode {
    Forwarded,
    Stream,
    Mmap,
    MmapRayon,
}

impl FullFileHashMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Forwarded => "copy_forward",
            Self::Stream => "stream",
            Self::Mmap => "mmap",
            Self::MmapRayon => "mmap_rayon",
        }
    }
}

pub(crate) struct FullFileIntegrityResult {
    pub(crate) elapsed: Duration,
    pub(crate) read_elapsed: Duration,
    pub(crate) hash_elapsed: Duration,
    pub(crate) bytes: u64,
    pub(crate) mode: FullFileHashMode,
}

pub(crate) fn select_full_file_hash_mode(
    file_size: u64,
    has_precomputed_hash: bool,
    worker_count: usize,
) -> FullFileHashMode {
    if has_precomputed_hash {
        FullFileHashMode::Forwarded
    } else if file_size < PARALLEL_FULL_HASH_MIN_BYTES {
        FullFileHashMode::Stream
    } else if worker_count <= 1 {
        FullFileHashMode::MmapRayon
    } else {
        FullFileHashMode::Mmap
    }
}

pub(crate) fn verify_full_file_integrity(
    path: &Path,
    file_size: u64,
    expected_hash: ChunkHash,
    precomputed_hash: Option<ChunkHash>,
    worker_count: usize,
    control: &GutchOperationControl,
) -> Result<FullFileIntegrityResult> {
    control.checkpoint()?;
    let started_at = Instant::now();
    let mode = select_full_file_hash_mode(file_size, precomputed_hash.is_some(), worker_count);
    let mut read_elapsed = Duration::ZERO;
    let mut hash_elapsed = Duration::ZERO;

    let actual_hash = match (mode, precomputed_hash) {
        (FullFileHashMode::Forwarded, Some(hash)) => hash,
        (FullFileHashMode::MmapRayon, None) => {
            let hash_started_at = Instant::now();
            let mut hasher = blake3::Hasher::new();
            hasher.update_mmap_rayon(path)?;
            hash_elapsed = hash_started_at.elapsed();
            ChunkHash::from_blake3(hasher.finalize())
        }
        (FullFileHashMode::Mmap, None) => {
            let hash_started_at = Instant::now();
            let mut hasher = blake3::Hasher::new();
            hasher.update_mmap(path)?;
            hash_elapsed = hash_started_at.elapsed();
            ChunkHash::from_blake3(hasher.finalize())
        }
        (FullFileHashMode::Stream, None) => {
            let mut input = File::open(path)?;
            let (hash, measured_read, measured_hash) =
                STREAM_BUFFER.with(|buffer| -> Result<(ChunkHash, Duration, Duration)> {
                    let mut buffer = buffer.borrow_mut();
                    let mut hasher = blake3::Hasher::new();
                    let mut measured_read = Duration::ZERO;
                    let mut measured_hash = Duration::ZERO;
                    loop {
                        control.checkpoint()?;
                        let read_started_at = Instant::now();
                        let read = input.read(&mut buffer)?;
                        measured_read += read_started_at.elapsed();
                        if read == 0 {
                            break;
                        }
                        let hash_started_at = Instant::now();
                        hasher.update(&buffer[..read]);
                        measured_hash += hash_started_at.elapsed();
                    }
                    Ok((
                        ChunkHash::from_blake3(hasher.finalize()),
                        measured_read,
                        measured_hash,
                    ))
                })?;
            read_elapsed = measured_read;
            hash_elapsed = measured_hash;
            hash
        }
        (FullFileHashMode::Forwarded, None)
        | (FullFileHashMode::Stream, Some(_))
        | (FullFileHashMode::Mmap, Some(_))
        | (FullFileHashMode::MmapRayon, Some(_)) => {
            unreachable!("full-file hash mode must match precomputed hash availability")
        }
    };

    if actual_hash != expected_hash {
        return Err(Error::HashMismatch {
            expected: expected_hash.to_hex(),
            actual: actual_hash.to_hex(),
        });
    }

    Ok(FullFileIntegrityResult {
        elapsed: started_at.elapsed(),
        read_elapsed,
        hash_elapsed,
        bytes: file_size,
        mode,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_file_hash_mode_matches_reconstruction_policy() {
        assert_eq!(
            select_full_file_hash_mode(1, true, 8),
            FullFileHashMode::Forwarded
        );
        assert_eq!(
            select_full_file_hash_mode(PARALLEL_FULL_HASH_MIN_BYTES - 1, false, 8),
            FullFileHashMode::Stream
        );
        assert_eq!(
            select_full_file_hash_mode(PARALLEL_FULL_HASH_MIN_BYTES, false, 8),
            FullFileHashMode::Mmap
        );
        assert_eq!(
            select_full_file_hash_mode(PARALLEL_FULL_HASH_MIN_BYTES, false, 1),
            FullFileHashMode::MmapRayon
        );
    }
}
