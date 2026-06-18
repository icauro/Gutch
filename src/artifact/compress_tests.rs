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
