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
        if hex.len() != 64 {
            return Err(crate::Error::verify("hash must contain 64 hex bytes"));
        }
        let mut bytes = [0u8; 32];
        for (output, pair) in bytes.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
            let digit = |byte: u8| match byte {
                b'0'..=b'9' => Ok(byte - b'0'),
                b'a'..=b'f' => Ok(byte - b'a' + 10),
                _ => Err(crate::Error::verify(
                    "hash must use lowercase ASCII hexadecimal",
                )),
            };
            *output = digit(pair[0])? * 16 + digit(pair[1])?;
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

impl std::fmt::Display for ChunkHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
