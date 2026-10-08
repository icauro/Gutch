use super::*;
use crate::artifact::profiles::CompressionProfile;

#[test]
fn test_compress_decompress_roundtrip() {
    let original = b"hello world, this is some test data for compression ".repeat(100);
    let profile = CompressionProfile::default();
    let compressed = compress_with_profile(&original, &profile).unwrap();
    assert!(
        compressed.len() < original.len(),
        "zstd should compress repetitive data"
    );

    let decompressed = decompress_data(&compressed, CompressionKind::Zstd).unwrap();
    assert_eq!(original.as_slice(), decompressed.as_slice());
}

#[test]
fn test_compress_small_data() {
    let original = b"hi";
    let profile = CompressionProfile::default();
    let compressed = compress_with_profile(original, &profile).unwrap();
    let decompressed = decompress_data(&compressed, CompressionKind::Zstd).unwrap();
    assert_eq!(original.as_slice(), decompressed.as_slice());
}

#[test]
fn test_compress_segments_roundtrip() {
    let first = b"first segment repeats ".repeat(100);
    let second = b"second segment repeats ".repeat(100);
    let third = b"third segment repeats ".repeat(100);
    let raw_size = (first.len() + second.len() + third.len()) as u64;
    let compressed = compress_segments_with_profile(
        &[first.as_slice(), second.as_slice(), third.as_slice()],
        raw_size,
        &CompressionProfile::default(),
    )
    .unwrap();

    let decompressed = decompress_data(&compressed, CompressionKind::Zstd).unwrap();
    let mut expected = Vec::new();
    expected.extend_from_slice(&first);
    expected.extend_from_slice(&second);
    expected.extend_from_slice(&third);
    assert_eq!(decompressed, expected);
}

#[test]
fn test_different_levels() {
    let original = b"some data that repeats again and again ".repeat(200);
    let c1 = compress_with_profile(
        &original,
        &CompressionProfile::for_algorithm(CompressionKind::Zstd, Some(1)),
    )
    .unwrap();
    let c9 = compress_with_profile(
        &original,
        &CompressionProfile::for_algorithm(CompressionKind::Zstd, Some(9)),
    )
    .unwrap();
    let d1 = decompress_data(&c1, CompressionKind::Zstd).unwrap();
    let d9 = decompress_data(&c9, CompressionKind::Zstd).unwrap();
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
    let decompressed = decompress_data(&original, CompressionKind::None).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn test_lz4_roundtrip() {
    let original = b"lz4 compression adapter data ".repeat(200);
    let profile = CompressionProfile::for_algorithm(CompressionKind::Lz4, Some(9));
    let compressed = compress_with_profile(&original, &profile).unwrap();
    let decompressed = decompress_data(&compressed, CompressionKind::Lz4).unwrap();

    assert_eq!(decompressed, original);
    assert!(compressed.len() < original.len());
}

#[test]
fn test_compression_levels_are_strictly_validated() {
    let zstd_invalid = CompressionProfile::for_algorithm(CompressionKind::Zstd, Some(23));
    let lz4_invalid = CompressionProfile::for_algorithm(CompressionKind::Lz4, Some(10));
    let none_invalid = CompressionProfile::for_algorithm(CompressionKind::None, Some(1));
    let none_zero = CompressionProfile::for_algorithm(CompressionKind::None, Some(0));

    assert!(validate_profile(&zstd_invalid).is_err());
    assert!(validate_profile(&lz4_invalid).is_err());
    assert!(validate_profile(&none_invalid).is_err());
    assert!(validate_profile(&none_zero).is_err());
}
