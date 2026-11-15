use super::*;
use crate::artifact::profiles::ChunkSizingPreset;
use crate::progress::CancellationHook;
use clap::ValueEnum;
use fastcdc::v2020::{FastCDC, Normalization};
use std::io::{self, Cursor};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

fn random_bytes(length: usize, mut seed: u64) -> Vec<u8> {
    (0..length)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as u8
        })
        .collect()
}

fn compare(bytes: &[u8], profile: &ChunkSizingProfile, hint: u64, read_limit: usize) {
    struct ShortReads<'a> {
        input: Cursor<&'a [u8]>,
        limit: usize,
    }
    impl Read for ShortReads<'_> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            let length = output.len().min(self.limit);
            self.input.read(&mut output[..length])
        }
    }
    let expected: Vec<_> = FastCDC::with_level(
        bytes,
        profile.min_chunk_bytes as usize,
        profile.avg_chunk_bytes as usize,
        profile.max_chunk_bytes as usize,
        Normalization::Level1,
    )
    .collect();
    let mut stream = ChunkStream::new(
        ShortReads {
            input: Cursor::new(bytes),
            limit: read_limit,
        },
        profile,
        hint,
    );
    let control = OperationControl::default();
    let boundary = BoundaryFinder::new(profile);
    for chunk in expected {
        let actual = stream.next(&control).unwrap().expect("missing chunk");
        assert_eq!(
            (actual.offset, actual.data.len()),
            (chunk.offset as u64, chunk.length)
        );
        assert_eq!(
            actual.data,
            &bytes[chunk.offset..chunk.offset + chunk.length]
        );
        assert_eq!(boundary.cut(&bytes[chunk.offset..]), chunk.length);
    }
    assert!(stream.next(&control).unwrap().is_none());
    assert!(stream.next(&control).unwrap().is_none());
}

#[test]
fn every_preset_matches_upstream_across_refills_and_short_reads() {
    let bytes = random_bytes(2 * 1024 * 1024 + 37, 917);
    for preset in ChunkSizingPreset::value_variants() {
        let profile = ChunkSizingProfile::for_preset(*preset);
        for (hint, limit) in [
            (0, 4093),
            (bytes.len() as u64, usize::MAX),
            (u64::MAX, 8191),
        ] {
            compare(&bytes, &profile, hint, limit);
        }
        for length in [0, 1, 127, 128, 511, 512, 1023] {
            compare(&bytes[..length], &profile, length as u64, 31);
        }
    }
}

#[test]
fn repeated_bytes_and_largest_windows_preserve_exact_boundaries() {
    let profile = ChunkSizingProfile::for_preset(ChunkSizingPreset::Chunk4MiB);
    let length = 2 * profile.max_chunk_bytes as usize + 3;
    compare(&vec![0; length], &profile, length as u64, usize::MAX);
    compare(
        &random_bytes(length, 73),
        &profile,
        length as u64,
        usize::MAX,
    );
}

#[test]
fn randomized_profiles_and_inputs_match_upstream() {
    bolero::check!().for_each(|input: &[u8]| {
        let presets = ChunkSizingPreset::value_variants();
        let index = input.first().copied().unwrap_or(0) as usize % presets.len();
        let profile = ChunkSizingProfile::for_preset(presets[index]);
        compare(input, &profile, input.len() as u64, 113);
    });
}

#[test]
fn stream_propagates_read_errors_and_retries_interruption() {
    struct FaultReader {
        calls: usize,
    }
    impl Read for FaultReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            self.calls += 1;
            Err(io::Error::from(if self.calls == 1 {
                io::ErrorKind::Interrupted
            } else {
                io::ErrorKind::PermissionDenied
            }))
        }
    }
    let mut stream = ChunkStream::new(FaultReader { calls: 0 }, &ChunkSizingProfile::default(), 0);
    assert!(matches!(
        stream.next(&Default::default()),
        Err(crate::Error::Io(_))
    ));
    assert_eq!(stream.reader.calls, 2);
}

#[test]
fn cancellation_is_checked_during_buffer_refill() {
    #[derive(Debug)]
    struct CancelAfterTwoChecks(AtomicUsize);
    impl CancellationHook for CancelAfterTwoChecks {
        fn is_cancelled(&self) -> bool {
            self.0.fetch_add(1, Ordering::Relaxed) >= 2
        }
    }
    let control = OperationControl::default()
        .with_cancellation(Arc::new(CancelAfterTwoChecks(AtomicUsize::new(0))));
    let mut stream = ChunkStream::new(io::repeat(4).take(1), &ChunkSizingProfile::default(), 0);
    assert!(matches!(
        stream.next(&control),
        Err(crate::Error::Cancelled)
    ));
    assert_eq!(
        stream.reader.limit(),
        0,
        "one read completed before cancellation"
    );
}
