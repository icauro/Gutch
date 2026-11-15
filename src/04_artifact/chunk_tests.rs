use super::*;
use crate::artifact::profiles::ChunkSizingProfile;
use crate::test_util;

#[test]
fn inventory_selected_scan_matches_chunk_boundaries_for_every_preset() {
    use clap::ValueEnum;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source");
    let mut seed = 17u64;
    for size in [
        0usize, 1, 127, 128, 511, 512, 32767, 32768, 524287, 524288, 524289, 1048575, 1048576,
        1048577,
    ] {
        let data: Vec<u8> = (0..size)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed as u8
            })
            .collect();
        std::fs::write(&path, &data).unwrap();
        for preset in crate::artifact::profiles::ChunkSizingPreset::value_variants() {
            let profile = ChunkSizingProfile::for_preset(*preset);
            let expected = chunk_bytes(&data, &profile);
            let (actual, _) =
                scan_file_chunks(&path, size as u64, &profile, &Default::default()).unwrap();
            assert_eq!(actual.full_file_hash, hash_hex(&blake3::hash(&data)));
            assert_eq!(expected.len(), actual.chunks.len());
            for (left, right) in expected.iter().zip(&actual.chunks) {
                assert_eq!(
                    (left.offset, left.size, left.hash),
                    (right.offset, right.size, right.hash)
                );
            }
        }
    }
}

#[test]
fn small_file_scan_rejects_growth_and_truncation_since_inventory() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source");
    std::fs::write(&path, b"bytes").unwrap();
    for inventory_size in [0, 4, 6, 1024] {
        assert!(
            scan_file_chunks(
                &path,
                inventory_size,
                &Default::default(),
                &Default::default()
            )
            .is_err()
        );
    }
}

#[test]
fn test_chunk_bytes_basic() {
    let data =
        b"hello world here is some test data that should be longer than the minimum chunk size "
            .repeat(1000);
    let profile = ChunkSizingProfile::default();
    let results = chunk_bytes(&data, &profile);
    assert!(!results.is_empty());
    for r in &results {
        assert!(r.size > 0);
        assert!(r.size as u32 <= profile.max_chunk_size_bytes());
    }
}

#[test]
fn test_chunk_bytes_empty() {
    let data: &[u8] = &[];
    let profile = ChunkSizingProfile::default();
    let results = chunk_bytes(data, &profile);
    assert!(results.is_empty());
}

#[test]
fn test_chunk_bytes_small_data() {
    let data = b"short";
    let profile = ChunkSizingProfile::default();
    let results = chunk_bytes(data, &profile);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].size, 5);
}

