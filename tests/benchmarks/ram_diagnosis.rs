//! Diagnostic only: resident-input boundary and hashing costs, one worker.
use crate::artifact::chunk_stream::{BoundaryFinder, ChunkBoundary};
use crate::artifact::ram::Ram;
use crate::manifest::{ChunkSizingPreset, ChunkSizingProfile};
use std::{hint::black_box, time::Instant};

fn scan<C: ChunkBoundary>(kernel: &C, files: &[Vec<u8>], mode: usize) -> (u64, u64) {
    let mut count = 0u64;
    let mut fingerprint = 0u64;
    for file in files {
        let mut bytes = black_box(file.as_slice());
        let mut full_hash = blake3::Hasher::new();
        while !bytes.is_empty() {
            let length = kernel.cut(bytes);
            count += 1;
            fingerprint = fingerprint.wrapping_mul(31).wrapping_add(length as u64);
            if mode >= 1 {
                black_box(blake3::hash(&bytes[..length]));
            }
            if mode == 2 {
                full_hash.update(&bytes[..length]);
            }
            bytes = &bytes[length..];
        }
        if mode == 2 {
            black_box(full_hash.finalize());
        }
    }
    black_box((count, fingerprint))
}

#[test]
#[ignore = "resident-input diagnostic; explicit data/output environment required"]
fn compare_ram_kernels() {
    let input = std::env::var("GUTCH_RAM_INPUTS").unwrap();
    let paths: Vec<String> = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
    let files: Vec<Vec<u8>> = paths.iter().map(|p| std::fs::read(p).unwrap()).collect();
    let total: usize = files.iter().map(Vec::len).sum();
    let mut results = Vec::new();
    for (window, preset) in [
        (8192, ChunkSizingPreset::Chunk8KiB),
        (131072, ChunkSizingPreset::Chunk128KiB),
    ] {
        let scalar = Ram::new(window, window * 4);
        let vector = scalar.accelerated().expect("diagnosis requires AVX2");
        let fast = BoundaryFinder::new(&ChunkSizingProfile::for_preset(preset));
        let mut samples = vec![vec![Vec::new(); 3]; 3];
        let mut counts = [(0, 0); 3];
        for repetition in 0..8 {
            for offset in 0..3 {
                let variant = (offset + repetition) % 3;
                for (mode, mode_samples) in samples[variant].iter_mut().enumerate() {
                    let start = Instant::now();
                    let count = match variant {
                        0 => scan(&fast, &files, mode),
                        1 => scan(&scalar, &files, mode),
                        _ => scan(&vector, &files, mode),
                    };
                    let elapsed = start.elapsed().as_secs_f64();
                    counts[variant] = count;
                    if repetition > 0 {
                        mode_samples.push(elapsed);
                    }
                }
            }
        }
        assert_eq!(counts[1], counts[2]);
        for variant in 0..3 {
            for mode in 0..3 {
                let values = &mut samples[variant][mode];
                values.sort_by(f64::total_cmp);
                results.push(serde_json::json!({"window":window,
                    "variant":(["fastcdc","ram-scalar-early-exit","vram-avx2"][variant]),
                    "mode":(["boundary","boundary+chunk-hash","boundary+chunk+file-hash"][mode]),
                    "seconds":values[values.len()/2],"samples":values,"bytes":total,
                    "chunks":counts[variant].0,"boundary_fingerprint":counts[variant].1}));
            }
        }
        eprintln!("Completed window {window}");
    }
    std::fs::write(
        std::env::var("GUTCH_RAM_OUTPUT").unwrap(),
        serde_json::to_vec_pretty(&results).unwrap(),
    )
    .unwrap();
}
