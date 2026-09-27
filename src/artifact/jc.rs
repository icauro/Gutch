/*
 * Copyright (c) 2024 Gilles Chehade <gilles@poolp.org>
 *
 * Permission to use, copy, modify, and distribute this software for any
 * purpose with or without fee is hereby granted, provided that the above
 * copyright notice and this permission notice appear in all copies.
 *
 * THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
 * WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
 * MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
 * ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
 * WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
 * ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
 * OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
 */
//! Experimental JC kernels; compiled only into tests and benchmarks.
//! The Plakar preset reproduces go-cdc-chunkers v1.1.0. The scaled preset
//! uses paper-derived bit counts with explicitly chosen evenly spaced bits.

use super::chunk_stream::ChunkBoundary;
use super::gear::update;
#[path = "jc_gear.rs"]
mod table;

#[derive(Clone, Copy, Debug)]
pub(super) enum Parameters {
    Plakar,
    PaperScaled,
}

pub(super) struct JumpChunker {
    min: usize,
    max: usize,
    jump: usize,
    cut_mask: u64,
    jump_mask: u64,
}

impl JumpChunker {
    pub(super) fn new(min: usize, target: usize, max: usize, parameters: Parameters) -> Self {
        assert!(target.is_power_of_two() && target >= 64);
        assert!(0 < min && min < target && target < max);
        let (cut_mask, jump_mask) = match parameters {
            Parameters::Plakar => (0x590003570000, 0x590003560000),
            Parameters::PaperScaled => {
                let bits = target.ilog2() - 1;
                let step = 64 / bits;
                let mut mask = 0_u64;
                for index in 0..bits {
                    mask |= 1_u64 << (63 - index * step);
                }
                (mask, mask & (mask - 1))
            }
        };
        Self {
            min,
            max,
            jump: target / 2,
            cut_mask,
            jump_mask,
        }
    }
}

impl ChunkBoundary for JumpChunker {
    #[inline]
    fn max_size(&self) -> usize {
        self.max
    }

    #[inline]
    fn cut(&self, bytes: &[u8]) -> usize {
        let limit = bytes.len().min(self.max);
        let mut offset = self.min;
        let mut hash = 0;
        while offset < limit {
            hash = update(hash, bytes[offset], &table::GEAR);
            if hash & self.jump_mask == 0 {
                if hash & self.cut_mask == 0 {
                    return offset;
                }
                hash = 0;
                // Saturation to the live input also handles a jump past EOF.
                offset += self.jump.min(limit - offset);
            } else {
                offset += 1;
            }
        }
        offset.min(limit)
    }
}
