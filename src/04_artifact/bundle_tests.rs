use super::*;
use crate::repo::artifacts::verify_bundle_identity_file;

fn profile() -> CompressionProfile {
    CompressionProfile::default()
}
use crate::artifact::compression::compressed_size;
use crate::artifact::hash::ChunkHash;
use crate::artifact::profiles::CompressionKind;
use crate::error::Error;
use crate::filesystem::write_atomic_with;
use crate::profiling::ProfileMode;
use crate::profiling::Profiler;
use std::fs;
use std::fs::File;
use std::path::Path;
use std::path::PathBuf;

#[test]
fn build_and_read_bundle() {
    let chunk1_data = b"chunk one data for testing ".repeat(10);
    let chunk2_data = b"chunk two different data ".repeat(10);

    let hash1 = ChunkHash::of(&chunk1_data);
    let hash2 = ChunkHash::of(&chunk2_data);

    let mut builder = BundleBuilder::new(CompressionProfile::default());
    builder.add_chunk(hash1, chunk1_data.to_vec());
    builder.add_chunk(hash2, chunk2_data.to_vec());

    let (_bundle_id, bundle_data) = builder.build_with_id().unwrap();

    let mut reader = read_bundle(&bundle_data).unwrap();
    assert_eq!(reader.chunk_count(), 2);

    let read1 = reader.read_chunk_by_hash(&hash1.to_string()).unwrap();
    let read2 = reader.read_chunk_by_hash(&hash2.to_string()).unwrap();

    assert_eq!(read1, chunk1_data.to_vec());
    assert_eq!(read2, chunk2_data.to_vec());
}

#[test]
fn empty_bundle() {
    let builder = BundleBuilder::new(CompressionProfile::default());
    assert!(builder.is_empty());
    assert_eq!(builder.chunk_count(), 0);
    let bundle_data = builder.build().unwrap();
    let reader = read_bundle(&bundle_data).unwrap();
    assert_eq!(reader.chunk_count(), 0);
}

#[test]
fn empty_lz4_bundle() {
    let profile = CompressionProfile::for_algorithm(CompressionKind::Lz4, None);
    let builder = BundleBuilder::new(profile.clone());
    let bundle_data = builder.build().unwrap();
    let mut reader = read_bundle(&bundle_data).unwrap();
    assert_eq!(reader.chunk_count(), 0);
    reader.verify(None).unwrap();
}

#[test]
fn single_chunk_bundle() {
    let data = b"single chunk test data ".repeat(20);
    let hash = ChunkHash::of(&data);

    let mut builder = BundleBuilder::new(CompressionProfile::default());
    builder.add_chunk(hash, data.to_vec());

    let bundle_data = builder.build().unwrap();
    let mut reader = read_bundle(&bundle_data).unwrap();
    assert_eq!(reader.chunk_count(), 1);

    let result = reader.read_chunk(0).unwrap();
    assert_eq!(result, data.to_vec());
}

#[test]
fn verify_bundle() {
    let chunk1 = b"verify test chunk one ".repeat(10);
    let chunk2 = b"verify test chunk two ".repeat(10);

    let mut builder = BundleBuilder::new(CompressionProfile::default());
    builder.add_chunk(ChunkHash::of(&chunk1), chunk1.to_vec());
    builder.add_chunk(ChunkHash::of(&chunk2), chunk2.to_vec());

    let bundle_data = builder.build().unwrap();
    let mut reader = read_bundle(&bundle_data).unwrap();
    reader.verify(None).unwrap();
}

