//! Scalar RAM and AVX2 VectorCDC with identical boundaries.
//! Algorithm reference: UWASL/dedup-bench, commit
//! 8e2697cbf6332ac5da6dc615bfab82a720e820e4, ram_chunking.cpp (Apache-2.0).
//! This Rust implementation uses register reductions rather than scratch arrays.
//! Our preset is the window size (upstream ram_avg_block_size minus 256).

use super::chunk_stream::ChunkBoundary;

pub(crate) struct Ram<const SIMD: bool> {
    window: usize,
    maximum: usize,
}

impl Ram<false> {
    pub(crate) fn new(window: usize, maximum: usize) -> Self {
        assert!(window > 0 && window <= maximum);
        Self { window, maximum }
    }

    pub(crate) fn accelerated(&self) -> Option<Ram<true>> {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        if std::is_x86_feature_detected!("avx2") {
            return Some(Ram {
                window: self.window,
                maximum: self.maximum,
            });
        }
        None
    }
}

impl<const SIMD: bool> ChunkBoundary for Ram<SIMD> {
    fn max_size(&self) -> usize {
        self.maximum
    }

    #[inline]
    fn cut(&self, bytes: &[u8]) -> usize {
        let limit = bytes.len().min(self.maximum);
        if limit <= self.window {
            return limit;
        }
        let bytes = &bytes[..limit];
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        if SIMD {
            // Ram<true> is constructed only after runtime AVX2 detection.
            return unsafe { cut_avx2(bytes, self.window) };
        }
        cut_scalar(bytes, self.window)
    }
}

fn cut_scalar(bytes: &[u8], window: usize) -> usize {
    let mut maximum = 0;
    for &byte in &bytes[..window] {
        maximum = maximum.max(byte);
        // No later byte can increase an unsigned byte maximum of 255.
        if maximum == u8::MAX {
            break;
        }
    }
    bytes[window..]
        .iter()
        .position(|&byte| byte >= maximum)
        .map_or(bytes.len(), |position| window + position)
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
unsafe fn cut_avx2(bytes: &[u8], window: usize) -> usize {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;
    // Every unaligned load stays inside the checked window or live input.
    // The caller has verified AVX2 support; no architecture-wide flags are needed.
    unsafe {
        let mut offset = 0;
        let mut maxima = _mm256_setzero_si256();
        let all_ones = _mm256_set1_epi8(-1);
        let mut saturated = false;
        while offset + 32 <= window {
            let values = _mm256_loadu_si256(bytes.as_ptr().add(offset).cast());
            maxima = _mm256_max_epu8(maxima, values);
            offset += 32;
            if _mm256_movemask_epi8(_mm256_cmpeq_epi8(maxima, all_ones)) != 0 {
                saturated = true;
                break;
            }
        }
        let maximum = if saturated {
            255
        } else {
            let mut lanes = [0u8; 32];
            _mm256_storeu_si256(lanes.as_mut_ptr().cast(), maxima);
            lanes
                .into_iter()
                .chain(bytes[offset..window].iter().copied())
                .max()
                .unwrap_or(0)
        };
        let threshold = _mm256_set1_epi8(maximum as i8);
        offset = window;
        while offset + 32 <= bytes.len() {
            let values = _mm256_loadu_si256(bytes.as_ptr().add(offset).cast());
            // max(value, threshold) == value is an unsigned >= comparison.
            let matches = _mm256_cmpeq_epi8(_mm256_max_epu8(values, threshold), values);
            let mask = _mm256_movemask_epi8(matches) as u32;
            if mask != 0 {
                return offset + mask.trailing_zeros() as usize;
            }
            offset += 32;
        }
        bytes[offset..]
            .iter()
            .position(|&byte| byte >= maximum)
            .map_or(bytes.len(), |position| offset + position)
    }
}

#[cfg(test)]
#[path = "ram_tests.rs"]
mod tests;
