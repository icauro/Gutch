use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 1;
pub const CHUNKER_FASTCDC: &str = "fastcdc-v2020";
pub const CHUNK_HASH_BLAKE3: &str = "blake3";
pub const COMPRESSION_ZSTD: &str = "zstd";
pub const BUNDLE_FORMAT: &str = "gutch-bundle-v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkingProfile {
    pub chunker: String,
    pub min_chunk_size: u32,
    pub avg_chunk_size: u32,
    pub max_chunk_size: u32,
    pub gear_table_id: u32,
    pub chunk_hash_algorithm: String,
    pub format_version: u32,
}

impl Default for ChunkingProfile {
    fn default() -> Self {
        ChunkingProfile {
            chunker: CHUNKER_FASTCDC.to_string(),
            min_chunk_size: 16 * 1024,
            avg_chunk_size: 64 * 1024,
            max_chunk_size: 256 * 1024,
            gear_table_id: 3,
            chunk_hash_algorithm: CHUNK_HASH_BLAKE3.to_string(),
            format_version: FORMAT_VERSION,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionProfile {
    pub compression: String,
    pub compression_unit: String,
    pub zstd_level: i32,
}

impl Default for CompressionProfile {
    fn default() -> Self {
        CompressionProfile {
            compression: COMPRESSION_ZSTD.to_string(),
            compression_unit: "chunk".to_string(),
            zstd_level: 6,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetManifest {
    pub manifest_version: u32,
    pub product_id: String,
    pub version_id: String,
    pub platform: String,
    pub architecture: String,
    pub tier_id: String,
    pub created_at: String,
    pub chunking_profile: ChunkingProfile,
    pub compression_profile: CompressionProfile,
    pub bundle_format_version: String,
    pub files: Vec<FileManifest>,
    pub required_chunk_set: Vec<String>,
    pub required_bundle_set: Vec<String>,
    pub total_logical_size: u64,
    pub total_compressed_size_estimate: u64,
    pub signature_metadata: Option<SignatureMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureMetadata {
    pub algorithm: String,
    pub key_id: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileManifest {
    pub path: String,
    pub size: u64,
    pub mode: u32,
    pub executable: bool,
    pub symlink_target: Option<String>,
    pub full_file_hash: String,
    pub chunks: Vec<ChunkRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkRef {
    pub chunk_hash: String,
    pub raw_size: u64,
    pub file_offset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierOverlayManifest {
    pub format_version: u32,
    pub product_id: String,
    pub version_id: String,
    pub base_tier_id: String,
    pub target_tier_id: String,
    pub add_files: Vec<FileManifest>,
    pub replace_files: Vec<FileManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleTableEntry {
    pub chunk_hash: String,
    pub raw_size: u64,
    pub compressed_size: u64,
    pub compressed_offset: u64,
    pub compression: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Estimate {
    pub missing_logical_bytes: u64,
    pub missing_compressed_bytes: u64,
    pub missing_chunk_count: u64,
    pub required_bundle_count: u64,
    pub range_request_count: u64,
    pub local_cache_hits: u64,
    pub available_disk_space: u64,
    pub fragmentation_score: f64,
}

impl TargetManifest {
    pub fn new(
        product_id: String,
        version_id: String,
        platform: String,
        architecture: String,
        tier_id: String,
    ) -> Self {
        TargetManifest {
            manifest_version: FORMAT_VERSION,
            product_id,
            version_id,
            platform,
            architecture,
            tier_id,
            created_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            chunking_profile: ChunkingProfile::default(),
            compression_profile: CompressionProfile::default(),
            bundle_format_version: BUNDLE_FORMAT.to_string(),
            files: Vec::new(),
            required_chunk_set: Vec::new(),
            required_bundle_set: Vec::new(),
            total_logical_size: 0,
            total_compressed_size_estimate: 0,
            signature_metadata: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunking_profile_defaults() {
        let p = ChunkingProfile::default();
        assert_eq!(p.min_chunk_size, 16 * 1024);
        assert_eq!(p.avg_chunk_size, 64 * 1024);
        assert_eq!(p.max_chunk_size, 256 * 1024);
        assert_eq!(p.chunk_hash_algorithm, "blake3");
    }

    #[test]
    fn test_compression_profile_defaults() {
        let p = CompressionProfile::default();
        assert_eq!(p.zstd_level, 6);
        assert_eq!(p.compression, "zstd");
    }

    #[test]
    fn test_target_manifest_new() {
        let m = TargetManifest::new(
            "prod-1".into(), "1.0.0".into(), "windows".into(), "x86_64".into(), "base".into(),
        );
        assert_eq!(m.product_id, "prod-1");
        assert_eq!(m.manifest_version, 1);
        assert!(m.signature_metadata.is_none());
        assert!(m.created_at.len() > 0);
    }

    #[test]
    fn test_manifest_serialization_roundtrip() {
        let m = TargetManifest::new(
            "prod-1".into(), "1.0.0".into(), "windows".into(), "x86_64".into(), "base".into(),
        );
        let json = serde_json::to_string_pretty(&m).unwrap();
        let m2: TargetManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(m.product_id, m2.product_id);
        assert_eq!(m.version_id, m2.version_id);
    }

    #[test]
    fn test_file_manifest_serialization() {
        let fm = FileManifest {
            path: "game.exe".to_string(),
            size: 12345,
            mode: 0o644,
            executable: true,
            symlink_target: None,
            full_file_hash: "abc123".to_string(),
            chunks: vec![
                ChunkRef { chunk_hash: "def456".to_string(), raw_size: 8192, file_offset: 0 },
                ChunkRef { chunk_hash: "ghi789".to_string(), raw_size: 4153, file_offset: 8192 },
            ],
        };
        let json = serde_json::to_string(&fm).unwrap();
        let fm2: FileManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(fm.path, fm2.path);
        assert_eq!(fm.chunks.len(), 2);
    }
}
