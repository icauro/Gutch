use super::*;
use crate::manifest::ChunkSizingProfile;
use crate::test_util;

#[test]
fn test_chunk_bytes_basic() {
    let data =
        b"hello world here is some test data that should be longer than the minimum chunk size "
            .repeat(1000);
    let profile = ChunkSizingProfile::default();
    let results = chunk_bytes(&data, &profile);
    assert!(!results.is_empty());
    for r in &results {
        assert!(!r.data.is_empty());
        assert!(r.data.len() as u32 <= profile.max_chunk_size_bytes());
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
    assert_eq!(results[0].data, b"short");
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
    let (scan, timings) = chunk_file_streaming_with_file_hash(&path, &profile).unwrap();
    let streaming = scan.chunks;

    assert_eq!(in_memory.len(), streaming.len());
    assert_eq!(timings.boundary_scan_bytes, data.len() as u64);
    assert_eq!(timings.hash_judgment_bytes, data.len() as u64);

    for (left, right) in in_memory.iter().zip(streaming.iter()) {
        assert_eq!(left.offset, right.offset);
        assert_eq!(left.hash, right.hash);
        assert_eq!(left.data, right.data);
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
    let (scan, timings) = chunk_file_streaming_with_file_hash(&path, &profile).unwrap();

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
fn test_parallel_chunk_hashing_matches_streaming_results() {
    let dir = test_util::temp_path("gutch_parallel_chunk_hashing");
    test_util::recreate_dir(&dir);
    let path = dir.join("sample.bin");
    let data = b"parallel chunk hashing payload with varied boundaries ".repeat(150_000);
    std::fs::write(&path, &data).unwrap();

    let profile = ChunkSizingProfile::default();
    let serial = chunk_file_streaming_with_file_hash(&path, &profile).unwrap();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    let parallel = chunk_file_streaming_with_file_hash_and_control_on_pool(
        &path,
        &profile,
        &crate::GutchOperationControl::default(),
        &pool,
    )
    .unwrap();

    assert_eq!(serial.0.file_size, parallel.0.file_size);
    assert_eq!(serial.0.full_file_hash, parallel.0.full_file_hash);
    assert_eq!(serial.0.chunks.len(), parallel.0.chunks.len());
    assert_eq!(serial.1.hash_judgment_bytes, parallel.1.hash_judgment_bytes);
    for (serial_chunk, parallel_chunk) in serial.0.chunks.iter().zip(parallel.0.chunks.iter()) {
        assert_eq!(serial_chunk.offset, parallel_chunk.offset);
        assert_eq!(serial_chunk.hash, parallel_chunk.hash);
        assert_eq!(serial_chunk.data, parallel_chunk.data);
    }

    let _ = std::fs::remove_dir_all(&dir);
}
