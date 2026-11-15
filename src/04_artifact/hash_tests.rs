use super::*;
use crate::artifact::integrity::hash_file;
use std::env::temp_dir;
use std::fs;

#[test]
fn test_hash_chunk() {
    let hash = hash_chunk(b"hello");
    let hex = hash_hex(&hash);
    assert_eq!(hex.len(), 64);
}

#[test]
fn test_chunk_hash_byte_roundtrip() {
    let hash = ChunkHash::of(b"hello");
    let reparsed = ChunkHash::from_bytes(*hash.as_bytes());
    assert_eq!(hash, reparsed);
    let expected = blake3::hash(b"hello").to_hex().to_string();
    assert_eq!(hash.to_string(), expected);
    assert_eq!(
        serde_json::to_string(&hash).unwrap(),
        format!("\"{expected}\"")
    );
}

#[test]
fn test_hash_file() {
    let dir = temp_dir();
    let path = dir.join("gutch_test_hash_file.tmp");
    fs::write(&path, b"test content").unwrap();
    let hash = hash_file(&path).unwrap();
    let _ = fs::remove_file(&path);
    assert_eq!(hash, blake3::hash(b"test content"));
}
