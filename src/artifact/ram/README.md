# RAM / VectorCDC reference

Algorithm reference: UWASL/DedupBench, Apache-2.0, copyright WASL 2023.
Pinned commit: `8e2697cbf6332ac5da6dc615bfab82a720e820e4`.
Source: https://github.com/UWASL/dedup-bench/blob/8e2697cbf6332ac5da6dc615bfab82a720e820e4/dedup/src/chunking/ram_chunking.cpp
Paper: https://www.usenix.org/system/files/fast25-udayashankar.pdf

The Rust implementation in `../ram.rs` expresses the scalar RAM boundary rule
and implements bounded, unaligned AVX2 searches without upstream scratch arrays.
The size preset denotes the initial window: upstream's `ram_avg_block_size`
parameter would be this value plus 256. Maximum chunk size is four windows.
Empty inputs and SIMD tails are checked explicitly. Early exit upon maximum
byte 255 preserves the scalar rule. Scalar and AVX2 share `ram-v1` identity.