#[test]
fn build_and_read_bundle_compressed_payload() {
    let chunk1 = b"bundle compression test chunk one ".repeat(8);
    let chunk2 = b"bundle compression test chunk two ".repeat(8);

    let profile = CompressionProfile {
        compression_unit: CompressionUnit::Bundle,
        ..CompressionProfile::default()
    };

    let mut builder = BundleBuilder::new(profile.clone());
    builder.add_chunk(ChunkHash::of(&chunk1), chunk1.to_vec());
    builder.add_chunk(ChunkHash::of(&chunk2), chunk2.to_vec());

    let bundle_data = builder.build().unwrap();
    let mut reader = read_bundle(&bundle_data).unwrap();

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
            .all(|entry| entry.estimated_compressed_size > 0)
    );
    assert_eq!(reader.read_chunk(0).unwrap(), chunk1.to_vec());
    assert_eq!(reader.read_chunk(1).unwrap(), chunk2.to_vec());

    let staged = tempfile::NamedTempFile::new().unwrap();
    fs::write(staged.path(), &bundle_data).unwrap();
    let mut file_reader = BundleReader::open(File::open(staged.path()).unwrap()).unwrap();
    let profiler = Profiler::new(ProfileMode::Detailed);
    let (read, metrics) = file_reader.read_bundle_payload(Some(&profiler)).unwrap();
    let second = &read[reader.entries()[1].raw_offset as usize
        ..(reader.entries()[1].raw_offset + reader.entries()[1].raw_size) as usize];
    assert_eq!(second, chunk2.as_slice());
    assert_eq!(metrics.raw_bytes, (chunk1.len() + chunk2.len()) as u64);
    let staging = tempfile::tempdir().unwrap();
    let capture_profiler = Profiler::new(ProfileMode::Detailed);
    builder
        .write_to_staging(staging.path(), Some(&capture_profiler))
        .unwrap();
    let trace_path = staging.path().join("trace.json");
    capture_profiler.write_trace(&trace_path).unwrap();
    let trace: serde_json::Value = serde_json::from_slice(&fs::read(trace_path).unwrap()).unwrap();
    assert!(
        trace["traceEvents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event["ph"] == "X" || event["ph"] == "M")
    );
    let spans = capture_profiler.summary().spans;
    assert_eq!(
        spans
            .iter()
            .find(|span| span.name == "capture.bundle.flush")
            .map(|span| span.count),
        Some(1),
        "missing post-finalization flush marker"
    );
}

#[test]
fn stream_bundle_chunks_only_delivers_required_chunks() {
    let chunks = [
        b"streamed chunk one".to_vec(),
        b"streamed chunk two".repeat(100),
        b"small".to_vec(),
        b"larger again".repeat(200),
    ];
    let hashes = chunks
        .iter()
        .map(|chunk| ChunkHash::of(chunk))
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
    let mut reader = read_bundle(&bundle_data).unwrap();
    let mut delivered = Vec::new();
    let metrics = reader
        .stream_bundle_chunks(None, &[false, true, true, true], |index, chunk| {
            delivered.push((index, chunk.to_vec()));
            Ok(())
        })
        .unwrap();

    assert_eq!(
        delivered,
        vec![
            (1, chunks[1].clone()),
            (2, chunks[2].clone()),
            (3, chunks[3].clone())
        ]
    );
    assert_eq!(
        metrics.raw_bytes,
        chunks.iter().map(|chunk| chunk.len() as u64).sum::<u64>()
    );
}

#[test]
fn build_and_read_uncompressed_bundle_with_zstd_magic_prefix() {
    let chunk1 = vec![0x28, 0xB5, 0x2F, 0xFD, b'r', b'a', b'w', b'1'];
    let chunk2 = b"plain uncompressed payload".to_vec();

    let profile = CompressionProfile::for_algorithm(CompressionKind::None, None);
    let mut builder = BundleBuilder::new(profile.clone());
    builder.add_chunk(ChunkHash::of(&chunk1), chunk1.clone());
    builder.add_chunk(ChunkHash::of(&chunk2), chunk2.clone());

    let bundle_data = builder.build().unwrap();
    let mut reader = read_bundle(&bundle_data).unwrap();

    assert_eq!(reader.compression_unit(), CompressionUnit::Bundle);
    assert_eq!(reader.read_chunk(0).unwrap(), chunk1);
    assert_eq!(reader.read_chunk(1).unwrap(), chunk2);
}

