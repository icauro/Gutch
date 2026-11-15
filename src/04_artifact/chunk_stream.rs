//! Gutch's level-1 FastCDC boundary finder and borrowed streaming adapter.

use crate::artifact::profiles::ChunkSizingProfile;
use crate::progress::OperationControl;
use std::io::ErrorKind;
use std::io::Read;

/// Stateless boundary kernels must return 1..=min(input.len(), max_size())
/// for nonempty input, and zero for empty input.
pub(crate) trait ChunkBoundary {
    fn max_size(&self) -> usize;
    fn cut(&self, bytes: &[u8]) -> usize;
}

pub(crate) struct BoundaryFinder {
    min: usize,
    average: usize,
    max: usize,
    strict_mask: u64,
    relaxed_mask: u64,
}

impl BoundaryFinder {
    pub(crate) fn new(profile: &ChunkSizingProfile) -> Self {
        let average = profile.avg_chunk_size_bytes() as usize;
        let (strict_mask, relaxed_mask) = super::fastcdc_v2020::select_masks(average);
        Self {
            min: profile.min_chunk_size_bytes() as usize,
            average,
            max: profile.max_chunk_size_bytes() as usize,
            strict_mask,
            relaxed_mask,
        }
    }
}

impl ChunkBoundary for BoundaryFinder {
    #[inline]
    fn max_size(&self) -> usize {
        self.max
    }

    #[inline]
    fn cut(&self, bytes: &[u8]) -> usize {
        super::fastcdc_v2020::cut(
            bytes,
            self.min,
            self.average,
            self.max,
            self.strict_mask,
            self.relaxed_mask,
        )
        .1
    }
}

pub(crate) struct BorrowedChunk<'a> {
    pub(crate) offset: u64,
    pub(crate) data: &'a [u8],
}

pub(crate) struct ChunkStream<R, C = BoundaryFinder> {
    reader: R,
    boundary: C,
    buffer: Vec<u8>,
    start: usize,
    end: usize,
    offset: u64,
    eof: bool,
}

#[cfg(test)]
impl<R: Read> ChunkStream<R> {
    pub(crate) fn new(reader: R, profile: &ChunkSizingProfile, inventory_size: u64) -> Self {
        Self::with_boundary(reader, BoundaryFinder::new(profile), inventory_size)
    }
}

impl<R: Read, C: ChunkBoundary> ChunkStream<R, C> {
    pub(crate) fn with_boundary(reader: R, boundary: C, inventory_size: u64) -> Self {
        let max_size = boundary.max_size();
        // Upstream retains a max-sized buffer plus an owned chunk of up to max.
        // Reuse that worst-case bound for read-ahead instead of copying every chunk.
        // Small sources need no extra read-ahead allocation; never shrink below max,
        // because inventory size is a hint and cannot decide a chunk boundary.
        let extra = inventory_size
            .saturating_sub(max_size as u64)
            .min(max_size as u64) as usize;
        let capacity = max_size + extra;
        Self {
            reader,
            boundary,
            buffer: vec![0; capacity],
            start: 0,
            end: 0,
            offset: 0,
            eof: false,
        }
    }

    pub(crate) fn next(
        &mut self,
        control: &OperationControl,
    ) -> crate::Result<Option<BorrowedChunk<'_>>> {
        control.checkpoint()?;
        if !self.eof && self.end - self.start < self.boundary.max_size() {
            self.buffer.copy_within(self.start..self.end, 0);
            self.end -= self.start;
            self.start = 0;
            while self.end < self.buffer.len() && !self.eof {
                control.checkpoint()?;
                match self.reader.read(&mut self.buffer[self.end..]) {
                    Ok(0) => self.eof = true,
                    Ok(count) => self.end += count,
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error.into()),
                }
            }
        }
        if self.start == self.end {
            return Ok(None);
        }
        let length = self.boundary.cut(&self.buffer[self.start..self.end]);
        let start = self.start;
        let offset = self.offset;
        self.start += length;
        self.offset += length as u64;
        Ok(Some(BorrowedChunk {
            offset,
            data: &self.buffer[start..self.start],
        }))
    }
}

#[cfg(test)]
#[path = "chunk_stream_tests.rs"]
mod tests;
