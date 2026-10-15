use crate::profile::ProfileInstant;
use std::cell::RefCell;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use crate::Error;
use crate::Result;
use crate::artifact::hash::ChunkHash;
use crate::progress::GutchOperationControl;

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
            Self::Forwarded => "forwarded",
            Self::Stream => "stream",
            Self::Mmap => "mmap",
            Self::MmapRayon => "mmap_rayon",
        }
    }
}

pub(crate) struct FullFileIntegrityResult {
    pub(crate) read_elapsed: Duration,
    pub(crate) hash_elapsed: Duration,
    pub(crate) bytes: u64,
}

// Prefer file-level parallelism when multiple files can occupy the pool.
// With one file, Rayon can use the same pool for its internal hash tree.
pub(crate) fn select_full_file_hash_mode(
    file_size: u64,
    has_precomputed_hash: bool,
    concurrent_file_count: usize,
) -> FullFileHashMode {
    if has_precomputed_hash {
        FullFileHashMode::Forwarded
    } else if file_size < PARALLEL_FULL_HASH_MIN_BYTES {
        FullFileHashMode::Stream
    } else if concurrent_file_count <= 1 {
        FullFileHashMode::MmapRayon
    } else {
        FullFileHashMode::Mmap
    }
}

/// Hashes arbitrary files without allocating in proportion to their size.
pub(crate) fn hash_file(path: &Path) -> Result<blake3::Hash> {
    let (hash, _, _) = hash_file_streaming(path, &GutchOperationControl::default())?;
    Ok(hash)
}

fn hash_file_streaming(
    path: &Path,
    control: &GutchOperationControl,
) -> Result<(blake3::Hash, Duration, Duration)> {
    control.checkpoint()?;
    let mut input = File::open(path)?;
    with_hash_read_buffer(|buffer| {
        let mut hasher = blake3::Hasher::new();
        let mut read_elapsed = Duration::ZERO;
        let mut hash_elapsed = Duration::ZERO;
        loop {
            control.checkpoint()?;
            let read_started_at = ProfileInstant::detailed(control.profiler());
            let read = input.read(buffer)?;
            read_elapsed += read_started_at.elapsed();
            if read == 0 {
                break;
            }
            let hash_started_at = ProfileInstant::detailed(control.profiler());
            hasher.update(&buffer[..read]);
            hash_elapsed += hash_started_at.elapsed();
        }
        Ok((hasher.finalize(), read_elapsed, hash_elapsed))
    })
}

/// Reuses bounded scratch storage for sequential file and artifact hashing.
pub(crate) fn with_hash_read_buffer<T>(hash: impl FnOnce(&mut [u8]) -> Result<T>) -> Result<T> {
    STREAM_BUFFER.with(|buffer| hash(&mut buffer.borrow_mut()))
}

pub(crate) fn verify_full_file_integrity(
    path: &Path,
    file_size: u64,
    expected_hash: ChunkHash,
    precomputed_hash: Option<ChunkHash>,
    concurrent_file_count: usize,
    control: &GutchOperationControl,
) -> Result<FullFileIntegrityResult> {
    control.checkpoint()?;
    let mode =
        select_full_file_hash_mode(file_size, precomputed_hash.is_some(), concurrent_file_count);
    let mut read_elapsed = Duration::ZERO;
    let mut hash_elapsed = Duration::ZERO;

    let actual_hash = match (mode, precomputed_hash) {
        (FullFileHashMode::Forwarded, Some(hash)) => hash,
        (FullFileHashMode::MmapRayon, None) => {
            let hash_started_at = ProfileInstant::detailed(control.profiler());
            let mut hasher = blake3::Hasher::new();
            hasher.update_mmap_rayon(path)?;
            hash_elapsed = hash_started_at.elapsed();
            ChunkHash::from_blake3(hasher.finalize())
        }
        (FullFileHashMode::Mmap, None) => {
            let hash_started_at = ProfileInstant::detailed(control.profiler());
            let mut hasher = blake3::Hasher::new();
            hasher.update_mmap(path)?;
            hash_elapsed = hash_started_at.elapsed();
            ChunkHash::from_blake3(hasher.finalize())
        }
        (FullFileHashMode::Stream, None) => {
            let (hash, measured_read, measured_hash) = hash_file_streaming(path, control)?;
            read_elapsed = measured_read;
            hash_elapsed = measured_hash;
            ChunkHash::from_blake3(hash)
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
        read_elapsed,
        hash_elapsed,
        bytes: file_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_hash_handles_empty_files_and_buffer_boundaries() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("content");
        for size in [
            0,
            1,
            STREAM_BUFFER_BYTES - 1,
            STREAM_BUFFER_BYTES,
            STREAM_BUFFER_BYTES + 37,
        ] {
            let bytes: Vec<_> = (0..size).map(|index| (index % 251) as u8).collect();
            std::fs::write(&path, &bytes).unwrap();
            assert_eq!(hash_file(&path).unwrap(), blake3::hash(&bytes));
        }
        assert!(hash_file(&directory.path().join("missing")).is_err());
    }

    #[test]
    fn streaming_hash_observes_cancellation_between_reads() {
        #[derive(Debug)]
        struct CancelAfterRead(std::sync::atomic::AtomicUsize);
        impl crate::progress::GutchCancellationHook for CancelAfterRead {
            fn is_cancelled(&self) -> bool {
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 2
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("content");
        std::fs::write(&path, vec![17; STREAM_BUFFER_BYTES * 2]).unwrap();
        let control = GutchOperationControl::default().with_cancellation(std::sync::Arc::new(
            CancelAfterRead(std::sync::atomic::AtomicUsize::new(0)),
        ));
        assert!(matches!(
            hash_file_streaming(&path, &control),
            Err(Error::Cancelled)
        ));
    }

    #[test]
    fn full_file_verification_rejects_corruption_in_stream_and_mmap_modes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("content");
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap();
        for size in [37, PARALLEL_FULL_HASH_MIN_BYTES as usize] {
            let mut bytes = vec![17; size];
            let expected = ChunkHash::from_blake3(blake3::hash(&bytes));
            std::fs::write(&path, &bytes).unwrap();
            for concurrent_files in [1, 4] {
                pool.install(|| {
                    verify_full_file_integrity(
                        &path,
                        size as u64,
                        expected,
                        None,
                        concurrent_files,
                        &GutchOperationControl::default(),
                    )
                })
                .unwrap();
            }
            bytes[size - 1] ^= 1;
            std::fs::write(&path, &bytes).unwrap();
            for concurrent_files in [1, 4] {
                let result = pool.install(|| {
                    verify_full_file_integrity(
                        &path,
                        size as u64,
                        expected,
                        None,
                        concurrent_files,
                        &GutchOperationControl::default(),
                    )
                });
                assert!(matches!(result, Err(Error::HashMismatch { .. })));
            }
        }
    }

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
