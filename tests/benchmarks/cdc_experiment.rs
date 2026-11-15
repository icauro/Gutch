//! Test-only native experiment: no experimental chunker reaches stored manifests.
use super::chunk_stream::{BoundaryFinder, ChunkBoundary, ChunkStream};
use super::fastcdc_v2020;
use super::jc::{JumpChunker, Parameters};
use super::jc_tests::random_bytes;
mod cdc_baseline;
use crate::artifact::profiles::{ChunkSizingPreset, ChunkSizingProfile};
use crate::progress::OperationControl;
use serde_json::json;
use std::{
    collections::HashSet,
    hint::black_box,
    io::Cursor,
    path::{Path, PathBuf},
    time::Instant,
};

fn file_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<_> = walkdir::WalkDir::new(root)
        .into_iter()
        .map(Result::unwrap)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .collect();
    paths.sort();
    paths
}

// Untimed reuse analysis needs only one updated file resident alongside the base.
enum UpdatedInput {
    Memory(Vec<Vec<u8>>),
    Files(Vec<PathBuf>),
}

impl UpdatedInput {
    fn visit(&self, mut consume: impl FnMut(&[u8])) {
        match self {
            Self::Memory(files) => files.iter().for_each(|file| consume(file)),
            Self::Files(paths) => paths
                .iter()
                .for_each(|path| consume(&std::fs::read(path).unwrap())),
        }
    }
}

fn scan<C: ChunkBoundary>(boundary: &C, data: &[Vec<u8>], hash: bool) -> usize {
    let mut chunks = 0;
    for file in data {
        let mut remaining = black_box(file.as_slice());
        while !remaining.is_empty() {
            let length = boundary.cut(remaining);
            if hash {
                black_box(blake3::hash(&remaining[..length]));
            }
            remaining = &remaining[length..];
            chunks += 1;
        }
    }
    black_box(chunks)
}

// The old direct call path, without a trait, controls for interface overhead.
fn direct_fastcdc(data: &[Vec<u8>], profile: &ChunkSizingProfile, hash: bool) -> usize {
    let (min, avg, max) = (
        profile.min_chunk_bytes as usize,
        profile.avg_chunk_bytes as usize,
        profile.max_chunk_bytes as usize,
    );
    let (strict, relaxed) = super::fastcdc_v2020::select_masks(avg);
    let mut chunks = 0;
    for file in data {
        let mut remaining = black_box(file.as_slice());
        while !remaining.is_empty() {
            let length = super::fastcdc_v2020::cut(remaining, min, avg, max, strict, relaxed).1;
            if hash {
                black_box(blake3::hash(&remaining[..length]));
            }
            remaining = &remaining[length..];
            chunks += 1;
        }
    }
    black_box(chunks)
}

fn stream<C: ChunkBoundary>(factory: impl Fn() -> C, data: &[Vec<u8>]) -> usize {
    let mut chunks = 0;
    let control = OperationControl::default();
    for file in data {
        let mut stream =
            ChunkStream::with_boundary(Cursor::new(file), factory(), file.len() as u64);
        while let Some(chunk) = stream.next(&control).unwrap() {
            black_box(chunk.offset);
            black_box(blake3::hash(chunk.data));
            chunks += 1;
        }
    }
    black_box(chunks)
}

fn baseline_stream(data: &[Vec<u8>], profile: &ChunkSizingProfile) -> usize {
    let mut chunks = 0;
    let control = OperationControl::default();
    for file in data {
        let mut stream =
            cdc_baseline::ChunkStream::new(Cursor::new(file), profile, file.len() as u64);
        while let Some(chunk) = stream.next(&control).unwrap() {
            black_box(chunk.offset);
            black_box(blake3::hash(chunk.data));
            chunks += 1;
        }
    }
    black_box(chunks)
}

fn distribution<C: ChunkBoundary>(
    boundary: &C,
    base: &[Vec<u8>],
    updated: &UpdatedInput,
) -> serde_json::Value {
    let mut known = HashSet::new();
    let mut lengths = Vec::new();
    for file in base {
        let mut remaining = file.as_slice();
        while !remaining.is_empty() {
            let length = boundary.cut(remaining);
            known.insert(blake3::hash(&remaining[..length]));
            lengths.push(length);
            remaining = &remaining[length..];
        }
    }
    let mut reused = 0_u64;
    let mut new_chunks = HashSet::new();
    let mut new_unique = 0_u64;
    let mut updated_bytes = 0_usize;
    updated.visit(|file| {
        updated_bytes += file.len();
        let mut remaining = file;
        while !remaining.is_empty() {
            let length = boundary.cut(remaining);
            let digest = blake3::hash(&remaining[..length]);
            if known.contains(&digest) {
                reused += length as u64;
            } else if new_chunks.insert(digest) {
                new_unique += length as u64;
            }
            remaining = &remaining[length..];
        }
    });
    lengths.sort();
    let total: usize = lengths.iter().sum();
    json!({"chunks": lengths.len(), "mean_bytes": total as f64 / lengths.len() as f64,
        "p50_bytes": lengths[lengths.len()/2], "p95_bytes": lengths[lengths.len()*95/100],
        "max_size_cuts": lengths.iter().filter(|&&n| n == boundary.max_size()).count(),
        "updated_reused_bytes": reused, "updated_bytes": updated_bytes,
        "updated_reused_percent": 100.0 * reused as f64 / updated_bytes as f64,
        "updated_new_unique_bytes": new_unique})
}