#[test]
fn bundle_id_deterministic() {
    let data1 = b"deterministic test ".repeat(15);
    let hash1 = ChunkHash::of(&data1);

    let mut b1 = BundleBuilder::new(CompressionProfile::default());
    b1.add_chunk(hash1, data1.to_vec());
    let (id1, _) = b1.build_with_id().unwrap();

    let mut b2 = BundleBuilder::new(CompressionProfile::default());
    b2.add_chunk(hash1, data1.to_vec());
    let (id2, _) = b2.build_with_id().unwrap();

    assert_eq!(id1, id2);
}

#[test]
fn bundle_id_depends_on_final_bundle_bytes() {
    let d1 = b"chunk a ".repeat(10);
    let d2 = b"chunk b ".repeat(10);
    let h1 = ChunkHash::of(&d1);
    let h2 = ChunkHash::of(&d2);

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
fn invalid_bundle_table() {
    let result = read_bundle(b"XXXX extra data to read");
    assert!(
        matches!(result, Err(Error::Artifact(_))),
        "should reject invalid magic"
    );
}

fn test_bundle_with_id() -> (String, Vec<u8>) {
    let data = b"identity test chunk data ".repeat(20);
    let hash = ChunkHash::of(&data);
    let mut builder = BundleBuilder::new(CompressionProfile::default());
    builder.add_chunk(hash, data.to_vec());
    builder.build_with_id().unwrap()
}

fn write_named_bundle(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn bundle_identity_accepts_intact_file_and_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let (bundle_id, bundle_data) = test_bundle_with_id();
    verify_bundle_identity_bytes(&bundle_id, &bundle_data).unwrap();
    let path = write_named_bundle(temp.path(), &bundle_id, &bundle_data);
    verify_bundle_identity_file(&path, &bundle_id, None).unwrap();
}

#[test]
fn streamed_identity_matches_whole_bundle_check_for_any_write_split() {
    use std::io::Write;
    let (bundle_id, bundle_data) = test_bundle_with_id();
    let mut damaged_trailer = bundle_data.clone();
    *damaged_trailer.last_mut().unwrap() ^= 1;
    let mut damaged_content = bundle_data.clone();
    damaged_content[0] ^= 1;
    // Splits smaller and larger than the 32-byte ID exercise both tail paths.
    for piece_size in [1, 7, 31, 32, 33, 4096, bundle_data.len()] {
        for (bytes, valid) in [
            (&bundle_data, true),
            (&damaged_trailer, false),
            (&damaged_content, false),
            (&bundle_data[..20].to_vec(), false),
        ] {
            let mut writer = BundleIdentityWriter::new(Vec::new());
            for piece in bytes.chunks(piece_size) {
                writer.write_all(piece).unwrap();
            }
            let result = writer.finish(&bundle_id);
            assert_eq!(result.is_ok(), valid, "piece size {piece_size}");
            if let Ok(output) = result {
                assert_eq!(&output, bytes);
            }
        }
    }
}

#[test]
fn invalid_download_is_not_promoted_over_a_valid_cached_bundle() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let (bundle_id, bundle_data) = test_bundle_with_id();
    let path = write_named_bundle(temp.path(), &bundle_id, &bundle_data);
    let mut corrupt = bundle_data.clone();
    corrupt[0] ^= 0xff;
    let result = write_atomic_with(&path, |file| {
        file.write_all(&corrupt)?;
        verify_bundle_identity_reader(file, &bundle_id, None)
    });
    assert!(matches!(result, Err(crate::Error::HashMismatch { .. })));
    assert_eq!(fs::read(&path).unwrap(), bundle_data);
    write_atomic_with(&path, |file| {
        file.write_all(&bundle_data)?;
        verify_bundle_identity_reader(file, &bundle_id, None)
    })
    .unwrap();
    verify_bundle_identity_file(&path, &bundle_id, None).unwrap();
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn bundle_identity_rejects_corrupt_filename() {
    let temp = tempfile::tempdir().unwrap();
    let (bundle_id, bundle_data) = test_bundle_with_id();
    // Valid hex that names nothing: the path-derived ID must match.
    let other_id = "0".repeat(64);
    assert_ne!(other_id, bundle_id);
    let path = write_named_bundle(temp.path(), &other_id, &bundle_data);
    assert!(verify_bundle_identity_file(&path, &bundle_id, None).is_err());
    // A non-hex filename is rejected before any hashing.
    let path = write_named_bundle(temp.path(), "not-a-bundle-id", &bundle_data);
    assert!(verify_bundle_identity_file(&path, &bundle_id, None).is_err());
}

#[test]
fn bundle_identity_rejects_corrupt_trailing_id() {
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
    assert!(verify_bundle_identity_file(&path, &bundle_id, None).is_err());
}

#[test]
fn bundle_identity_rejects_corrupt_framing_and_payload() {
    let (bundle_id, bundle_data) = test_bundle_with_id();

    // Corrupt framing: the magic no longer parses.
    let mut framed = bundle_data.clone();
    framed[0] ^= 0xff;
    assert!(read_bundle(&framed).is_err());
    assert!(verify_bundle_identity_bytes(&bundle_id, &framed).is_err());

    // Corrupt payload: framing still parses, but the content hash no longer
    // matches the trailing ID.
    let mut payload = bundle_data.clone();
    payload[20] ^= 0xff;
    assert!(verify_bundle_identity_bytes(&bundle_id, &payload).is_err());

    let temp = tempfile::tempdir().unwrap();
    let path = write_named_bundle(temp.path(), &bundle_id, &payload);
    assert!(verify_bundle_identity_file(&path, &bundle_id, None).is_err());
}

#[test]
fn probe_encoded_size_matches_real_bundle_for_all_codecs() {
    use crate::artifact::profiles::CompressionKind;
    for codec in [
        CompressionKind::None,
        CompressionKind::Zstd,
        CompressionKind::Lz4,
    ] {
        let profile = CompressionProfile::for_algorithm(codec, None);
        for chunk_count in [0, 1, 3] {
            let mut builder = BundleBuilder::new(profile.clone());
            let mut raw = Vec::new();
            for index in 0..chunk_count {
                let data = format!("chunk {index} repeated content ")
                    .repeat(20_000)
                    .into_bytes();
                raw.push(data.clone());
                builder.add_chunk(ChunkHash::of(&data), data);
            }
            let segments: Vec<&[u8]> = raw.iter().map(Vec::as_slice).collect();
            let expected =
                compressed_size(&segments, &profile).unwrap() + encoded_metadata_size(chunk_count);
            assert_eq!(builder.build().unwrap().len() as u64, expected);
        }
    }
}

#[test]
fn streamed_bundle_releases_each_loaded_chunk_before_requesting_the_next() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct TrackedBytes {
        bytes: Vec<u8>,
        live: Arc<AtomicUsize>,
    }
    impl AsRef<[u8]> for TrackedBytes {
        fn as_ref(&self) -> &[u8] {
            &self.bytes
        }
    }
    impl Drop for TrackedBytes {
        fn drop(&mut self) {
            self.live.fetch_sub(1, Ordering::SeqCst);
        }
    }
    for codec in [
        CompressionKind::None,
        CompressionKind::Zstd,
        CompressionKind::Lz4,
    ] {
        let profile = CompressionProfile::for_algorithm(codec, None);
        let inputs = [
            b"first".repeat(100_000),
            b"small".to_vec(),
            b"last".repeat(70_000),
        ];
        let descriptors = inputs
            .iter()
            .map(|bytes| (ChunkHash::of(bytes), bytes.len() as u64))
            .collect::<Vec<_>>();
        let directory = tempfile::tempdir().unwrap();
        let live = Arc::new(AtomicUsize::new(0));
        let mut next_index = 0;
        let artifact = write_bundle_to_staging(
            &descriptors,
            &profile,
            directory.path(),
            None,
            None,
            |index| {
                assert_eq!(index, next_index);
                next_index += 1;
                assert_eq!(
                    live.fetch_add(1, Ordering::SeqCst),
                    0,
                    "previous raw chunk is still retained"
                );
                Ok(TrackedBytes {
                    bytes: inputs[index].clone(),
                    live: live.clone(),
                })
            },
        )
        .unwrap();
        assert_eq!(next_index, inputs.len());
        assert_eq!(live.load(Ordering::SeqCst), 0);
        let encoded = fs::read(&artifact.staged_path).unwrap();
        let mut reader = read_bundle(&encoded).unwrap();
        for ((hash, _), expected) in descriptors.iter().zip(&inputs) {
            assert_eq!(
                &reader.read_chunk_by_hash(&hash.to_string()).unwrap(),
                expected
            );
        }
    }
}

