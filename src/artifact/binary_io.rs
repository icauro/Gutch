use crate::artifact::hash::ChunkHash;
use crate::{Error, Result};
use std::io::{Read, Write};
pub(crate) struct BinaryReader<R> {
    pub(crate) inner: R,
    pub(crate) remaining: u64,
}

impl<R: Read> BinaryReader<R> {
    pub(crate) fn require_records(&self, count: u64, minimum_bytes: u64) -> Result<()> {
        if count > self.remaining / minimum_bytes {
            return Err(Error::manifest(
                "binary manifest declares more data than remains",
            ));
        }
        Ok(())
    }
}

impl<R: Read> Read for BinaryReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let limit = buffer
            .len()
            .min(self.remaining.min(usize::MAX as u64) as usize);
        let count = self.inner.read(&mut buffer[..limit])?;
        self.remaining -= count as u64;
        Ok(count)
    }
}

pub(crate) fn read_exact_array<const N: usize>(reader: &mut impl Read) -> Result<[u8; N]> {
    let mut buffer = [0u8; N];
    reader.read_exact(&mut buffer)?;
    Ok(buffer)
}

pub(crate) fn read_u16(reader: &mut impl Read) -> Result<u16> {
    Ok(u16::from_le_bytes(read_exact_array(reader)?))
}

pub(crate) fn read_u32(reader: &mut impl Read) -> Result<u32> {
    Ok(u32::from_le_bytes(read_exact_array(reader)?))
}

pub(crate) fn read_u8(reader: &mut impl Read) -> Result<u8> {
    Ok(read_exact_array::<1>(reader)?[0])
}

pub(crate) fn read_u64(reader: &mut impl Read) -> Result<u64> {
    Ok(u64::from_le_bytes(read_exact_array(reader)?))
}

pub(crate) fn read_chunk_hash(reader: &mut impl Read) -> Result<ChunkHash> {
    Ok(ChunkHash::from_bytes(read_exact_array(reader)?))
}

pub(crate) fn write_u16(writer: &mut impl Write, value: u16) -> Result<()> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

pub(crate) fn write_u32(writer: &mut impl Write, value: u32) -> Result<()> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

pub(crate) fn write_u64(writer: &mut impl Write, value: u64) -> Result<()> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

pub(crate) fn write_chunk_hash(writer: &mut impl Write, value: ChunkHash) -> Result<()> {
    writer.write_all(value.as_bytes())?;
    Ok(())
}

pub(crate) fn write_len_prefixed_string(writer: &mut impl Write, value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    if bytes.len() > crate::filesystem::paths::MAX_MANIFEST_PATH_BYTES {
        return Err(Error::manifest("binary string exceeds 128 KiB limit"));
    }
    let length = u32::try_from(bytes.len())
        .map_err(|_| Error::manifest("string too large for binary manifest"))?;
    write_u32(writer, length)?;
    writer.write_all(bytes)?;
    Ok(())
}

pub(crate) fn read_len_prefixed_string(reader: &mut BinaryReader<impl Read>) -> Result<String> {
    let length = read_u32(reader)? as usize;
    if length > crate::filesystem::paths::MAX_MANIFEST_PATH_BYTES {
        return Err(Error::manifest("binary string exceeds 128 KiB limit"));
    }
    reader.require_records(length as u64, 1)?;
    let mut bytes = vec![0u8; length];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes).map_err(|err| Error::manifest(format!("invalid UTF-8: {}", err)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_string_is_rejected_before_reading_payload() {
        let prefix = ((crate::filesystem::paths::MAX_MANIFEST_PATH_BYTES + 1) as u32).to_le_bytes();
        let mut reader = BinaryReader {
            inner: prefix.as_slice(),
            remaining: u64::MAX,
        };
        let error = read_len_prefixed_string(&mut reader)
            .unwrap_err()
            .to_string();
        assert!(error.contains("128 KiB"), "{error}");
    }
}
