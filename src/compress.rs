use crate::error::Result;
use crate::manifest::CompressionProfile;

pub fn compress_chunk(data: &[u8], profile: &CompressionProfile) -> Result<Vec<u8>> {
    let level = profile.zstd_level;
    zstd::encode_all(data, level)
        .map_err(|e| crate::error::Error::bundle(format!("zstd compress: {}", e)))
}

pub fn decompress_chunk(data: &[u8]) -> Result<Vec<u8>> {
    zstd::decode_all(data)
        .map_err(|e| crate::error::Error::bundle(format!("zstd decompress: {}", e)))
}

pub fn decompress_chunk_at_level(data: &[u8], _level: i32) -> Result<Vec<u8>> {
    decompress_chunk(data)
}

pub fn compress_with_level(data: &[u8], level: i32) -> Result<Vec<u8>> {
    zstd::encode_all(data, level)
        .map_err(|e| crate::error::Error::bundle(format!("zstd compress: {}", e)))
}

#[cfg(test)]
#[path = "compress_tests.rs"]
mod tests;
