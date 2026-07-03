use super::*;

#[test]
fn test_hash_chunk() {
    let hash = hash_chunk(b"hello");
    let hex = hash_hex(&hash);
    assert_eq!(hex.len(), 64);
}

#[test]
fn test_hex_decode_roundtrip() {
    let data = b"test data for hex encoding";
    let hash = hash_chunk(data);
    let hex = hash_hex(&hash);
    let bytes = hex_decode(&hex).unwrap();
    assert_eq!(bytes.as_slice(), hash.as_bytes());
}

#[test]
fn test_hex_decode_invalid_length() {
    let result = hex_decode("abcde");
    assert!(result.is_err());
}

#[test]
fn test_chunk_hash_byte_roundtrip() {
    let hash = ChunkHash::from_blake3(hash_chunk(b"hello"));
    let reparsed = ChunkHash::from_bytes(*hash.as_bytes());
    assert_eq!(hash, reparsed);
}

#[test]
fn test_hash_file() {
    let dir = std::env::temp_dir();
    let path = dir.join("gutch_test_hash_file.tmp");
    std::fs::write(&path, b"test content").unwrap();
    let hash = hash_file(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(hash, blake3::hash(b"test content"));
}
