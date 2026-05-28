use std::io::{Cursor, Write};

use crate::error::Result;
use crate::manifest::CompressionProfile;

pub fn compress_chunk(data: &[u8], profile: &CompressionProfile) -> Result<Vec<u8>> {
    let level = profile.zstd_level;
    zstd::encode_all(data, level)
        .map_err(|e| crate::error::Error::bundle(format!("zstd compress: {}", e)))
}

pub fn decompress_chunk(data: &[u8]) -> Result<Vec<u8>> {
    zstd::decode_all(data)
        .map_err(|e| crate::error::Error::bundle(format!("zstd decompress: {}", e)))
}

pub fn decompress_chunk_to_writer<W: Write>(data: &[u8], mut writer: W) -> Result<u64> {
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

    let mut counting_writer = CountingWriter {
        inner: &mut writer,
        written: 0,
    };
    zstd::stream::copy_decode(Cursor::new(data), &mut counting_writer)
        .map_err(|e| crate::error::Error::bundle(format!("zstd decompress: {}", e)))?;
    Ok(counting_writer.written)
}

pub fn decompress_chunk_at_level(data: &[u8], _level: i32) -> Result<Vec<u8>> {
    decompress_chunk(data)
}

pub fn compress_with_level(data: &[u8], level: i32) -> Result<Vec<u8>> {
    zstd::encode_all(data, level)
        .map_err(|e| crate::error::Error::bundle(format!("zstd compress: {}", e)))
}

#[cfg(test)]
#[path = "compress_tests.rs"]
mod tests;