#[test]
fn streamed_bundle_discards_partial_staging_on_loader_failure_or_wrong_length() {
    let first = b"first".to_vec();
    let second = b"second".to_vec();
    let descriptors = [&first, &second].map(|bytes| (ChunkHash::of(bytes), bytes.len() as u64));
    for codec in [
        CompressionKind::None,
        CompressionKind::Zstd,
        CompressionKind::Lz4,
    ] {
        let profile = CompressionProfile::for_algorithm(codec, None);
        // Exercise failure after an earlier chunk has already entered the codec.
        for wrong_length in [None, Some(0), Some(5), Some(7)] {
            let directory = tempfile::tempdir().unwrap();
            let result = write_bundle_to_staging(
                &descriptors,
                &profile,
                directory.path(),
                None,
                None,
                |index| {
                    if index == 0 {
                        return Ok(first.clone());
                    }
                    match wrong_length {
                        Some(size) => Ok(vec![0; size]),
                        None => Err(crate::Error::artifact("injected source read failure")),
                    }
                },
            );
            assert!(result.is_err(), "{codec:?} {wrong_length:?}");
            assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
        }
    }
}

#[test]
fn streamed_bundle_validates_descriptors_before_loading_payloads() {
    let hash = ChunkHash::from_bytes([1; 32]);
    for descriptors in [
        vec![(hash, 0)],
        vec![(hash, 1), (hash, 2)],
        vec![(hash, format::MAX_RAW_CHUNK_SIZE + 1)],
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut loaded = false;
        let result = write_bundle_to_staging(
            &descriptors,
            &profile(),
            directory.path(),
            None,
            None,
            |_| {
                loaded = true;
                Ok(Vec::<u8>::new())
            },
        );
        assert!(result.is_err());
        assert!(!loaded);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

#[test]
fn only_verification_checks_the_trailing_bundle_id() {
    let bytes = b"identity checked on demand".repeat(10);
    let mut builder = BundleBuilder::new(profile());
    builder.add_chunk(ChunkHash::of(&bytes), bytes.clone());
    let mut bundle_data = builder.build().unwrap();
    *bundle_data.last_mut().unwrap() ^= 1;

    let mut reader = read_bundle(&bundle_data).unwrap();
    reader
        .stream_bundle_chunks(None, &[true], |_, chunk| {
            assert_eq!(chunk, bytes.as_slice());
            Ok(())
        })
        .unwrap();
    assert!(reader.verify(None).is_err());
}

#[test]
fn damaged_compressed_payload_is_an_artifact_error() {
    let bytes = b"compressible payload ".repeat(100);
    let mut builder = BundleBuilder::new(profile());
    builder.add_chunk(ChunkHash::of(&bytes), bytes);
    let mut bundle_data = builder.build().unwrap();
    // Damage the first compressed byte, right after the bundle header.
    bundle_data[BUNDLE_HEADER_SIZE as usize] ^= 0xff;

    let mut reader = read_bundle(&bundle_data).unwrap();
    let error = reader
        .stream_bundle_chunks(None, &[true], |_, _| Ok(()))
        .unwrap_err();
    assert!(matches!(error, Error::Artifact(_)), "{error:?}");
}
