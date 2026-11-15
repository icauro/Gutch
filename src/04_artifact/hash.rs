//! Chunk content hashes and their hex form.

use crate::error::Result;
use std::fmt::Display;
use std::fmt::Formatter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChunkHash([u8; 32]);

impl ChunkHash {
    pub fn from_blake3(hash: blake3::Hash) -> Self {
        Self(*hash.as_bytes())
    }

    /// Content identity of `data`: its BLAKE3 hash.
    pub fn of(data: &[u8]) -> Self {
        Self::from_blake3(hash_chunk(data))
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn parse_hex(hex: &str) -> Result<Self> {
        if hex.len() != 64 {
            return Err(crate::Error::verify("hash must contain 64 hex bytes"));
        }
        let mut bytes = [0u8; 32];
        for (output, [high, low]) in bytes.iter_mut().zip(hex.as_bytes().as_chunks::<2>().0) {
            let digit = |byte: u8| match byte {
                b'0'..=b'9' => Ok(byte - b'0'),
                b'a'..=b'f' => Ok(byte - b'a' + 10),
                _ => Err(crate::Error::verify(
                    "hash must use lowercase ASCII hexadecimal",
                )),
            };
            *output = digit(*high)? * 16 + digit(*low)?;
        }
        Ok(Self(bytes))
    }

    pub fn to_hex(self) -> String {
        blake3::Hash::from_bytes(self.0).to_hex().to_string()
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl serde::Serialize for ChunkHash {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(blake3::Hash::from_bytes(self.0).to_hex().as_str())
    }
}
impl<'de> serde::Deserialize<'de> for ChunkHash {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::parse_hex(&value).map_err(serde::de::Error::custom)
    }
}

impl Display for ChunkHash {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(blake3::Hash::from_bytes(self.0).to_hex().as_str())
    }
}

pub fn hash_chunk(data: &[u8]) -> blake3::Hash {
    blake3::hash(data)
}

pub fn hash_hex(hash: &blake3::Hash) -> String {
    hash.to_hex().to_string()
}

#[cfg(test)]
pub(crate) fn hex_decode(hex: &str) -> Result<Vec<u8>> {
    use crate::error::Error;

    if !hex.len().is_multiple_of(2) {
        return Err(Error::verify("hex string has odd length"));
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|e| Error::verify(format!("invalid hex: {}", e)))
        })
        .collect()
}

#[cfg(test)]
#[path = "hash_tests.rs"]
mod tests;
