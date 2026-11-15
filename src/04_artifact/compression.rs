//! Compression adapters (none, zstd, lz4) with bounded working memory.

#[cfg(test)]
use std::io::Cursor;
use std::io::Read;
use std::io::Write;

use crate::artifact::profiles::CompressionKind;
use crate::artifact::profiles::CompressionProfile;
use crate::error::Error;
use crate::error::Result;

// Admission estimates for codec work, not a bound on total process RSS.
// Keep encoder and decoder reservations separate: their live buffers differ.
const ZSTD_WINDOW_LOG: u32 = 27;
pub(crate) const ENCODE_WORKING_MEMORY_BYTES: u64 = 1 << ZSTD_WINDOW_LOG;
pub(crate) const DECODE_WORKING_MEMORY_BYTES: u64 = (1 << ZSTD_WINDOW_LOG) + 8 * 1024 * 1024;

trait CompressionAdapter {
    fn default_level(&self) -> i32;
    fn validate_level(&self, level: Option<i32>) -> Result<i32>;
    #[cfg(test)]
    fn decoder<'a>(&self, data: &'a [u8]) -> Result<Box<dyn Read + 'a>>;
}

struct NoneAdapter;

impl CompressionAdapter for NoneAdapter {
    fn default_level(&self) -> i32 {
        0
    }

    fn validate_level(&self, level: Option<i32>) -> Result<i32> {
        match level {
            None => Ok(0),
            Some(level) => Err(Error::artifact(format!(
                "compression level {level} is invalid for uncompressed content"
            ))),
        }
    }

    #[cfg(test)]
    fn decoder<'a>(&self, data: &'a [u8]) -> Result<Box<dyn Read + 'a>> {
        Ok(Box::new(Cursor::new(data)))
    }
}

struct ZstdAdapter;

impl CompressionAdapter for ZstdAdapter {
    fn default_level(&self) -> i32 {
        3
    }

    fn validate_level(&self, level: Option<i32>) -> Result<i32> {
        let level = level.unwrap_or_else(|| self.default_level());
        if (1..=22).contains(&level) {
            Ok(level)
        } else {
            Err(Error::artifact(format!(
                "zstd compression level {level} is outside the supported range 1..=22"
            )))
        }
    }

    #[cfg(test)]
    fn decoder<'a>(&self, data: &'a [u8]) -> Result<Box<dyn Read + 'a>> {
        let decoder = zstd::stream::read::Decoder::new(Cursor::new(data))
            .map_err(|error| Error::artifact(format!("zstd decompress: {error}")))?;
        Ok(Box::new(decoder))
    }
}

struct Lz4Adapter;

impl CompressionAdapter for Lz4Adapter {
    fn default_level(&self) -> i32 {
        0
    }

    fn validate_level(&self, level: Option<i32>) -> Result<i32> {
        let level = level.unwrap_or_else(|| self.default_level());
        if (0..=9).contains(&level) {
            Ok(level)
        } else {
            Err(Error::artifact(format!(
                "lz4 compression level {level} is outside the supported range 0..=9"
            )))
        }
    }

    #[cfg(test)]
    fn decoder<'a>(&self, data: &'a [u8]) -> Result<Box<dyn Read + 'a>> {
        let decoder = lz4::Decoder::new(Cursor::new(data))
            .map_err(|error| Error::artifact(format!("lz4 decompress: {error}")))?;
        Ok(Box::new(decoder))
    }
}

fn adapter_for(compression: CompressionKind) -> Box<dyn CompressionAdapter> {
    match compression {
        CompressionKind::None => Box::new(NoneAdapter),
        CompressionKind::Zstd => Box::new(ZstdAdapter),
        CompressionKind::Lz4 => Box::new(Lz4Adapter),
    }
}

pub(crate) fn validate_profile(profile: &CompressionProfile) -> Result<i32> {
    adapter_for(profile.compression).validate_level(profile.level)
}

#[cfg(test)]
pub(crate) fn decompress_data(data: &[u8], compression: CompressionKind) -> Result<Vec<u8>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let mut decoder = adapter_for(compression).decoder(data)?;
    let mut output = Vec::new();
    decoder
        .read_to_end(&mut output)
        .map_err(|error| Error::artifact(format!("decompress: {error}")))?;
    Ok(output)
}

pub(crate) fn decoder_from_reader<'a>(
    reader: impl Read + 'a,
    compression: CompressionKind,
) -> Result<Box<dyn Read + 'a>> {
    Ok(match compression {
        CompressionKind::None => Box::new(reader),
        CompressionKind::Zstd => {
            let mut decoder = zstd::stream::read::Decoder::new(reader)?;
            decoder.window_log_max(ZSTD_WINDOW_LOG)?;
            Box::new(decoder)
        }
        CompressionKind::Lz4 => Box::new(lz4::Decoder::new(reader)?),
    })
}

pub(crate) fn encode_to_writer(
    writer: &mut impl Write,
    profile: &CompressionProfile,
    raw_size: u64,
    write_raw: impl FnOnce(&mut dyn Write) -> Result<()>,
) -> Result<()> {
    let level = validate_profile(profile)?;
    match profile.compression {
        CompressionKind::None => write_raw(writer),
        CompressionKind::Zstd => {
            let mut encoder = zstd::stream::Encoder::new(writer, level)?;
            encoder.window_log(ZSTD_WINDOW_LOG)?;
            encoder.set_pledged_src_size(Some(raw_size))?;
            write_raw(&mut encoder)?;
            encoder.finish()?;
            Ok(())
        }
        CompressionKind::Lz4 => {
            let mut encoder = lz4::EncoderBuilder::new()
                .level(level as u32)
                .build(writer)?;
            write_raw(&mut encoder)?;
            encoder.finish().1?;
            Ok(())
        }
    }
}

/// Count the exact codec output without retaining compressed bytes.
pub(crate) fn compressed_size_from_stream(
    profile: &CompressionProfile,
    raw_size: u64,
    write_raw: impl FnOnce(&mut dyn Write) -> Result<()>,
) -> Result<u64> {
    struct ByteCounter(u64);
    impl Write for ByteCounter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len() as u64;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = ByteCounter(0);
    encode_to_writer(&mut output, profile, raw_size, write_raw)?;
    Ok(output.0)
}

#[cfg(test)]
pub(crate) fn compressed_size(segments: &[&[u8]], profile: &CompressionProfile) -> Result<u64> {
    let raw_size = segments.iter().map(|segment| segment.len() as u64).sum();
    compressed_size_from_stream(profile, raw_size, |writer| {
        for segment in segments {
            writer.write_all(segment)?;
        }
        Ok(())
    })
}

#[cfg(test)]
fn compress_with_profile(data: &[u8], profile: &CompressionProfile) -> Result<Vec<u8>> {
    compress_segments_with_profile(&[data], data.len() as u64, profile)
}

#[cfg(test)]
fn compress_segments_with_profile(
    segments: &[&[u8]],
    raw_size: u64,
    profile: &CompressionProfile,
) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    encode_to_writer(&mut output, profile, raw_size, |writer| {
        for segment in segments {
            writer.write_all(segment)?;
        }
        Ok(())
    })?;
    Ok(output)
}

#[cfg(test)]
#[path = "compression_tests.rs"]
mod tests;
