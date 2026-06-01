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
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|e| crate::error::Error::verify(format!("invalid hex: {}", e)))
        })
        .collect()
}

#[cfg(test)]
#[path = "hash_tests.rs"]
mod tests;
