# JC chunking experiment — 2026-09-27

Implemented as test-only kernels. Production capture remains FastCDC v2020, normalization level 1. No repository format, persisted algorithm identifier, or default changed.

## Method

- Windows 11; AMD Ryzen 9 7900; rustc 1.97.1; release build; single thread.
- One warmup and 11 measured repetitions. Algorithm order rotates; medians are reported.
- Blender 5.2.0 Windows x64: 6,525 files, 944,682,178 bytes. Blender 5.2.1: 6,526 files, 944,579,836 bytes.
- Identical minimum/target/maximum sizes for each algorithm: target/4, target, target*4.
- Files are loaded before timing. Boundary-only scans, scans with per-chunk BLAKE3, and buffered streaming with BLAKE3 are measured separately.
- Reuse is the fraction of updated-build bytes whose chunk hash exists in the base build. New unique bytes exclude duplicate new chunks. Neither metric estimates compressed storage.
- These are warm, memory-resident kernel/pipeline measurements, not full capture/backup performance. The stream test forces all files through the shared reader; production also has a small-file path. CPU time and peak RSS were not measured separately.

## Blender results

| Target KiB | Kernel | Scan MiB/s | Scan + BLAKE3 MiB/s | Stream + BLAKE3 MiB/s | Mean chunk KiB | Reused updated bytes % | New unique MiB |
|---:|---|---:|---:|---:|---:|---:|---:|
| 8 | FastCDC | 3993 | 2077 | 1943 | 9.72 | 68.001 | 284.43 |
| 8 | JC Plakar | 6141 | 2223 | 2327 | 7.73 | 67.929 | 285.14 |
| 8 | JC paper-scaled | 4493 | 2134 | 2134 | 9.47 | 67.860 | 285.84 |
| 128 | FastCDC | 4310 | 2774 | 2322 | 80.94 | 65.135 | 312.43 |
| 128 | JC Plakar | 53362 | 5460 | 4348 | 59.82 | 63.295 | 328.51 |
| 128 | JC paper-scaled | 5169 | 2981 | 2524 | 79.44 | 64.830 | 315.04 |
| 1024 | FastCDC | 4686 | 2899 | 1869 | 133.37 | 61.358 | 348.09 |
| 1024 | JC Plakar | 275645 | 6249 | 2785 | 122.99 | 61.485 | 346.20 |
| 1024 | JC paper-scaled | 5532 | 3102 | 1952 | 132.45 | 62.083 | 341.56 |

Mean sizes include short final chunks and thousands of small files. Plakar uses fixed 11/10-bit masks while its jump grows with the requested target; its enormous boundary-only rate counts logically processed bytes, including skipped bytes, not memory bandwidth.

## Interface overhead

The frozen pre-interface stream in `tests/benchmarks/cdc_baseline.rs` is a test-only performance control. It uses the same unchanged FastCDC kernel. Across all nine dataset/size cases, generic streaming throughput changed from -0.13% to +0.47% versus that control. FastCDC boundaries remain identical to fastcdc 5.0.0 in differential tests. This supports no meaningful measured interface penalty on this host; it is not a universal performance guarantee.

An initial five-sample run showed a 5.58% synthetic streaming slowdown at 128 KiB. The 11-sample repeat reduced it to 0.02%; the initial result is not treated as a reproducible regression.

## Synthetic checks

| Input | Target KiB | Kernel | Scan + BLAKE3 MiB/s | Stream + BLAKE3 MiB/s | Reuse % |
|---|---:|---|---:|---:|---:|
| random-edits | 8 | FastCDC | 2045 | 1942 | 99.936 |
| random-edits | 8 | JC Plakar | 2322 | 2417 | 99.929 |
| random-edits | 8 | JC paper-scaled | 2193 | 2227 | 99.966 |
| random-edits | 128 | FastCDC | 2791 | 2583 | 99.396 |
| random-edits | 128 | JC Plakar | 6125 | 5752 | 98.477 |
| random-edits | 128 | JC paper-scaled | 3131 | 2972 | 98.401 |
| random-edits | 1024 | FastCDC | 2869 | 2399 | 95.809 |
| random-edits | 1024 | JC Plakar | 6775 | 4513 | 95.691 |
| random-edits | 1024 | JC paper-scaled | 3329 | 2717 | 95.842 |
| repeated | 8 | FastCDC | 2568 | 2394 | 100.000 |
| repeated | 8 | JC Plakar | 1885 | 1781 | 100.000 |
| repeated | 8 | JC paper-scaled | 1884 | 1782 | 100.000 |
| repeated | 128 | FastCDC | 2606 | 2465 | 100.000 |
| repeated | 128 | JC Plakar | 1888 | 1813 | 100.000 |
| repeated | 128 | JC paper-scaled | 1889 | 1813 | 100.000 |
| repeated | 1024 | FastCDC | 2574 | 2340 | 100.000 |
| repeated | 1024 | JC Plakar | 1872 | 1746 | 100.000 |
| repeated | 1024 | JC paper-scaled | 1874 | 1745 | 100.000 |

Random data is 64 MiB from xorshift64, seed 917. Its update inserts 17 bytes at three positions. Repeated data is 64 MiB of zeros with 17 zeros appended. Both JC presets scan zeros more slowly than the two-byte FastCDC kernel; they are not universal throughput improvements.

## Decision

Keep JC experimental. At 128 KiB, Plakar-compatible JC improves Blender scan-plus-hash throughput by about 97%, but reuse falls by 1.84 percentage points and new unique data grows by 16.08 MiB. Paper-scaled JC improves scan-plus-hash throughput by about 7.5%, with a 0.31-point reuse reduction and 2.61 MiB extra new unique data. At 1 MiB the paper-scaled variant is promising on this corpus: about 7% higher scan-plus-hash throughput and 6.53 MiB less new unique data. More workloads and full capture measurements would be needed before considering a default change.

## Correctness and reference

- 49 artifact tests passed; one explicit benchmark test is ignored during ordinary tests.
- Plakar-compatible JC matches 24 frozen Go-oracle cases covering all three target sizes and EOF boundaries.
- Both variants preserve bytes and chunk limits across short reads, empty inputs, repeated inputs, and random data.
- Production library release check passed. Existing FastCDC boundary-oracle tests cover all presets.
- The Plakar Gear table is shared by the two experimental presets; it differs from the retained Gutch table. Gear arithmetic is inlined; FastCDC keeps its two-byte scan.
- JC source and table carry the upstream ISC notice; the existing FastCDC-derived code retains MIT attribution.

References: [JC paper](https://doi.org/10.1109/TPDS.2023.3290770), [pinned Plakar JC source](https://github.com/PlakarKorp/go-cdc-chunkers/blob/b6c4abce19e9691184a0aceefc4e9f53fcfe79c8/chunkers/jc/jc.go). Paper-scaled masks use evenly spaced bits, one fewer embedded jump bit, and target/2 jumps; this is a documented experimental parameterization, not an asserted byte-identical author implementation.

Run `python tests/benchmarks/cdc_experiment.py --base PATH --updated PATH --repetitions 11` from the project root. Raw samples and metadata are under `tests/.artifacts/jc-experiment/`. The original five-sample run is retained as `results-5-samples.json`.
