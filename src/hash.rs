use crate::error::Result;

pub fn hash_chunk(data: &[u8]) -> blake3::Hash {
    blake3::hash(data)
}

pub fn hash_file(path: &std::path::Path) -> Result<blake3::Hash> {
    let data = std::fs::read(path)?;
    Ok(blake3::hash(&data))
}

pub fn hash_hex(hash: &blake3::Hash) -> String {
    hash.to_hex().to_string()
}

pub fn hex_decode(hex: &str) -> Result<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return Err(crate::error::Error::verify("hex string has odd length"));
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| {
                crate::error::Error::verify(format!("invalid hex: {}", e))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_chunk() {
        let hash = hash_chunk(b"hello");
        let hex = hash_hex(&hash);
        assert_eq!(hex.len(), 64);
    }

    #[test]
    fn test_hex_decode_roundtrip() {
        let data = b"test data for hex encoding";
        let hash = hash_chunk(data);
        let hex = hash_hex(&hash);
        let bytes = hex_decode(&hex).unwrap();
        assert_eq!(bytes.as_slice(), hash.as_bytes());
    }

    #[test]
    fn test_hex_decode_invalid_length() {
        let result = hex_decode("abcde");
        assert!(result.is_err());
    }

    #[test]
    fn test_hash_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("gutch_test_hash_file.tmp");
        std::fs::write(&path, b"test content").unwrap();
        let hash = hash_file(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(hash, blake3::hash(b"test content"));
    }
}