#[cfg(test)]
use std::io::Cursor;
use std::io::{Read, Write};

use crate::error::Result;
use crate::manifest::{CompressionKind, CompressionProfile};

trait CompressionAdapter {
    fn default_level(&self) -> i32;
    fn validate_level(&self, level: Option<i32>) -> Result<i32>;
    fn compress_segments(&self, segments: &[&[u8]], raw_size: u64, level: i32) -> Result<Vec<u8>>;
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
            Some(level) => Err(crate::error::Error::bundle(format!(
                "compression level {level} is invalid for uncompressed content"
            ))),
        }
    }

    fn compress_segments(&self, segments: &[&[u8]], raw_size: u64, _level: i32) -> Result<Vec<u8>> {
        let capacity = usize::try_from(raw_size).map_err(|_| {
            crate::error::Error::bundle("uncompressed payload is too large for this platform")
        })?;
        let mut output = Vec::with_capacity(capacity);
        for segment in segments {
            output.extend_from_slice(segment);
        }
        Ok(output)
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
            Err(crate::error::Error::bundle(format!(
                "zstd compression level {level} is outside the supported range 1..=22"
            )))
        }
    }

    fn compress_segments(&self, segments: &[&[u8]], raw_size: u64, level: i32) -> Result<Vec<u8>> {
        let output = Vec::with_capacity(estimated_zstd_output_capacity(raw_size));
        let mut encoder = zstd::stream::Encoder::new(output, level)
            .map_err(|error| crate::error::Error::bundle(format!("zstd compress: {error}")))?;

        for segment in segments {
            encoder
                .write_all(segment)
                .map_err(|error| crate::error::Error::bundle(format!("zstd compress: {error}")))?;
        }

        encoder
            .finish()
            .map_err(|error| crate::error::Error::bundle(format!("zstd compress: {error}")))
    }

    #[cfg(test)]
    fn decoder<'a>(&self, data: &'a [u8]) -> Result<Box<dyn Read + 'a>> {
        let decoder = zstd::stream::read::Decoder::new(Cursor::new(data))
            .map_err(|error| crate::error::Error::bundle(format!("zstd decompress: {error}")))?;
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
            Err(crate::error::Error::bundle(format!(
                "lz4 compression level {level} is outside the supported range 0..=9"
            )))
        }
    }

    fn compress_segments(&self, segments: &[&[u8]], raw_size: u64, level: i32) -> Result<Vec<u8>> {
        let output = Vec::with_capacity(estimated_lz4_output_capacity(raw_size));
        let mut encoder = lz4::EncoderBuilder::new()
            .level(level as u32)
            .build(output)
            .map_err(|error| crate::error::Error::bundle(format!("lz4 compress: {error}")))?;

        for segment in segments {
            encoder
                .write_all(segment)
                .map_err(|error| crate::error::Error::bundle(format!("lz4 compress: {error}")))?;
        }

        let (output, result) = encoder.finish();
        result.map_err(|error| crate::error::Error::bundle(format!("lz4 compress: {error}")))?;
        Ok(output)
    }

    #[cfg(test)]
    fn decoder<'a>(&self, data: &'a [u8]) -> Result<Box<dyn Read + 'a>> {
        let decoder = lz4::Decoder::new(Cursor::new(data))
            .map_err(|error| crate::error::Error::bundle(format!("lz4 decompress: {error}")))?;
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
        .map_err(|error| crate::error::Error::bundle(format!("decompress: {error}")))?;
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
            decoder.window_log_max(27)?;
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
            encoder.window_log(27)?;
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

pub(crate) fn compress_with_profile(data: &[u8], profile: &CompressionProfile) -> Result<Vec<u8>> {
    compress_segments_with_profile(std::slice::from_ref(&data), data.len() as u64, profile)
}

pub(crate) fn compress_segments_with_profile(
    segments: &[&[u8]],
    raw_size: u64,
    profile: &CompressionProfile,
) -> Result<Vec<u8>> {
    let adapter = adapter_for(profile.compression);
    let level = adapter.validate_level(profile.level)?;
    adapter.compress_segments(segments, raw_size, level)
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

fn estimated_lz4_output_capacity(raw_size: u64) -> usize {
    raw_size.min(usize::MAX as u64) as usize
}

#[cfg(test)]
#[path = "compress_tests.rs"]
mod tests;
