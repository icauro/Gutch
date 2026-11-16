//! Compression adapters (none, zstd, lz4) with bounded working memory.

#[cfg(test)]
use std::io::Cursor;
use std::io::Read;
use std::io::Write;

use crate::artifact::profiles::CompressionKind;
use crate::artifact::profiles::CompressionProfile;
use crate::error::Error;
use crate::error::Result;

pub(crate) const ZSTD_WINDOW_LOG: u32 = 27;
/// zstd never uses a window below 1 KiB.
const ZSTD_MIN_WINDOW_LOG: u32 = 10;
/// LZ4 frames hold at most 4 MiB blocks: one block in, one compressed block out,
/// plus the linked-block dictionary and context.
const LZ4_WORKING_MEMORY_BYTES: u64 = 16 * 1024 * 1024;

// The working-memory functions below are admission estimates for codec work,
// not a bound on total process RSS. Encoder and decoder buffers differ, so each
// side has its own estimate.

/// The window zstd uses for a frame of `raw_size` bytes: the encoder pledges the
/// size, so zstd shrinks the configured window to fit the content.
fn zstd_window_log(raw_size: u64) -> u32 {
    let content_log = u64::BITS - raw_size.saturating_sub(1).leading_zeros();
    content_log.clamp(ZSTD_MIN_WINDOW_LOG, ZSTD_WINDOW_LOG)
}

/// Memory one encoder needs for `raw_size` bytes under `profile`.
pub(crate) fn encode_working_memory_bytes(profile: &CompressionProfile, raw_size: u64) -> u64 {
    match profile.compression {
        CompressionKind::None => 0,
        CompressionKind::Lz4 => LZ4_WORKING_MEMORY_BYTES,
        CompressionKind::Zstd => {
            // An invalid level fails at encode time; estimate for the largest one meanwhile.
            let level =
                validate_profile(profile).unwrap_or_else(|_| zstd::zstd_safe::max_c_level());
            use zstd::zstd_safe::zstd_sys;
            // SAFETY: these zstd functions only compute values from their arguments.
            let estimate = unsafe {
                // Mirror the encoder: level parameters for the pledged size, the
                // configured window, then zstd's own shrinking to that size.
                let mut parameters = zstd_sys::ZSTD_getCParams(level, raw_size, 0);
                parameters.windowLog = ZSTD_WINDOW_LOG;
                let parameters = zstd_sys::ZSTD_adjustCParams(parameters, raw_size, 0);
                zstd_sys::ZSTD_estimateCStreamSize_usingCParams(parameters)
            };
            estimate as u64 + zstd::zstd_safe::CCtx::out_size() as u64
        }
    }
}

/// Memory one decoder needs for an artifact holding `raw_size` bytes. The codec
/// is known only once the artifact is open, so this covers any of them.
pub(crate) fn decode_working_memory_bytes(raw_size: u64) -> u64 {
    // SAFETY: this zstd function only computes a value from its argument.
    let zstd = unsafe {
        zstd::zstd_safe::zstd_sys::ZSTD_estimateDStreamSize(1 << zstd_window_log(raw_size))
    };
    (zstd as u64 + zstd::zstd_safe::DCtx::in_size() as u64).max(LZ4_WORKING_MEMORY_BYTES)
}

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

/// Maps a failure to decode stored bytes to `Error::Artifact`: the bytes are
/// damaged or malformed, which callers must not mistake for a disk fault.
pub(crate) fn corrupt_payload(context: &'static str) -> impl Fn(std::io::Error) -> Error {
    move |error| Error::Artifact(format!("{context}: {error}"))
}

pub(crate) fn decoder_from_reader<'a>(
    reader: impl Read + 'a,
    compression: CompressionKind,
) -> Result<Box<dyn Read + 'a>> {
    let corrupt = corrupt_payload("opening compressed payload");
    Ok(match compression {
        CompressionKind::None => Box::new(reader),
        CompressionKind::Zstd => {
            let mut decoder = zstd::stream::read::Decoder::new(reader).map_err(&corrupt)?;
            decoder.window_log_max(ZSTD_WINDOW_LOG)?;
            Box::new(decoder)
        }
        CompressionKind::Lz4 => Box::new(lz4::Decoder::new(reader).map_err(&corrupt)?),
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
