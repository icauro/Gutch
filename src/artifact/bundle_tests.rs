use super::*;
use crate::repo::artifacts::verify_bundle_identity_file;

fn profile() -> CompressionProfile {
    CompressionProfile::default()
}
use crate::artifact::hash;
use crate::artifact::hash::ChunkHash;

#[test]
fn test_build_and_read_bundle() {
    let chunk1_data = b"chunk one data for testing ".repeat(10);
    let chunk2_data = b"chunk two different data ".repeat(10);

    let hash1 = ChunkHash::from_blake3(hash::hash_chunk(&chunk1_data));
    let hash2 = ChunkHash::from_blake3(hash::hash_chunk(&chunk2_data));

    let mut builder = BundleBuilder::new(CompressionProfile::default());
    builder.add_chunk(hash1, chunk1_data.to_vec());
    builder.add_chunk(hash2, chunk2_data.to_vec());

    let (_bundle_id, bundle_data) = builder.build_with_id().unwrap();

    let mut reader = read_bundle(&bundle_data, &profile()).unwrap();
    assert_eq!(reader.chunk_count(), 2);

    let read1 = reader.read_chunk_by_hash(&hash1.to_string()).unwrap();
    let read2 = reader.read_chunk_by_hash(&hash2.to_string()).unwrap();

    assert_eq!(read1, chunk1_data.to_vec());
    assert_eq!(read2, chunk2_data.to_vec());
}

#[test]
fn test_empty_bundle() {
    let builder = BundleBuilder::new(CompressionProfile::default());
    assert!(builder.is_empty());
    assert_eq!(builder.chunk_count(), 0);
    let bundle_data = builder.build().unwrap();
    let reader = read_bundle(&bundle_data, &profile()).unwrap();
    assert_eq!(reader.chunk_count(), 0);
}

#[test]
fn test_empty_lz4_bundle() {
    let profile = CompressionProfile::for_algorithm(crate::manifest::CompressionKind::Lz4, None);
    let builder = BundleBuilder::new(profile.clone());
    let bundle_data = builder.build().unwrap();
    let mut reader = read_bundle(&bundle_data, &profile).unwrap();
    assert_eq!(reader.chunk_count(), 0);
    assert!(reader.verify().unwrap());
}

#[test]
fn test_single_chunk_bundle() {
    let data = b"single chunk test data ".repeat(20);
    let hash = ChunkHash::from_blake3(hash::hash_chunk(&data));

    let mut builder = BundleBuilder::new(CompressionProfile::default());
    builder.add_chunk(hash, data.to_vec());

    let bundle_data = builder.build().unwrap();
    let mut reader = read_bundle(&bundle_data, &profile()).unwrap();
    assert_eq!(reader.chunk_count(), 1);

    let result = reader.read_chunk(0).unwrap();
    assert_eq!(result, data.to_vec());
}

#[test]
fn test_verify_bundle() {
    let chunk1 = b"verify test chunk one ".repeat(10);
    let chunk2 = b"verify test chunk two ".repeat(10);

    let mut builder = BundleBuilder::new(CompressionProfile::default());
    builder.add_chunk(
        ChunkHash::from_blake3(hash::hash_chunk(&chunk1)),
        chunk1.to_vec(),
    );
    builder.add_chunk(
        ChunkHash::from_blake3(hash::hash_chunk(&chunk2)),
        chunk2.to_vec(),
    );

    let bundle_data = builder.build().unwrap();
    let mut reader = read_bundle(&bundle_data, &profile()).unwrap();
    assert!(reader.verify().unwrap());
}

