use super::*;
use crate::artifact::integrity::hash_file;
use std::fs;

#[test]
fn hash_chunk_returns_a_64_digit_hex_digest() {
    let hash = hash_chunk(b"hello");
    let hex = hash_hex(&hash);
    assert_eq!(hex.len(), 64);
}

#[test]
fn chunk_hash_byte_roundtrip() {
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
fn hash_file_matches_blake3_of_the_contents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("content.tmp");
    fs::write(&path, b"test content").unwrap();
    let hash = hash_file(&path).unwrap();
    assert_eq!(hash, blake3::hash(b"test content"));
}
