use std::io::Read;
use std::time::{Duration, Instant};

use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChunkHash([u8; 32]);

impl ChunkHash {
    pub fn from_blake3(hash: blake3::Hash) -> Self {
        Self(*hash.as_bytes())
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn parse_hex(hex: &str) -> Result<Self> {
        let bytes = hex_decode(hex)?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
            crate::error::Error::verify(format!(
                "chunk hash hex decoded to {} bytes, expected 32",
                hex.len() / 2
            ))
        })?;
        Ok(Self(bytes))
    }

    pub fn to_hex(self) -> String {
        blake3::Hash::from_bytes(self.0).to_hex().to_string()
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Display for ChunkHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

pub fn hash_chunk(data: &[u8]) -> blake3::Hash {
    blake3::hash(data)
}

pub fn hash_file(path: &std::path::Path) -> Result<blake3::Hash> {
    let data = crate::helpers::read(path)?;
    Ok(blake3::hash(&data))
}

pub fn hash_file_streaming_with_control(
    path: &std::path::Path,
    control: &crate::GutchOperationControl,
) -> Result<(blake3::Hash, u64, Duration)> {
    control.checkpoint()?;
    let started_at = Instant::now();
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut hasher = blake3::Hasher::new();
    let mut bytes_read = 0u64;

    loop {
        control.checkpoint()?;
        let read_count = reader.read(&mut buffer)?;
        if read_count == 0 {
            break;
        }
        hasher.update(&buffer[..read_count]);
        bytes_read += read_count as u64;
    }

    Ok((hasher.finalize(), bytes_read, started_at.elapsed()))
}

pub fn hash_hex(hash: &blake3::Hash) -> String {
    hash.to_hex().to_string()
}

pub fn hex_decode(hex: &str) -> Result<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return Err(crate::error::Error::verify("hex string has odd length"));
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|e| crate::error::Error::verify(format!("invalid hex: {}", e)))
        })
        .collect()
}

#[cfg(test)]
#[path = "hash_tests.rs"]
mod tests;
