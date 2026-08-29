use std::io::Write;

use crate::error::Result;
use crate::manifest::CompressionKind;

pub(crate) fn decompress_data(data: &[u8], compression: CompressionKind) -> Result<Vec<u8>> {
    match compression {
        CompressionKind::None => Ok(data.to_vec()),
        CompressionKind::Zstd => zstd::decode_all(data)
            .map_err(|e| crate::error::Error::bundle(format!("zstd decompress: {}", e))),
    }
}

pub(crate) fn compress_with_level(data: &[u8], level: i32) -> Result<Vec<u8>> {
    compress_segments_with_level(std::iter::once(data), data.len() as u64, level)
}

pub(crate) fn compress_segments_with_level<'a, I>(
    segments: I,
    raw_size: u64,
    level: i32,
) -> Result<Vec<u8>>
where
    I: IntoIterator<Item = &'a [u8]>,
{
    let output = Vec::with_capacity(estimated_zstd_output_capacity(raw_size));
    let mut encoder = zstd::stream::Encoder::new(output, level)
        .map_err(|e| crate::error::Error::bundle(format!("zstd compress: {}", e)))?;

    for segment in segments {
        encoder
            .write_all(segment)
            .map_err(|e| crate::error::Error::bundle(format!("zstd compress: {}", e)))?;
    }

    encoder
        .finish()
        .map_err(|e| crate::error::Error::bundle(format!("zstd compress: {}", e)))
}

fn estimated_zstd_output_capacity(raw_size: u64) -> usize {
    if raw_size == 0 {
        return 0;
    }

    let bounded_size = raw_size.min(usize::MAX as u64) as usize;
    (bounded_size / 2)
        .saturating_add(1024 * 1024)
        .min(bounded_size)
}

#[cfg(test)]
#[path = "compress_tests.rs"]
mod tests;
