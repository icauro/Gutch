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
    let (streaming, timings) = chunk_file_streaming(&path, &profile).unwrap();

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
