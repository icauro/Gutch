// Copyright (c) 2025 Nathan Fiedler
// SPDX-License-Identifier: MIT
// Derived from fastcdc 5.0.0, v2020. See README.md and LICENSE in this directory.

const MASKS: [u64; 26] = [
    0,                  // padding
    0,                  // padding
    0,                  // padding
    0,                  // padding
    0,                  // padding
    0x0000000001804110, // unused except for NC 3
    0x0000000001803110, // 64B
    0x0000000018035100, // 128B
    0x0000001800035300, // 256B
    0x0000019000353000, // 512B
    0x0000590003530000, // 1KB
    0x0000d90003530000, // 2KB
    0x0000d90103530000, // 4KB
    0x0000d90303530000, // 8KB
    0x0000d90313530000, // 16KB
    0x0000d90f03530000, // 32KB
    0x0000d90303537000, // 64KB
    0x0000d90703537000, // 128KB
    0x0000d90707537000, // 256KB
    0x0000d91707537000, // 512KB
    0x0000d91747537000, // 1MB
    0x0000d91767537000, // 2MB
    0x0000d93767537000, // 4MB
    0x0000d93777537000, // 8MB
    0x0000d93777577000, // 16MB
    0x0000db3777577000, // unused except for NC 3
];

use super::gear::{GEAR, GEAR_LS};

pub(super) fn cut(
    source: &[u8],
    min_size: usize,
    avg_size: usize,
    max_size: usize,
    mask_s: u64,
    mask_l: u64,
) -> (u64, usize) {
    let gear = &GEAR;
    let gear_ls = &GEAR_LS;
    let mask_s_ls = mask_s << 1;
    let mask_l_ls = mask_l << 1;

    // The two-byte scan below tests candidates in pairs starting at
    // `min_size`/`center`/`max_size`; an odd value truncates when halved and
    // silently shifts those boundaries by one byte (see issue #52). Only
    // `remaining`, the leftover length of the source itself, is exempt: a
    // file's size is not under the caller's control, so an odd final window
    // is handled below rather than rejected.
    debug_assert!(min_size.is_multiple_of(2), "min_size must be even");
    debug_assert!(avg_size.is_multiple_of(2), "avg_size must be even");
    debug_assert!(max_size.is_multiple_of(2), "max_size must be even");
    let mut remaining = source.len();
    if remaining <= min_size {
        return (0, remaining);
    }
    let mut center = avg_size;
    if remaining > max_size {
        remaining = max_size;
    } else if remaining < center {
        center = remaining;
    }
    // Narrow once to the live window. Note: this does NOT remove the per-byte
    // source bounds check (the compiler won't prove `2*index+1 < remaining`
    // here); 4 such checks remain. They are free in practice — the loop is
    // latency-bound on the hash chain, not throughput-bound. The real win is
    // the `&[u64; 256]` GEAR tables above, which drop the table-lookup checks.
    let src = &source[..remaining];
    let limit1 = center / 2;
    let limit2 = remaining / 2;
    let mut index = min_size / 2;
    let mut hash: u64 = 0;
    while index < limit1 {
        let a = index * 2;
        hash = (hash << 2).wrapping_add(gear_ls[src[a] as usize]);
        if (hash & mask_s_ls) == 0 {
            return (hash, a);
        }
        hash = hash.wrapping_add(gear[src[a + 1] as usize]);
        if (hash & mask_s) == 0 {
            return (hash, a + 1);
        }
        index += 1;
    }
    while index < limit2 {
        let a = index * 2;
        hash = (hash << 2).wrapping_add(gear_ls[src[a] as usize]);
        if (hash & mask_l_ls) == 0 {
            return (hash, a);
        }
        hash = hash.wrapping_add(gear[src[a + 1] as usize]);
        if (hash & mask_l) == 0 {
            return (hash, a + 1);
        }
        index += 1;
    }
    // If all else fails, return the largest chunk. This will happen with
    // pathological data, such as all zeroes. When `remaining` is odd, its
    // last byte was never part of a tested pair; fold it into the hash (the
    // same accumulation a scalar byte-at-a-time scan would do) so the
    // returned fingerprint reflects the whole chunk, without testing it as
    // its own boundary candidate.
    if remaining % 2 == 1 {
        hash = super::gear::update(hash, src[remaining - 1], gear);
    }
    (hash, remaining)
}

/// Gutch uses upstream v2020 normalization level 1 and the default gear tables.
pub(super) fn select_masks(average: usize) -> (u64, u64) {
    let bits = (average as f64).log2().round() as usize;
    (MASKS[bits + 1], MASKS[bits - 1])
}