#[test]
fn test_hash_hex() {
    let hash = blake3::hash(b"test");
    let hex = hash_hex(&hash);
    assert_eq!(hex.len(), 64);
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_chunks_have_increasing_offsets() {
    let data = b"x".repeat(1024 * 64);
    let profile = ChunkSizingProfile::default();
    let results = chunk_bytes(&data, &profile);
    for w in results.windows(2) {
        assert!(w[1].offset > w[0].offset);
    }
}

#[test]
fn test_chunk_file_streaming_matches_in_memory_chunking() {
    let dir = test_util::temp_path("gutch_chunk_streaming_match");
    test_util::recreate_dir(&dir);
    let path = dir.join("sample.bin");
    let data = b"streaming chunk comparison payload ".repeat(4000);
    std::fs::write(&path, &data).unwrap();

    let profile = ChunkSizingProfile::default();
    let in_memory = chunk_bytes(&data, &profile);
    let (scan, timings) = scan_file_chunks(
        &path,
        data.len() as u64,
        &profile,
        &crate::progress::OperationControl::default(),
    )
    .unwrap();
    let streaming = scan.chunks;

    assert_eq!(in_memory.len(), streaming.len());
    assert_eq!(timings.boundary_scan_bytes, data.len() as u64);
    assert_eq!(timings.hash_judgment_bytes, data.len() as u64);

    for (left, right) in in_memory.iter().zip(streaming.iter()) {
        assert_eq!(left.offset, right.offset);
        assert_eq!(left.hash, right.hash);
        assert_eq!(left.size, right.size);
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_chunk_file_streaming_with_file_hash_matches_raw_file_hash() {
    let dir = test_util::temp_path("gutch_chunk_streaming_file_hash");
    test_util::recreate_dir(&dir);
    let path = dir.join("sample.bin");
    let data = b"streaming full file hash payload ".repeat(4000);
    std::fs::write(&path, &data).unwrap();

    let profile = ChunkSizingProfile::default();
    let (scan, timings) = scan_file_chunks(
        &path,
        data.len() as u64,
        &profile,
        &crate::progress::OperationControl::default(),
    )
    .unwrap();

    assert_eq!(scan.file_size, data.len() as u64);
    assert_eq!(
        scan.full_file_hash,
        hash_hex(&blake3::hash(data.as_slice()))
    );
    assert_eq!(timings.boundary_scan_bytes, data.len() as u64);
    assert_eq!(timings.hash_judgment_bytes, data.len() as u64);
    assert!(!scan.chunks.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn repeated_chunk_scans_have_identical_boundaries() {
    let dir = test_util::temp_path("gutch_parallel_chunk_hashing");
    test_util::recreate_dir(&dir);
    let path = dir.join("sample.bin");
    let data = b"parallel chunk hashing payload with varied boundaries ".repeat(150_000);
    std::fs::write(&path, &data).unwrap();

    let profile = ChunkSizingProfile::default();
    let serial = scan_file_chunks(
        &path,
        data.len() as u64,
        &profile,
        &crate::progress::OperationControl::default(),
    )
    .unwrap();
    let parallel = scan_file_chunks(
        &path,
        data.len() as u64,
        &profile,
        &crate::progress::OperationControl::default(),
    )
    .unwrap();

    assert_eq!(serial.0.file_size, parallel.0.file_size);
    assert_eq!(serial.0.full_file_hash, parallel.0.full_file_hash);
    assert_eq!(serial.0.chunks.len(), parallel.0.chunks.len());
    assert_eq!(serial.1.hash_judgment_bytes, parallel.1.hash_judgment_bytes);
    for (serial_chunk, parallel_chunk) in serial.0.chunks.iter().zip(parallel.0.chunks.iter()) {
        assert_eq!(serial_chunk.offset, parallel_chunk.offset);
        assert_eq!(serial_chunk.hash, parallel_chunk.hash);
        assert_eq!(serial_chunk.size, parallel_chunk.size);
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn jc_capture_scan_matches_kernel_across_small_and_streaming_paths() {
    use crate::artifact::jc::{JumpChunker, Parameters};
    use crate::artifact::profiles::{ChunkSizingPreset, Chunker};
    use clap::ValueEnum;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source");
    let mut seed = 917u64;
    let data: Vec<u8> = (0..2_100_000)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as u8
        })
        .collect();
    for size in [0, 1, 32768, 524288, 524289, 1048576, data.len()] {
        let bytes = &data[..size];
        std::fs::write(&path, bytes).unwrap();
        for preset in ChunkSizingPreset::value_variants() {
            let profile = ChunkSizingProfile::for_preset(*preset).with_chunker(Chunker::Jc);
            let boundary = JumpChunker::new(
                profile.min_chunk_size_bytes() as usize,
                profile.avg_chunk_size_bytes() as usize,
                profile.max_chunk_size_bytes() as usize,
                Parameters::Plakar,
            );
            let (scan, _) =
                scan_file_chunks(&path, size as u64, &profile, &Default::default()).unwrap();
            assert_eq!(scan.full_file_hash, hash_hex(&blake3::hash(bytes)));
            let mut offset = 0;
            for chunk in scan.chunks {
                let length = boundary.cut(&bytes[offset..]);
                assert_eq!((chunk.offset, chunk.size), (offset as u64, length as u64));
                assert_eq!(chunk.hash, blake3::hash(&bytes[offset..offset + length]));
                offset += length;
            }
            assert_eq!(offset, size);
        }
    }
}

#[test]
fn streaming_scan_rejects_source_that_differs_from_inventory_size() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source");
    std::fs::write(&path, vec![7u8; 3 * 1024 * 1024]).unwrap();
    let profile = ChunkSizingProfile::default();
    let stale_inventory_size = 2 * 1024 * 1024;
    assert!(scan_file_chunks(&path, stale_inventory_size, &profile, &Default::default()).is_err());
}
