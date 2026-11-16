# Gutch Chunkers

Status: Current

Scope: Selectable content-defined chunking algorithms and their comparison tools

## Selectable chunkers

The borrowed chunk stream uses a statically dispatched `ChunkBoundary` kernel.
FastCDC v2020 level 1 remains the default. Capture accepts `--chunker fastcdc|jc|vram256`;
`jc` is the Plakar go-cdc-chunkers v1.1.0 algorithm with its fixed masks and Gear
table. The paper-scaled variant remains test-only. Dispatch happens once per
file; the chunk loop stays statically dispatched. The shifted FastCDC Gear table
is derived at compile time.

The public API exposes `Chunker` and `SnapshotCaptureRequest.chunker`. Repository
profiles and snapshot metadata record `fastcdc-v2020` or `jc-v1.1.0`.
Reconstruction and repair use stored boundaries, with no chunker selection.
Recapturing identical content retains the existing snapshot and its algorithm;
new snapshots use the requested algorithm even for unchanged files. Changing
algorithms can reduce reuse of chunks from older snapshots.

Run `python gutch.py benchmark --fixture 16_gutch_fastcdc_vs_jc --product PATH`
from the Gutch directory. The fixture compares three full-flow runs per algorithm
with FastCDC at 128 KiB and JC at 256 KiB, with identical compression and bundle
settings. Equal target
sizes do not imply equal actual average chunk sizes. Each lane has its own repo.

`tests/benchmarks/cdc_experiment.py` compares the frozen pre-interface adapter,
current FastCDC, Plakar-compatible JC, and paper-scaled JC. It accepts `--base`
and `--updated` build directories, `--name` for a dataset label, and
`--repetitions` (default five). Run from
the project root with `python tests/benchmarks/cdc_experiment.py --base PATH
--updated PATH`. Evidence is written to `.artifacts/tests/jc-experiment/<name>/`, so datasets
do not overwrite each other. The base build is resident during measurement;
updated files are loaded one at a time during untimed reuse analysis.

The experiment reports boundary scanning, scanning plus BLAKE3, streaming plus
BLAKE3, chunk-size statistics, and byte reuse against the previous build.
Input loading is excluded, bytes are resident in memory, and variants rotate
order after a warmup. These are single-thread kernel/pipeline measurements,
not complete backup or capture throughput. Reuse checks chunk contents across
files; it does not estimate compressed repository size.

Plakar compatibility is pinned to go-cdc-chunkers v1.1.0, commit
`b6c4abce19e9691184a0aceefc4e9f53fcfe79c8`. Its fixed cut/jump masks contain 11/10
bits. Paper-scaled JC uses `log2(target)-1` cut bits, one fewer jump bit,
evenly spaced mask positions, and target/2 jumps. Both share the JC loop and
Plakar Gear table; the paper preset is not advertised as an exact upstream
implementation. That table differs from Gutch's and must remain distinct.

The 24 frozen Go-oracle cases in `src/04_artifact/jc_tests.rs` use xorshift64
(13-left, 7-right, 17-left), seed 917, emitting the low byte after each update.
Each case records its exact size parameters, input length, and cut lengths.
They were generated using the pinned Go library's `jc-v1.1.0` reader API.
The tests run offline. The reference Gear table and JC source retain the
upstream ISC notices; Gutch's FastCDC table retains its upstream MIT notice.

## RAM and VRAM-256

`--chunker vram256` selects RAM byte-comparison boundaries. AVX2 is detected at
runtime once per file, with scalar RAM on unsupported CPUs. Both paths use the
shared `ChunkBoundary` interface and produce the same persisted `ram-v1` identity.
The size preset denotes the initial maximum-search window (and effective minimum),
not FastCDC's average-size mask. Maximum chunk size remains four times the preset.
The cutoff byte belongs to the following chunk. Files shorter than the window
form one chunk; a search without a qualifying byte cuts at the maximum or EOF.
Unaligned vector loads stay inside the live input and scalar code handles tails.

Run `python gutch.py benchmark --fixture 17_gutch_fastcdc_jc_vram256 --product PATH`
with a directory containing the builds. The fixture compares FastCDC 128 KiB,
JC 256 KiB, and VRAM-256 with a 128 KiB window, using zstd 3 and 8 MiB bundles.
It runs three repetitions with reconstruction and corruption repair. Targets do
not normalize actual chunk counts or reuse. See the implementation's
[reference and provenance](../../src/04_artifact/ram/README.md).
