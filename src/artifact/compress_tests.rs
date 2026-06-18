use super::*;
use crate::manifest::CompressionProfile;

#[test]
fn test_compress_decompress_roundtrip() {
    let original = b"hello world, this is some test data for compression ".repeat(100);
    let profile = CompressionProfile::default();
    let compressed = compress_with_level(&original, profile.zstd_level).unwrap();
    assert!(
        compressed.len() < original.len(),
        "zstd should compress repetitive data"
    );

    let decompressed = decompress_data(&compressed, "zstd").unwrap();
    assert_eq!(original.as_slice(), decompressed.as_slice());
}

#[test]
fn test_compress_small_data() {
    let original = b"hi";
    let profile = CompressionProfile::default();
    let compressed = compress_with_level(original, profile.zstd_level).unwrap();
    let decompressed = decompress_data(&compressed, "zstd").unwrap();
    assert_eq!(original.as_slice(), decompressed.as_slice());
}

#[test]
fn test_compress_segments_roundtrip() {
    let first = b"first segment repeats ".repeat(100);
    let second = b"second segment repeats ".repeat(100);
    let third = b"third segment repeats ".repeat(100);
    let raw_size = (first.len() + second.len() + third.len()) as u64;
    let compressed = compress_segments_with_level(
        [first.as_slice(), second.as_slice(), third.as_slice()],
        raw_size,
        CompressionProfile::default().zstd_level,
    )
    .unwrap();

    let decompressed = decompress_data(&compressed, "zstd").unwrap();
    let mut expected = Vec::new();
    expected.extend_from_slice(&first);
    expected.extend_from_slice(&second);
    expected.extend_from_slice(&third);
    assert_eq!(decompressed, expected);
}

#[test]
fn test_different_levels() {
    let original = b"some data that repeats again and again ".repeat(200);
    let c1 = compress_with_level(&original, 1).unwrap();
    let c9 = compress_with_level(&original, 9).unwrap();
    let d1 = decompress_data(&c1, "zstd").unwrap();
    let d9 = decompress_data(&c9, "zstd").unwrap();
    assert_eq!(d1, original);
    assert_eq!(d9, original);
    assert!(
        c9.len() <= c1.len(),
        "higher level should compress as well or better"
    );
}

#[test]
fn test_uncompressed_data_roundtrips_without_magic_sniffing() {
    let original = [0x28, 0xB5, 0x2F, 0xFD, b'r', b'a', b'w', b'!'];
    let decompressed = decompress_data(&original, "none").unwrap();
    assert_eq!(decompressed, original);
}
