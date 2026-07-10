use std::io::{Cursor, Write};

use crate::error::Result;
use crate::manifest::CompressionKind;

pub fn decompress_data(data: &[u8], compression: CompressionKind) -> Result<Vec<u8>> {
    match compression {
        CompressionKind::None => Ok(data.to_vec()),
        CompressionKind::Zstd => zstd::decode_all(data)
            .map_err(|e| crate::error::Error::bundle(format!("zstd decompress: {}", e))),
    }
}

pub fn decompress_data_to_writer<W: Write>(
    data: &[u8],
    compression: CompressionKind,
    mut writer: W,
) -> Result<u64> {
    struct CountingWriter<'a, W> {
        inner: &'a mut W,
        written: u64,
    }

    impl<W: Write> Write for CountingWriter<'_, W> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let written = self.inner.write(buf)?;
            self.written += written as u64;
            Ok(written)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }

    match compression {
        CompressionKind::None => {
            writer
                .write_all(data)
                .map_err(|e| crate::error::Error::bundle(format!("write: {}", e)))?;
            Ok(data.len() as u64)
        }
        CompressionKind::Zstd => {
            let mut counting_writer = CountingWriter {
                inner: &mut writer,
                written: 0,
            };
            zstd::stream::copy_decode(Cursor::new(data), &mut counting_writer)
                .map_err(|e| crate::error::Error::bundle(format!("zstd decompress: {}", e)))?;
            Ok(counting_writer.written)
        }
    }
}

pub fn compress_with_level(data: &[u8], level: i32) -> Result<Vec<u8>> {
    compress_segments_with_level(std::iter::once(data), data.len() as u64, level)
}

pub fn compress_segments_with_level<'a, I>(
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