#[test]
fn test_build_and_read_bundle_compressed_payload() {
    let chunk1 = b"bundle compression test chunk one ".repeat(8);
    let chunk2 = b"bundle compression test chunk two ".repeat(8);

    let profile = CompressionProfile {
        compression_unit: CompressionUnit::Bundle,
        ..CompressionProfile::default()
    };

    let mut builder = BundleBuilder::new(profile.clone());
    builder.add_chunk(
        ChunkHash::from_blake3(hash::hash_chunk(&chunk1)),
        chunk1.to_vec(),
    );
    builder.add_chunk(
        ChunkHash::from_blake3(hash::hash_chunk(&chunk2)),
        chunk2.to_vec(),
    );

    let bundle_data = builder.build().unwrap();
    let mut reader = read_bundle(&bundle_data, &profile).unwrap();

    let payload = &bundle_data[reader.data_start as usize..reader.payload_end as usize];
    let frame_size = zstd::zstd_safe::find_frame_compressed_size(payload).unwrap();
    assert_eq!(
        frame_size,
        payload.len(),
        "a bundled payload must be exactly one zstd frame"
    );
    assert!(
        reader
            .entries()
            .iter()
            .all(|entry| entry.compressed_size > 0)
    );
    assert_eq!(reader.read_chunk(0).unwrap(), chunk1.to_vec());
    assert_eq!(reader.read_chunk(1).unwrap(), chunk2.to_vec());

    let staged = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(staged.path(), &bundle_data).unwrap();
    let mut file_reader =
        BundleReader::open(std::fs::File::open(staged.path()).unwrap(), &profile).unwrap();
    let profiler = crate::profile::GutchProfiler::new(crate::profile::GutchProfileMode::Detailed);
    let (read, metrics) = file_reader
        .read_bundle_payload_with_profiler(Some(&profiler))
        .unwrap();
    let second = &read[reader.entries()[1].raw_offset as usize
        ..(reader.entries()[1].raw_offset + reader.entries()[1].raw_size) as usize];
    assert_eq!(second, chunk2.as_slice());
    assert_eq!(metrics.raw_bytes, (chunk1.len() + chunk2.len()) as u64);
    let staging = tempfile::tempdir().unwrap();
    let capture_profiler =
        crate::profile::GutchProfiler::new(crate::profile::GutchProfileMode::Detailed);
    builder
        .write_to_staging(staging.path(), Some(&capture_profiler))
        .unwrap();
    let trace_path = staging.path().join("trace.json");
    capture_profiler.write_trace(&trace_path).unwrap();
    let trace: serde_json::Value =
        serde_json::from_slice(&std::fs::read(trace_path).unwrap()).unwrap();
    assert!(
        trace["traceEvents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event["ph"] == "X" || event["ph"] == "M")
    );
    let spans = capture_profiler.summary().spans;
    for name in ["capture.bundle.flush", "capture.bundle.sync"] {
        assert_eq!(
            spans
                .iter()
                .find(|span| span.name == name)
                .map(|span| span.count),
            Some(1),
            "missing post-finalization marker {name}"
        );
    }
}

#[test]
fn test_stream_bundle_chunks_only_delivers_required_chunks() {
    let chunks = [
        b"streamed chunk one".to_vec(),
        b"streamed chunk two".repeat(100),
        b"small".to_vec(),
        b"larger again".repeat(200),
    ];
    let hashes = chunks
        .iter()
        .map(|chunk| ChunkHash::from_blake3(hash::hash_chunk(chunk)))
        .collect::<Vec<_>>();
    let profile = CompressionProfile {
        compression_unit: CompressionUnit::Bundle,
        ..CompressionProfile::default()
    };
    let mut builder = BundleBuilder::new(profile.clone());
    for (hash, chunk) in hashes.iter().zip(&chunks) {
        builder.add_chunk(*hash, chunk.clone());
    }

    let bundle_data = builder.build().unwrap();
    let mut reader = read_bundle(&bundle_data, &profile).unwrap();
    let mut delivered = Vec::new();
    let metrics = reader
        .stream_bundle_chunks_with_profiler(None, &[false, true, true, true], |index, chunk| {
            delivered.push((index, chunk.to_vec()));
            Ok(())
        })
        .unwrap();

    assert_eq!(
        delivered,
        vec![(1, chunks[1].clone()), (2, chunks[2].clone()), (3, chunks[3].clone())]
    );
    assert_eq!(
        metrics.raw_bytes,
        chunks.iter().map(|chunk| chunk.len() as u64).sum::<u64>()
    );
}

#[test]
fn test_build_and_read_uncompressed_bundle_with_zstd_magic_prefix() {
    let chunk1 = vec![0x28, 0xB5, 0x2F, 0xFD, b'r', b'a', b'w', b'1'];
    let chunk2 = b"plain uncompressed payload".to_vec();

    let profile = CompressionProfile::for_algorithm(crate::manifest::CompressionKind::None, None);
    let mut builder = BundleBuilder::new(profile.clone());
    builder.add_chunk(
        ChunkHash::from_blake3(hash::hash_chunk(&chunk1)),
        chunk1.clone(),
    );
    builder.add_chunk(
        ChunkHash::from_blake3(hash::hash_chunk(&chunk2)),
        chunk2.clone(),
    );

    let bundle_data = builder.build().unwrap();
    let mut reader = read_bundle(&bundle_data, &profile).unwrap();

    assert_eq!(reader.compression_unit(), CompressionUnit::Bundle);
    assert_eq!(reader.read_chunk(0).unwrap(), chunk1);
    assert_eq!(reader.read_chunk(1).unwrap(), chunk2);
}

#[test]
fn test_bundle_id_deterministic() {
    let data1 = b"deterministic test ".repeat(15);
    let hash1 = ChunkHash::from_blake3(hash::hash_chunk(&data1));

    let mut b1 = BundleBuilder::new(CompressionProfile::default());
    b1.add_chunk(hash1, data1.to_vec());
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
    let h1 = ChunkHash::from_blake3(hash::hash_chunk(&d1));
    let h2 = ChunkHash::from_blake3(hash::hash_chunk(&d2));

    let mut b1 = BundleBuilder::new(CompressionProfile::default());
    b1.add_chunk(h1, d1.to_vec());
    b1.add_chunk(h2, d2.to_vec());
    let (id1, _) = b1.build_with_id().unwrap();

    let mut b2 = BundleBuilder::new(CompressionProfile::default());
    b2.add_chunk(h2, d2.to_vec());
    b2.add_chunk(h1, d1.to_vec());
    let (id2, _) = b2.build_with_id().unwrap();

    assert_ne!(id1, id2, "bundle ID should reflect final bundle bytes");
}

#[test]
fn test_invalid_bundle_table() {
    let result = read_bundle(b"XXXX extra data to read", &profile());
    assert!(
        matches!(result, Err(crate::error::Error::InvalidBundle(_))),
        "should reject invalid magic"
    );
}

fn test_bundle_with_id() -> (String, Vec<u8>) {
    let data = b"identity test chunk data ".repeat(20);
    let hash = ChunkHash::from_blake3(hash::hash_chunk(&data));
    let mut builder = BundleBuilder::new(CompressionProfile::default());
    builder.add_chunk(hash, data.to_vec());
    builder.build_with_id().unwrap()
}

fn write_named_bundle(dir: &std::path::Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn test_bundle_identity_accepts_intact_file_and_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let (bundle_id, bundle_data) = test_bundle_with_id();
    verify_bundle_identity_bytes(&bundle_id, &bundle_data).unwrap();
    let path = write_named_bundle(temp.path(), &bundle_id, &bundle_data);
    verify_bundle_identity_file(&path, &bundle_id).unwrap();
}

#[test]
fn invalid_download_is_not_promoted_over_a_valid_cached_bundle() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let (bundle_id, bundle_data) = test_bundle_with_id();
    let path = write_named_bundle(temp.path(), &bundle_id, &bundle_data);
    let mut corrupt = bundle_data.clone();
    corrupt[0] ^= 0xff;
    let result = crate::filesystem::write_atomic_with(&path, |file| {
        file.write_all(&corrupt)?;
        verify_bundle_identity_reader(file, &bundle_id)
    });
    assert!(matches!(result, Err(crate::Error::HashMismatch { .. })));
    assert_eq!(std::fs::read(&path).unwrap(), bundle_data);
    crate::filesystem::write_atomic_with(&path, |file| {
        file.write_all(&bundle_data)?;
        verify_bundle_identity_reader(file, &bundle_id)
    })
    .unwrap();
    verify_bundle_identity_file(&path, &bundle_id).unwrap();
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn test_bundle_identity_rejects_corrupt_filename() {
    let temp = tempfile::tempdir().unwrap();
    let (bundle_id, bundle_data) = test_bundle_with_id();
    // Valid hex that names nothing: the path-derived ID must match.
    let other_id = "0".repeat(64);
    assert_ne!(other_id, bundle_id);
    let path = write_named_bundle(temp.path(), &other_id, &bundle_data);
    assert!(verify_bundle_identity_file(&path, &bundle_id).is_err());
    // A non-hex filename is rejected before any hashing.
    let path = write_named_bundle(temp.path(), "not-a-bundle-id", &bundle_data);
    assert!(verify_bundle_identity_file(&path, &bundle_id).is_err());
}

#[test]
fn test_bundle_identity_rejects_corrupt_trailing_id() {
    let (bundle_id, mut bundle_data) = test_bundle_with_id();
    let last = bundle_data.len() - 1;
    bundle_data[last] = if bundle_data[last] == b'0' {
        b'1'
    } else {
        b'0'
    };
    assert!(verify_bundle_identity_bytes(&bundle_id, &bundle_data).is_err());

    let temp = tempfile::tempdir().unwrap();
    let path = write_named_bundle(temp.path(), &bundle_id, &bundle_data);
    assert!(verify_bundle_identity_file(&path, &bundle_id).is_err());
}

#[test]
fn test_bundle_identity_rejects_corrupt_framing_and_payload() {
    let (bundle_id, bundle_data) = test_bundle_with_id();

    // Corrupt framing: the magic no longer parses.
    let mut framed = bundle_data.clone();
    framed[0] ^= 0xff;
    assert!(read_bundle(&framed, &profile()).is_err());
    assert!(verify_bundle_identity_bytes(&bundle_id, &framed).is_err());

    // Corrupt payload: framing still parses, but the content hash no longer
    // matches the trailing ID.
    let mut payload = bundle_data.clone();
    payload[20] ^= 0xff;
    assert!(verify_bundle_identity_bytes(&bundle_id, &payload).is_err());

    let temp = tempfile::tempdir().unwrap();
    let path = write_named_bundle(temp.path(), &bundle_id, &payload);
    assert!(verify_bundle_identity_file(&path, &bundle_id).is_err());
}

#[test]
fn probe_encoded_size_matches_real_bundle_for_all_codecs() {
    use crate::manifest::CompressionKind;
    for codec in [CompressionKind::None, CompressionKind::Zstd, CompressionKind::Lz4] {
        let profile = CompressionProfile::for_algorithm(codec, None);
        for chunk_count in [0, 1, 3] {
            let mut builder = BundleBuilder::new(profile.clone());
            let mut raw = Vec::new();
            for index in 0..chunk_count {
                let data = format!("chunk {index} repeated content ").repeat(20_000).into_bytes();
                raw.push(data.clone());
                builder.add_chunk(ChunkHash::from_blake3(hash::hash_chunk(&data)), data);
            }
            let segments: Vec<&[u8]> = raw.iter().map(Vec::as_slice).collect();
            let expected = crate::artifact::compression::compressed_size(&segments, &profile).unwrap()
                + encoded_metadata_size(chunk_count);
            assert_eq!(builder.build().unwrap().len() as u64, expected);
        }
    }
}
