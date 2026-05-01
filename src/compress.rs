use crate::error::Result;
use crate::manifest::CompressionProfile;

pub fn compress_chunk(data: &[u8], profile: &CompressionProfile) -> Result<Vec<u8>> {
    let level = profile.zstd_level;
    zstd::encode_all(data, level).map_err(|e| crate::error::Error::bundle(format!("zstd compress: {}", e)))
}

pub fn decompress_chunk(data: &[u8]) -> Result<Vec<u8>> {
    zstd::decode_all(data).map_err(|e| crate::error::Error::bundle(format!("zstd decompress: {}", e)))
}

pub fn decompress_chunk_at_level(data: &[u8], _level: i32) -> Result<Vec<u8>> {
    decompress_chunk(data)
}

pub fn compress_with_level(data: &[u8], level: i32) -> Result<Vec<u8>> {
    zstd::encode_all(data, level).map_err(|e| crate::error::Error::bundle(format!("zstd compress: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compress_decompress_roundtrip() {
        let original = b"hello world, this is some test data for compression ".repeat(100);
        let profile = CompressionProfile::default();
        let compressed = compress_chunk(&original, &profile).unwrap();
        assert!(compressed.len() < original.len(), "zstd should compress repetitive data");

        let decompressed = decompress_chunk(&compressed).unwrap();
        assert_eq!(original.as_slice(), decompressed.as_slice());
    }

    #[test]
    fn test_compress_small_data() {
        let original = b"hi";
        let profile = CompressionProfile::default();
        let compressed = compress_chunk(original, &profile).unwrap();
        let decompressed = decompress_chunk(&compressed).unwrap();
        assert_eq!(original.as_slice(), decompressed.as_slice());
    }

    #[test]
    fn test_different_levels() {
        let original = b"some data that repeats again and again ".repeat(200);
        let c1 = compress_with_level(&original, 1).unwrap();
        let c9 = compress_with_level(&original, 9).unwrap();
        let d1 = decompress_chunk(&c1).unwrap();
        let d9 = decompress_chunk(&c9).unwrap();
        assert_eq!(d1, original);
        assert_eq!(d9, original);
        assert!(c9.len() <= c1.len(), "higher level should compress as well or better");
    }
}