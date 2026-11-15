//! The only supported artifact generation. All integers are little endian.
use std::io::Read;
use std::io::Write;

use crate::Error;
use crate::Result;
use crate::artifact::profiles::CompressionKind;

/// `VERSION` as recorded in JSON and index records.
pub(crate) const FORMAT_VERSION: &str = "1";
pub(crate) const VERSION: u16 = 1;
pub(crate) const BUNDLE_MAGIC: &[u8; 16] = b"GUTCH_BUNDLE\0\0\0\0";
pub(crate) const BUNDLE_FOOTER_MAGIC: &[u8; 16] = b"GUTCH_BUNDLE_END";
pub(crate) const LOOSE_CHUNK_MAGIC: &[u8; 16] = b"GUTCH_CHUNK\0\0\0\0\0";
pub(crate) const CHUNK_INDEX_MAGIC: &[u8; 16] = b"GUTCH_INDEX\0\0\0\0\0";
pub(crate) const SNAPSHOT_BLUEPRINT_MAGIC: &[u8; 16] = b"GUTCH_BLUEPRINT\0";
pub(crate) const MAX_RAW_BUNDLE_SIZE: u64 = 256 * 1024 * 1024;
pub(crate) const MAX_RAW_CHUNK_SIZE: u64 = 16 * 1024 * 1024;

pub(crate) fn size_u32(size: impl Into<u64>) -> Result<u32> {
    u32::try_from(size.into()).map_err(|_| Error::artifact("artifact size exceeds u32"))
}

pub(crate) fn write_codec(writer: &mut impl Write, codec: CompressionKind) -> Result<()> {
    writer.write_all(&[
        match codec {
            CompressionKind::None => 0,
            CompressionKind::Zstd => 1,
            CompressionKind::Lz4 => 2,
        },
        0,
    ])?;
    Ok(())
}

pub(crate) fn read_codec(reader: &mut impl Read) -> Result<CompressionKind> {
    let mut bytes = [0; 2];
    reader.read_exact(&mut bytes)?;
    if bytes[1] != 0 {
        return Err(Error::artifact("nonzero artifact reserved byte"));
    }
    match bytes[0] {
        0 => Ok(CompressionKind::None),
        1 => Ok(CompressionKind::Zstd),
        2 => Ok(CompressionKind::Lz4),
        _ => Err(Error::artifact("unsupported artifact codec")),
    }
}

pub(crate) fn read_version(reader: &mut impl Read) -> Result<()> {
    let mut bytes = [0; 2];
    reader.read_exact(&mut bytes)?;
    if u16::from_le_bytes(bytes) != VERSION {
        return Err(Error::artifact("unsupported artifact format version"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn narrowing_and_envelope_parameters_reject_unsupported_values() {
        assert_eq!(size_u32(u64::from(u32::MAX)).unwrap(), u32::MAX);
        assert!(size_u32(u64::from(u32::MAX) + 1).is_err());
        assert!(read_version(&mut &2u16.to_le_bytes()[..]).is_err());
        assert!(read_codec(&mut &[3, 0][..]).is_err());
        assert!(read_codec(&mut &[1, 1][..]).is_err());
    }
}