#[test]
#[ignore = "explicit release experiment; reads caller-provided input builds"]
fn compare_chunkers() {
    let repetitions: usize = std::env::var("GUTCH_CDC_REPETITIONS")
        .unwrap_or("5".into())
        .parse()
        .unwrap();
    assert!(repetitions >= 3);
    let output = std::env::var("GUTCH_CDC_OUTPUT").expect("GUTCH_CDC_OUTPUT");
    let random = random_bytes(64 * 1024 * 1024, 917);
    let mut edited = random.clone();
    for position in [1024, 1_000_000, 32_000_000] {
        edited.splice(position..position, [7; 17]);
    }
    let mut rows = Vec::new();
    let label = std::env::var("GUTCH_CDC_LABEL").unwrap_or("input-builds".into());
    for dataset in ["random-edits", "repeated", label.as_str()] {
        let (base, updated) = match dataset {
            "random-edits" => (
                vec![random.clone()],
                UpdatedInput::Memory(vec![edited.clone()]),
            ),
            "repeated" => (
                vec![vec![0; 64 * 1024 * 1024]],
                UpdatedInput::Memory(vec![vec![0; 64 * 1024 * 1024 + 17]]),
            ),
            _ => (
                file_paths(Path::new(&std::env::var("GUTCH_CDC_BASE").unwrap()))
                    .iter()
                    .map(|p| std::fs::read(p).unwrap())
                    .collect(),
                UpdatedInput::Files(file_paths(Path::new(
                    &std::env::var("GUTCH_CDC_UPDATED").unwrap(),
                ))),
            ),
        };
        let bytes: usize = base.iter().map(Vec::len).sum();
        assert!(bytes > 0);
        for preset in [
            ChunkSizingPreset::Chunk8KiB,
            ChunkSizingPreset::Chunk128KiB,
            ChunkSizingPreset::Chunk1MiB,
        ] {
            let profile = ChunkSizingProfile::for_preset(preset);
            let (min, avg, max) = (
                profile.min_chunk_bytes as usize,
                profile.avg_chunk_bytes as usize,
                profile.max_chunk_bytes as usize,
            );
            let fast = BoundaryFinder::new(&profile);
            let jc = |parameters| JumpChunker::new(min, avg, max, parameters);
            let measure = |variant, mode| match (variant, mode) {
                (0, 2) => baseline_stream(&base, &profile),
                (0, _) => direct_fastcdc(&base, &profile, mode == 1),
                (1, 2) => stream(|| BoundaryFinder::new(&profile), &base),
                (2, 2) => stream(|| jc(Parameters::Plakar), &base),
                (3, 2) => stream(|| jc(Parameters::PaperScaled), &base),
                (1, _) => scan(&fast, &base, mode == 1),
                (2, _) => scan(&jc(Parameters::Plakar), &base, mode == 1),
                (3, _) => scan(&jc(Parameters::PaperScaled), &base, mode == 1),
                _ => unreachable!(),
            };
            let mut samples = vec![vec![Vec::new(); 3]; 4];
            for repetition in 0..=repetitions {
                for index in 0..4 {
                    let variant = (index + repetition) % 4;
                    for (mode, mode_samples) in samples[variant].iter_mut().enumerate() {
                        let start = Instant::now();
                        let count = measure(variant, mode);
                        let seconds = start.elapsed().as_secs_f64();
                        assert!(count > 0);
                        if repetition > 0 {
                            mode_samples.push(seconds);
                        }
                    }
                }
                println!(
                    "CDC progress {dataset} target={avg} repetition={repetition}/{repetitions}"
                );
            }
            for (variant, name) in [
                "fastcdc-direct",
                "fastcdc-generic",
                "jc-plakar",
                "jc-paper-scaled",
            ]
            .iter()
            .enumerate()
            {
                let quality = match variant {
                    0 => serde_json::Value::Null,
                    1 => distribution(&fast, &base, &updated),
                    2 => distribution(&jc(Parameters::Plakar), &base, &updated),
                    _ => distribution(&jc(Parameters::PaperScaled), &base, &updated),
                };
                let mut timings = serde_json::Map::new();
                for (mode, label) in ["boundary", "boundary_hash", "stream_hash"]
                    .iter()
                    .enumerate()
                {
                    let values = &mut samples[variant][mode];
                    if values.is_empty() {
                        continue;
                    }
                    values.sort_by(f64::total_cmp);
                    let seconds = values[values.len() / 2];
                    timings.insert(label.to_string(), json!({"seconds": seconds, "mib_per_second": bytes as f64 / 1048576.0 / seconds, "samples_seconds": values}));
                }
                rows.push(json!({"dataset": dataset, "variant": name, "target": avg, "min": min, "max": max,
                    "input_bytes": bytes, "files": base.len(), "timings": timings, "quality": quality}));
            }
            std::fs::write(&output, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
            println!("CDC completed {dataset} target={avg}");
        }
    }
}
