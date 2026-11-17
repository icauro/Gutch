use super::*;
use crate::artifact::chunk::scan_file_chunks;
use crate::artifact::chunk_stream::ChunkStream;
use crate::artifact::hash::ChunkHash;
use std::fs;
use std::io::{Cursor, Read};

// Literal scalar reference matching the pinned DedupBench RAM cutpoint rule.
fn reference(bytes: &[u8], window: usize, maximum: usize) -> usize {
    let limit = bytes.len().min(maximum);
    if limit <= window {
        return limit;
    }
    let maximum_byte = *bytes[..window].iter().max().unwrap();
    for (i, byte) in bytes.iter().enumerate().take(limit).skip(window) {
        if *byte >= maximum_byte {
            return i;
        }
    }
    limit
}

#[test]
fn scalar_and_avx2_match_reference_at_edges_and_unaligned_offsets() {
    let mut seed = 917u64;
    let random: Vec<u8> = (0..8193)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as u8
        })
        .collect();
    for data in [
        random,
        vec![0; 8193],
        vec![255; 8193],
        (0..8193).map(|i| (i % 251) as u8).collect(),
    ] {
        for window in [1, 15, 31, 32, 33, 63, 64, 65, 256, 512, 1024] {
            let scalar = Ram::new(window, window * 4);
            for start in 0..32 {
                for size in [
                    0,
                    1,
                    window - 1,
                    window,
                    window + 1,
                    window + 31,
                    window + 32,
                    window * 4,
                    window * 4 + 1,
                ] {
                    let bytes = &data[start..start + size];
                    let expected = reference(bytes, window, window * 4);
                    assert_eq!(scalar.cut(bytes), expected);
                    if let Some(vector) = scalar.accelerated() {
                        assert_eq!(vector.cut(bytes), expected);
                    }
                }
            }
        }
    }
}

#[test]
fn streaming_and_capture_preserve_ram_boundaries_and_hashes() {
    use crate::artifact::profiles::{ChunkSizingPreset, ChunkSizingProfile, Chunker};
    struct ShortRead(Cursor<Vec<u8>>);
    impl Read for ShortRead {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let size = buffer.len().min(997);
            self.0.read(&mut buffer[..size])
        }
    }
    let mut seed = 73u64;
    let data: Vec<u8> = (0..2_100_003)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as u8
        })
        .collect();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("source");
    for size in [0, 511, 524289, 1_048_577, data.len()] {
        fs::write(&path, &data[..size]).unwrap();
        for &preset in &ChunkSizingPreset::ALL {
            let profile = ChunkSizingProfile::for_preset(preset).with_chunker(Chunker::Vram256);
            let window = profile.avg_chunk_bytes as usize;
            let maximum = profile.max_chunk_bytes as usize;
            let (scan, _) =
                scan_file_chunks(&path, size as u64, &profile, None, &Default::default()).unwrap();
            let mut stream = ChunkStream::with_boundary(
                ShortRead(Cursor::new(data[..size].to_vec())),
                Ram::new(window, maximum),
                size as u64,
            );
            let mut offset = 0;
            for chunk in scan.chunks {
                let length = reference(&data[offset..size], window, maximum);
                assert_eq!((chunk.offset, chunk.size), (offset as u64, length as u64));
                assert_eq!(chunk.hash, ChunkHash::of(&data[offset..offset + length]));
                let streamed = stream.next(&Default::default()).unwrap().unwrap();
                assert_eq!(streamed.data, &data[offset..offset + length]);
                offset += length;
            }
            assert_eq!(offset, size);
            assert!(stream.next(&Default::default()).unwrap().is_none());
            assert_eq!(scan.full_file_hash, ChunkHash::of(&data[..size]));
        }
    }
}
