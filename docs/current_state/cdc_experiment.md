# JC chunking experiment — 2026-09-27

These measurements were collected with test-only kernels. Plakar-compatible JC is now selectable as `--chunker jc`, with persisted `jc-v1.1.0` identity; FastCDC v2020 normalization level 1 remains the default. Paper-scaled JC remains test-only. See [full-flow fixture](12_performance_and_development.md#selectable-jump-chunking).

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

## Hollow Knight follow-up — 2026-09-27

Fixture: `HollowKnight/hk-v0.0.1` to `hk-v1.1.0` (earliest to latest; the intermediate `hk-v1.0.0` was not measured separately). Both builds contain 1,785 files; base size is 5,197,398,400 bytes and updated size is 5,231,995,691 bytes. Hardware, release build, size bounds, warmup, and 11-sample rotating median methodology match the Blender experiment.

The timed base is resident in memory. Updated files are read one at a time during untimed reuse analysis, avoiding simultaneous residency of the two approximately 5 GB builds. The largest updated file is 255,055,152 bytes. No input files are modified.

| Target KiB | Kernel | Scan MiB/s | Scan + BLAKE3 MiB/s | Stream + BLAKE3 MiB/s | Mean chunk KiB | Reused updated bytes % | New unique MiB |
|---:|---|---:|---:|---:|---:|---:|---:|
| 8 | FastCDC | 3530 | 2135 | 2033 | 14.40 | 90.549 | 458.52 |
| 8 | JC Plakar | 4115 | 2089 | 2076 | 10.38 | 90.925 | 431.48 |
| 8 | JC paper-scaled | 3630 | 2056 | 2018 | 12.51 | 90.341 | 459.89 |
| 128 | FastCDC | 3518 | 2387 | 2193 | 230.23 | 81.591 | 918.19 |
| 128 | JC Plakar | 15501 | 4479 | 3940 | 100.18 | 84.448 | 775.45 |
| 128 | JC paper-scaled | 3426 | 2342 | 2127 | 213.57 | 80.925 | 951.45 |
| 1024 | FastCDC | 3717 | 2480 | 2025 | 1129.41 | 71.255 | 1434.23 |
| 1024 | JC Plakar | 33703 | 5304 | 3315 | 625.15 | 72.031 | 1395.26 |
| 1024 | JC paper-scaled | 3428 | 2363 | 1955 | 1263.53 | 68.369 | 1578.22 |

At 128 KiB, Plakar JC improves scan-plus-BLAKE3 throughput by 87.6% and reduces new unique data by 142.74 MiB versus FastCDC. At 1 MiB, the corresponding figures are +113.9% and -38.97 MiB. At 8 KiB, it is 2.1% slower in scan-plus-hash but produces 27.04 MiB less new unique data.

Paper-scaled JC is 1.9% slower at 128 KiB with 33.27 MiB more new unique data; at 1 MiB it is 4.7% slower with 143.99 MiB more new unique data. It does not improve the measured tradeoff on this fixture.

Plakar JC wins both measured axes at the larger targets here, unlike the Blender 128 KiB storage tradeoff. Its actual chunk sizes are also smaller. These results do not establish performance at matched actual average sizes, compressed repository size, or full capture/transfer/repair performance. FastCDC remains the default.

All nine requested algorithm/size combinations completed; each has 11 measured samples for each of three timing modes. The frozen FastCDC adapter remains an additional interface control. Raw results, metadata, and the native log are in `tests/.artifacts/jc-experiment/hollow-knight/`. Existing Blender evidence was retained.

Reproduce from the project root:

```text
python tests/benchmarks/cdc_experiment.py --name hollow-knight --base /path/to/HollowKnight/hk-v0.0.1 --updated /path/to/HollowKnight/hk-v1.1.0 --repetitions 11
```


## Production integration validation

The `16_gutch_fastcdc_vs_jc` fixture ran three repetitions per algorithm against
Blender 5.2.0 and 5.2.1: 13,051 input files and 1,889,262,014 bytes across both
builds. Each lane used a fresh repository, 128 KiB target, zstd level 3 and
8 MiB bundles. Profiling was off; OS cache was uncontrolled, with no explicit
warm-up. These are full-flow observations, not kernel throughput measurements.

Median seconds sum both builds within each repetition:

| Chunker | Capture s | Reconstruction s | Repair s | Repository MiB |
|---|---:|---:|---:|---:|
| fastcdc | 2.543 | 3.764 | 0.787 | 527.64 |
| jc | 2.716 | 3.812 | 0.806 | 534.50 |

All six measured lanes passed independent reconstruction and corruption-repair
comparison. Evidence: `tests/.artifacts/jc-full-flow/2026.09.27-source-16_gutch_fastcdc_vs_jc-002/`.
An earlier run completed five lanes before Windows returned access denied during
the final reconstruction. The fresh complete run above did not reproduce it;
its cause is unconfirmed. Do not include the incomplete run in these medians.

Validation also passed 317 Rust library tests, eight public API/installation
integration tests, 84 Gutch harness tests, six shared-suite tests, and the desktop
core compile check. Public API and CLI coverage verifies persisted `jc-v1.1.0`
identity. Changing chunkers with an unchanged file in a new snapshot is covered.


## Source-handle reuse follow-up

A bounded reader now retains the current source handle during bundle loading
and compression probing. It still seeks and verifies every chunk; no chunk
boundaries, packing order, or compression settings change. A source change,
read failure, or truncated range still fails capture. The reader retains at most
one handle per active job; this primarily helps multiple chunks from one file.

The old and new release binaries were alternated on the same Blender pair,
using detailed profiling, automatic 12 workers, one warm-up per combination and
three measured repetitions. Times below are medians of totals across both builds.
OS cache was uncontrolled. Worker seconds aggregate parallel work, not wall time.

| Chunker | Capture before / after s | Payload loading before / after worker-s | Packaging before / after s |
|---|---:|---:|---:|
| FastCDC | 2.644 / 2.548 | 3.315 / 2.369 | 0.897 / 0.781 |
| JC | 2.736 / 2.650 | 4.279 / 2.942 | 0.978 / 0.853 |

Total capture improved 3.7% for FastCDC and 3.1% for JC in this run. Payload
loading improved 28.5% and 31.2%, respectively. JC still takes longer overall
on this input. Published artifacts, blueprints and chunk maps were byte-identical
in all six measured before/after repository pairs.
Evidence: `tests/.artifacts/handle-reuse-comparison/`; old binary retained under
`tests/.artifacts/jc-timing-diagnosis/gutch-before-handle-reuse.exe`.

Validation passed 318 library tests and a full Blender capture, reconstruction,
and corruption-repair cycle for both chunkers. The new reader test covers source
switching, backward seeks, changed contents, and truncated input.


## FastCDC 128 KiB versus JC 256 KiB on Blender

The full-flow fixture now uses FastCDC at 128 KiB (32–512 KiB bounds) and JC at
256 KiB (64–1024 KiB bounds). Both retain zstd 3, 8 MiB bundles, automatic workers,
and source-handle reuse. This changes JC's complete size profile, not just its
jump distance. It is a near-count comparison, not an exactly matched-count test.

Three alternating repetitions per lane passed capture, independent reconstruction
comparison, corruption and repair. Profiling was off, no explicit warm-up was
added, and OS cache was uncontrolled. Medians sum both builds per repetition.

| Metric | FastCDC 128 KiB | JC 256 KiB |
|---|---:|---:|
| Capture seconds | 2.473 | 2.379 |
| Reconstruction seconds | 3.538 | 3.514 |
| Repair seconds | 0.781 | 0.767 |
| Repository MiB | 527.64 | 539.85 |
| First build unique chunks | 10,504 | 10,034 |
| Updated build required unique chunks | 10,498 | 10,064 |
| Update new unique chunks | 1,890 | 1,809 |
| Update new raw bytes to package | 327,602,738 | 357,507,134 |

JC capture was 3.8% faster, with 2.3% more repository bytes (+12.21 MiB).
The first-build unique chunk count is 4.5% below FastCDC. Similar counts do not
imply similar reuse: JC packages 28.52 MiB more new raw data on the update.
These three samples support a small observed capture gain on this fixture,
not a universal speed advantage. The repository size difference is deterministic
across these runs. Prior 128-versus-128 measurements above retain their original
settings and should not be treated as the current fixture configuration.

Evidence: `tests/.artifacts/jc-256-blender/2026.09.27-source-16_gutch_fastcdc_vs_jc-001/`.
Per-command capture statistics and all payloads were retained for inspection.


## FastCDC 128 KiB versus JC 256 KiB on Hollow Knight

The same full-flow fixture and release binary were run on all three available
builds: `hk-v0.0.1`, `hk-v1.0.0`, and `hk-v1.1.0`. Total input was
15,661,389,815 bytes across 5,355 files. Settings remained FastCDC 128 KiB versus
JC 256 KiB, zstd 3, 8 MiB bundles, automatic workers, profiling off, three
alternating repetitions and no explicit warm-up. OS cache was uncontrolled.
Unlike the earlier kernel experiment, this run includes the middle build.

Medians below sum the three builds within each repetition:

| Chunker | Capture s | Reconstruction s | Repair s | Repository MiB |
|---|---:|---:|---:|---:|
| fastcdc | 7.391 | 9.568 | 2.456 | 1165.38 |
| jc | 7.897 | 9.882 | 2.369 | 1189.00 |

JC capture was 6.9% slower, reconstruction 3.3% slower, repair 3.5% faster,
and repository size 2.0% larger (+23.62 MiB). Capture totals ranged from
7.154–9.991 seconds for FastCDC and 7.745–10.199 seconds for JC; three samples
with overlapping ranges do not establish a precise general performance gap.

| Build | FastCDC required unique chunks | JC required unique chunks | FastCDC new chunks | JC new chunks |
|---|---:|---:|---:|---:|
| 0.0.1 | 21,562 | 26,849 | 21,562 | 26,849 |
| 1.0.0 | 21,779 | 26,988 | 3,996 | 4,079 |
| 1.1.0 | 21,780 | 26,966 | 1,081 | 1,349 |

The initial JC unique chunk count is 24.5% higher: the size pairing that nearly
matched counts on Blender does not do so on Hollow Knight. These full-flow
measurements have no internal stage timings, so they do not separately attribute
the difference to chunk loading, compression, or other stages.

All six lanes passed independent reconstruction and corruption-repair checks.
Evidence and retained per-command statistics:
`tests/.artifacts/jc-256-hollow-knight/2026.09.27-HollowKnight-16_gutch_fastcdc_vs_jc-001/`.


## Payload ownership, JC loop and stream prototypes on Blender

Three isolated prototypes were compared with the existing release binary on the
two Blender builds, with FastCDC 128 KiB and JC 256 KiB. Other settings stayed
zstd 3, 8 MiB bundles and automatic workers. Detailed profiling was enabled.
One warm-up per combination was excluded, then nine measured repetitions rotated
binary order and alternated chunker order. OS cache and host load were uncontrolled.

Median capture seconds, summing both builds within each repetition:

| Prototype | FastCDC | JC |
|---|---:|---:|
| Baseline | 2.613 | 2.499 |
| Own loaded buffers directly | 2.483 | 2.390 |
| Unroll JC scan to two bytes | 2.570 | 2.407 |
| Try buffered cuts before refill | 2.589 | 2.417 |

Direct ownership removes the per-chunk Vec-to-Arc allocation/copy and the unused
shared payload abstraction. It reduced capture by 5.0% for FastCDC and 4.4% for JC
in this run. Median payload-load worker time fell from 2.500 to 2.136 seconds for
FastCDC and from 2.352 to 2.003 seconds for JC (about 15%). Chunk verification,
packing order and artifact formats remain unchanged.

The stream prototype tries a boundary cut in the remaining buffer before moving
bytes and refilling. A failed attempt repeats work after refill. In a separate
five-measured-repeat follow-up, adding it to direct ownership regressed capture:
FastCDC 2.498 to 2.567 seconds (+2.7%); JC 2.364 to 2.441 (+3.2%). It was discarded.
The unrolled loop was also discarded: its small scan improvement was not clearly
separated from variation in the unchanged FastCDC control, and it duplicated
the JC step. These are workload observations rather than universal conclusions.

Only direct buffer ownership is retained. All three prototypes matched baseline
published artifact bytes, snapshot blueprints and chunk maps in a two-build
repository comparison for each chunker. Loop and stream prototypes passed the
50 artifact tests; the combined prototype passed all 318 library tests.
Evidence, prototype sources, binaries and per-command measurements are retained
under `tests/.artifacts/jc-path-prototypes/`.

The retained ownership-only implementation also passed all 318 library tests
and the Blender full-flow fixture (one validation repetition per chunker),
including independent reconstruction comparison and corruption repair.


## RAM / VRAM-256 on the four-snapshot Blender fixture

Implemented scalar RAM and AVX2 VRAM-256 through the shared `ChunkBoundary`
interface, retaining BLAKE3, packaging, reconstruction and repair. Runtime AVX2
detection happens once per file; unsupported CPUs use scalar RAM. Both paths
persist `ram-v1` and have identical boundaries. RAM uses a maximum-byte search
and a >= byte comparison, with no Gear table. The preset specifies the initial
window, equivalent to DedupBench's `ram_avg_block_size` minus 256.
The reference is pinned in `src/artifact/ram/README.md` with its Apache-2.0 license.

The available `Blender_4_snapshot` dataset contains Windows x64 Blender 4.4.0,
4.5.0, 5.0.0 and 5.1.0: 3,794,868,170 bytes across 23,371 files. Three rotating
repetitions used FastCDC 128 KiB, JC 256 KiB, and VRAM-256 with a 128 KiB window
and 512 KiB maximum. All used zstd 3, 8 MiB bundles, automatic workers, profiling
off, and no explicit warm-up. AVX2 was detected on the benchmark host. OS cache
was uncontrolled. Medians sum all four builds within each repetition.

| Chunker | Capture s | Reconstruction s | Repair s | Repository MiB |
|---|---:|---:|---:|---:|
| fastcdc | 5.224 | 7.379 | 1.581 | 1191.58 |
| jc | 5.714 | 8.551 | 1.559 | 1220.15 |
| vram256 | 5.460 | 9.759 | 1.586 | 1214.04 |

VRAM capture was 4.5% slower than FastCDC and 4.4% faster than JC. VRAM used
1.9% more repository bytes than FastCDC and reconstructed 32.3% slower.
Capture ranges overlap: FastCDC 5.142-6.440 s, JC 5.509-5.931 s, VRAM
5.292-6.033 s. Three samples do not establish a precise general speed gap.
These full-flow runs have no internal stage timings and do not measure kernel
speed independently. FastCDC remains the default.

Required unique chunk counts by version (4.4, 4.5, 5.0, 5.1):

- FastCDC: 9,461 / 9,687 / 10,054 / 10,533.
- JC: 9,266 / 9,362 / 9,783 / 10,169.
- VRAM: 10,411 / 10,690 / 11,034 / 11,615.

All nine lanes passed independent reconstruction and corruption-repair checks.
Validation also passed 320 Rust library tests, three public API tests including
both JC and RAM CLI/metadata round trips, 72 benchmark/boundary harness tests, and six shared-suite tests.
The RAM tests compare scalar and AVX2 with an independent literal reference,
covering empty inputs, unaligned slices, vector tails, repeated bytes, all presets,
short reads and small/streaming capture paths. No global AVX2 compiler flag is used.
Evidence and per-command statistics:
`tests/.artifacts/vram-blender-four/2026.09.27-Blender-f5c6014c-17_gutch_fastcd-12342793-001/`.


## VectorCDC paper-claim diagnosis

A separate diagnostic isolates boundary detection from hashing and full capture.
`tests/benchmarks/ram_diagnosis.py --product PATH` requires an AVX2 CPU, g++ and
Cargo. It loads the four-snapshot Blender input into memory before timing, runs
one worker, excludes one warm-up, and rotates seven measured repetitions at 8 KiB
and 128 KiB. Throughputs below use decimal GB/s of logical input, including bytes
skipped by a chunker. High numbers do not represent physical memory bandwidth.

The C++ reference kernels were extracted from pinned DedupBench commit
`8e2697cbf6332ac5da6dc615bfab82a720e820e4`, with `-O3 -mavx2 -std=c++17`.
The original range scan omits a final tail shorter than 32 bytes. An additional
variant checks that tail. Its boundary counts and rolling fingerprints match the
reference scalar and both Gutch RAM implementations. The original vector version
produces 57 fewer chunks at 8 KiB and 11 fewer at 128 KiB on this input.
No production boundary rule was changed.

Boundary-only median throughput:

| Kernel | 8 KiB GB/s | 128 KiB GB/s |
|---|---:|---:|
| Gutch FastCDC | 3.64 | 4.39 |
| Reference scalar with auto-vectorization disabled | 2.86 | 2.97 |
| Reference scalar source compiled at O3 | 47.41 | 54.20 |
| Reference AVX2 as written | 27.16 | 19.99 |
| Reference AVX2 with corrected tail | 27.47 | 19.78 |
| Gutch scalar RAM, early exit at maximum byte 255 | 5.43 | 11.75 |
| Gutch AVX2 RAM, early exit at maximum byte 255 | 55.11 | 198.07 |

GCC auto-vectorizes the scalar-looking source at O3; saved assembly contains
`vpmaxub`. The explicitly non-vectorized control separates that compiler effect.
Gutch's early exit can avoid most of the initial window, while preserving the
maximum-byte rule. The reference scans the full window with a scratch-array tree.
This explains why a boundary-equivalent implementation need not have the same
speed. Compiler, hardware, workload and size differences remain; this is not an
exact reproduction of the paper's testbed.

At 128 KiB, boundary plus chunk BLAKE3 falls to 2.88 GB/s for FastCDC and
6.24 GB/s for Gutch VRAM. Including both chunk and full-file BLAKE3 lowers these
to 2.10 and 3.41 GB/s. At 8 KiB, both-hash throughput is 1.16 and 1.69 GB/s.
Hashing still reads every byte that the boundary kernel can skip.

A separate capture diagnostic used automatic workers, detailed profiling,
one warm-up and three rotating measured repetitions. Each value sums four
snapshots before taking its median. FastCDC stayed at 128 KiB, JC at 256 KiB,
and VRAM at a 128 KiB window, with zstd 3 and 8 MiB bundles.

| Stage, seconds | FastCDC | JC | VRAM |
|---|---:|---:|---:|
| Scan and hashing | 0.708 | 0.580 | 0.568 |
| Artifact packaging | 1.744 | 1.754 | 1.767 |
| Outside build pipeline | 2.636 | 2.673 | 2.669 |
| Whole capture command | 5.126 | 5.016 | 5.014 |

Phase medians are independent and need not sum to command medians. Source
inventory spans alone total approximately 1.86 seconds for FastCDC and VRAM;
these are included in outside-pipeline work, not additional time. Inclusive
nested trace spans must not be added together. Packaging also includes planning,
loading, compression and staged writes.

VRAM saves about 0.14 seconds of scan time, only about 2.7% of the baseline whole
command. Even eliminating the entire scan stage would remove only about 14% of
that time. It was about 2.2% faster overall in this diagnostic run, whereas the
previous full-flow run had the opposite small difference. That variation does
not negate the large, directly measured boundary speedup. It does mean that the
current evidence does not establish a consistent end-to-end winner.

Evidence: `tests/.artifacts/vram-diagnosis/` (native timings, assembly, compiler
metadata and boundary fingerprints) and `tests/.artifacts/vram-profiled-capture/`
(per-command profiling, warm-up and samples). No production optimization was
applied by this diagnosis.


## Four-snapshot Blender comparison with LZ4 level 1

Repeated the same three-lane full-flow comparison with LZ4 level 1 in every lane.
FastCDC stayed at 128 KiB, JC at 256 KiB and VRAM at a 128 KiB window; bundles
remained 8 MiB. The input was Blender 4.4.0, 4.5.0, 5.0.0 and 5.1.0 on Windows
x64. Three rotating repetitions used profiling off and no explicit warm-up;
OS cache and host load were uncontrolled. All nine lanes passed independent
reconstruction and corruption-repair validation. Compression settings were
confirmed in the saved run manifest. The normal Zstd fixture was not modified.

Medians sum all four builds within each repetition:

| Chunker | Capture s | Reconstruction s | Repair s | Repository MiB |
|---|---:|---:|---:|---:|
| fastcdc | 5.216 | 7.973 | 1.536 | 1577.61 |
| jc | 5.340 | 7.051 | 1.503 | 1614.80 |
| vram256 | 5.485 | 8.268 | 1.570 | 1605.54 |

Capture ranges were FastCDC 4.621-5.445 s, JC 5.143-5.651 s and VRAM
4.627-5.831 s. VRAM's capture median was 5.1% slower than FastCDC's, but the
ranges overlap substantially. LZ4 did not demonstrate a consistent VRAM capture
advantage. FastCDC's capture median was effectively unchanged from the earlier
Zstd run, with about 32% more stored bytes. These are separate runs, not paired
codec trials; internal stage timings were disabled, so no causal timing
breakdown is inferred from them.

The run-specific fixture and retained evidence are under
`tests/.artifacts/lz4-chunker-comparison/`; results directory:
`2026.09.27-Blender-f5c6014c-17_gutch_fastcd-e7223052-001/`.


## LZ4 level 1 with all chunkers at 256 KiB

Repeated the four-snapshot Blender full-flow comparison with `chunk_256KiB` in
all three lanes, retaining LZ4 level 1 and 8 MiB bundles. FastCDC and JC have
64 KiB minimum and 1 MiB maximum; RAM has a 256 KiB search window and 1 MiB
maximum. Equal presets therefore do not imply equal chunk distributions.
Three rotating repetitions used profiling off, no explicit warm-up, and
uncontrolled OS cache and host load. Saved lane profiles confirm these settings.
All nine lanes passed independent reconstruction and corruption-repair checks.

Medians sum all four builds within each repetition:

| Chunker | Capture s | Reconstruction s | Repair s | Repository MiB |
|---|---:|---:|---:|---:|
| fastcdc | 6.116 | 10.332 | 1.564 | 1609.82 |
| jc | 8.634 | 10.122 | 1.535 | 1614.80 |
| vram256 | 6.179 | 7.273 | 1.510 | 1635.38 |

Capture ranges were FastCDC 4.597-8.959 s, JC 5.178-10.243 s and VRAM
5.616-7.732 s. This run had large timing variability; the median ordering is
not a reliable general performance ranking. In particular, JC's configuration
was unchanged from the previous run while its timing changed substantially.
Repository sizes were stable. Initial required unique chunk counts were 7,173
for FastCDC, 9,266 for JC, and 7,696 for VRAM.

The run-specific fixture and retained evidence are under
`tests/.artifacts/lz4-all-256-comparison/`; results directory:
`2026.09.27-Blender-f5c6014c-all_chunkers_25-e42e5000-001/`. No production settings or algorithms changed.
