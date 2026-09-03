use super::*;

fn profile() -> CompressionProfile {
    CompressionProfile::default()
}
use crate::hash::{self, ChunkHash};

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
    assert_eq!(reader.entries()[0].compressed_offset, 0);
    assert_eq!(reader.entries()[1].compressed_offset, 0);
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
    let profiler = crate::GutchProfiler::new();
    let (read, metrics) = file_reader
        .read_bundle_payload_with_profiler(Some(&profiler))
        .unwrap();
    let second = &read[reader.entries()[1].raw_offset as usize
        ..(reader.entries()[1].raw_offset + reader.entries()[1].raw_size) as usize];
    assert_eq!(second, chunk2.as_slice());
    assert_eq!(metrics.raw_bytes, (chunk1.len() + chunk2.len()) as u64);
    let spans = profiler.summary().spans;
    for name in [
        "snapshot.reconstruct.bundle.read_payload",
        "snapshot.reconstruct.bundle.decompress_payload",
    ] {
        assert_eq!(
            spans
                .iter()
                .find(|span| span.name == name)
                .map(|span| span.count),
            Some(1),
            "missing expected bundle phase marker {name}"
        );
    }

    let staging = tempfile::tempdir().unwrap();
    let capture_profiler = crate::GutchProfiler::new();
    builder
        .write_to_staging(staging.path(), Some(&capture_profiler))
        .unwrap();
    let trace_path = staging.path().join("trace.json");
    capture_profiler.write_to(&trace_path).unwrap();
    let trace: serde_json::Value =
        serde_json::from_slice(&std::fs::read(trace_path).unwrap()).unwrap();
    let finalize_complete = trace["traceEvents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["name"] == "capture.compress_bundle_artifact.finalize_id.complete")
        .expect("missing bundle ID finalization completion marker");
    assert_eq!(finalize_complete["args"]["id_present"], true);
    assert_eq!(finalize_complete["args"]["next_phase"], "flush_staged_file");
    let keep_complete = trace["traceEvents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["name"] == "capture.compress_bundle_artifact.keep_staged_file.complete")
        .expect("missing staged artifact handoff marker");
    assert_eq!(keep_complete["args"]["id_present"], true);
    assert_eq!(
        keep_complete["args"]["next_phase"],
        "return_staged_artifact"
    );
    let spans = capture_profiler.summary().spans;
    for name in [
        "capture.compress_bundle_artifact.flush_staged_file",
        "capture.compress_bundle_artifact.sync_staged_file",
        "capture.compress_bundle_artifact.keep_staged_file",
    ] {
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
        b"streamed chunk two".to_vec(),
        b"streamed chunk three".to_vec(),
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
        .stream_bundle_chunks_with_profiler(None, &[false, true, true], |index, chunk| {
            delivered.push((index, chunk.to_vec()));
            Ok(())
        })
        .unwrap();

    assert_eq!(
        delivered,
        vec![(1, chunks[1].clone()), (2, chunks[2].clone())]
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

    let profile = CompressionProfile::for_preset(crate::manifest::CompressionPreset::Uncompressed);
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
