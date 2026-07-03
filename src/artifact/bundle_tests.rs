use super::*;
use crate::hash;

#[test]
fn test_build_and_read_bundle() {
    let chunk1_data = b"chunk one data for testing ".repeat(10);
    let chunk2_data = b"chunk two different data ".repeat(10);

    let hash1 = hash::hash_hex(&hash::hash_chunk(&chunk1_data));
    let hash2 = hash::hash_hex(&hash::hash_chunk(&chunk2_data));

    let mut builder = BundleBuilder::new(CompressionProfile::default());
    builder.add_chunk(hash1.clone(), chunk1_data.to_vec());
    builder.add_chunk(hash2.clone(), chunk2_data.to_vec());

    let (_bundle_id, bundle_data) = builder.build_with_id().unwrap();

    let mut reader = read_bundle(&bundle_data).unwrap();
    assert_eq!(reader.chunk_count(), 2);

    let read1 = reader.read_chunk_by_hash(&hash1).unwrap();
    let read2 = reader.read_chunk_by_hash(&hash2).unwrap();

    assert_eq!(read1, chunk1_data.to_vec());
    assert_eq!(read2, chunk2_data.to_vec());
}

#[test]
fn test_empty_bundle() {
    let builder = BundleBuilder::new(CompressionProfile::default());
    assert!(builder.is_empty());
    assert_eq!(builder.chunk_count(), 0);
    let bundle_data = builder.build().unwrap();
    let reader = read_bundle(&bundle_data).unwrap();
    assert_eq!(reader.chunk_count(), 0);
}

#[test]
fn test_single_chunk_bundle() {
    let data = b"single chunk test data ".repeat(20);
    let hash = hash::hash_hex(&hash::hash_chunk(&data));

    let mut builder = BundleBuilder::new(CompressionProfile::default());
    builder.add_chunk(hash.clone(), data.to_vec());

    let bundle_data = builder.build().unwrap();
    let mut reader = read_bundle(&bundle_data).unwrap();
    assert_eq!(reader.chunk_count(), 1);

    let result = reader.read_chunk(0).unwrap();
    assert_eq!(result, data.to_vec());
}

#[test]
fn test_verify_bundle() {
    let chunk1 = b"verify test chunk one ".repeat(10);
    let chunk2 = b"verify test chunk two ".repeat(10);

    let mut builder = BundleBuilder::new(CompressionProfile::default());
    builder.add_chunk(hash::hash_hex(&hash::hash_chunk(&chunk1)), chunk1.to_vec());
    builder.add_chunk(hash::hash_hex(&hash::hash_chunk(&chunk2)), chunk2.to_vec());

    let bundle_data = builder.build().unwrap();
    let mut reader = read_bundle(&bundle_data).unwrap();
    assert!(reader.verify().unwrap());
}

#[test]
fn test_build_and_read_bundle_compressed_stream() {
    let chunk1 = b"bundle compression test chunk one ".repeat(8);
    let chunk2 = b"bundle compression test chunk two ".repeat(8);

    let mut profile = CompressionProfile::default();
    profile.compression_unit = COMPRESSION_UNIT_BUNDLE.to_string();

    let mut builder = BundleBuilder::new(profile);
    builder.add_chunk(hash::hash_hex(&hash::hash_chunk(&chunk1)), chunk1.to_vec());
    builder.add_chunk(hash::hash_hex(&hash::hash_chunk(&chunk2)), chunk2.to_vec());

    let bundle_data = builder.build().unwrap();
    let mut reader = read_bundle(&bundle_data).unwrap();

    assert_eq!(reader.read_chunk(0).unwrap(), chunk1.to_vec());
    assert_eq!(reader.read_chunk(1).unwrap(), chunk2.to_vec());
}

#[test]
fn test_build_and_read_uncompressed_bundle_with_zstd_magic_prefix() {
    let chunk1 = vec![0x28, 0xB5, 0x2F, 0xFD, b'r', b'a', b'w', b'1'];
    let chunk2 = b"plain uncompressed payload".to_vec();

    let profile = CompressionProfile::for_preset(crate::manifest::CompressionPreset::Uncompressed);
    let mut builder = BundleBuilder::new(profile);
    builder.add_chunk(hash::hash_hex(&hash::hash_chunk(&chunk1)), chunk1.clone());
    builder.add_chunk(hash::hash_hex(&hash::hash_chunk(&chunk2)), chunk2.clone());

    let bundle_data = builder.build().unwrap();
    let mut reader = read_bundle(&bundle_data).unwrap();

    assert_eq!(reader.compression_unit(), COMPRESSION_UNIT_BUNDLE);
    assert_eq!(reader.read_chunk(0).unwrap(), chunk1);
    assert_eq!(reader.read_chunk(1).unwrap(), chunk2);
}

#[test]
fn test_bundle_id_deterministic() {
    let data1 = b"deterministic test ".repeat(15);
    let hash1 = hash::hash_hex(&hash::hash_chunk(&data1));

    let mut b1 = BundleBuilder::new(CompressionProfile::default());
    b1.add_chunk(hash1.clone(), data1.to_vec());
    let (id1, _) = b1.build_with_id().unwrap();

    let mut b2 = BundleBuilder::new(CompressionProfile::default());
    b2.add_chunk(hash1, data1.to_vec());
    let (id2, _) = b2.build_with_id().unwrap();

    assert_eq!(id1, id2);
}

#[test]
fn test_bundle_id_depends_on_final_bundle_bytes() {
    let d1 = b"chunk a ".repeat(10);
    let d2 = b"chunk b ".repeat(10);
    let h1 = hash::hash_hex(&hash::hash_chunk(&d1));
    let h2 = hash::hash_hex(&hash::hash_chunk(&d2));

    let mut b1 = BundleBuilder::new(CompressionProfile::default());
    b1.add_chunk(h1.clone(), d1.to_vec());
    b1.add_chunk(h2.clone(), d2.to_vec());
    let (id1, _) = b1.build_with_id().unwrap();

    let mut b2 = BundleBuilder::new(CompressionProfile::default());
    b2.add_chunk(h2, d2.to_vec());
    b2.add_chunk(h1, d1.to_vec());
    let (id2, _) = b2.build_with_id().unwrap();

    assert_ne!(id1, id2, "bundle ID should reflect final bundle bytes");
}

#[test]
fn test_invalid_magic() {
    let result = read_bundle(b"XXXX extra data to read");
    assert!(
        matches!(result, Err(crate::error::Error::InvalidBundle(_))),
        "should reject invalid magic"
    );
}
