use super::*;
use crate::manifest::target::TargetManifest;

    #[test]
    fn test_chunking_profile_defaults() {
        let p = ChunkSizingProfile::default();
        assert_eq!(p.preset, ArtifactSizingPreset::Balanced);
        assert_eq!(p.min_chunk_bytes, 4 * 1024);
        assert_eq!(p.avg_chunk_bytes, 16 * 1024);
        assert_eq!(p.max_chunk_bytes, 64 * 1024);
        assert_eq!(p.min_chunk_size_bytes(), 4 * 1024);
        assert_eq!(p.avg_chunk_size_bytes(), 16 * 1024);
        assert_eq!(p.max_chunk_size_bytes(), 64 * 1024);
        assert_eq!(p.chunk_hash_algorithm, "blake3");
    }

    #[test]
    fn test_small_chunk_profile_uses_sub_kib_range() {
        let p = ChunkSizingProfile::for_preset(ArtifactSizingPreset::Small);
        assert_eq!(p.preset, ArtifactSizingPreset::Small);
        assert_eq!(p.min_chunk_size_bytes(), 256);
        assert_eq!(p.avg_chunk_size_bytes(), 1024);
        assert_eq!(p.max_chunk_size_bytes(), 4 * 1024);
    }

    #[test]
    fn test_chunk_auto_selection_always_resolves_balanced() {
        assert_eq!(
            ChunkSizingProfile::auto_preset_for_median_file_size_bytes(1),
            ArtifactSizingPreset::Balanced
        );
        assert_eq!(
            ChunkSizingProfile::auto_preset_for_median_file_size_bytes(1024 * 1024 * 1024),
            ArtifactSizingPreset::Balanced
        );
    }

    #[test]
    fn test_bundle_sizing_profile_defaults() {
        let p = BundleSizingProfile::default();
        assert_eq!(p.preset, BundleSizingPreset::Balanced);
        assert_eq!(p.bundle_max_mib, 32);
        assert_eq!(p.bundle_preferred_mib, 16);
        assert_eq!(p.bundle_max_size_bytes(), 32 * 1024 * 1024);
        assert_eq!(p.bundle_preferred_size_bytes(), 16 * 1024 * 1024);
    }

    #[test]
    fn test_bundle_auto_selection_always_resolves_balanced() {
        assert_eq!(
            BundleSizingProfile::auto_preset_for_target_logical_size_bytes(1),
            BundleSizingPreset::Balanced
        );
        assert_eq!(
            BundleSizingProfile::auto_preset_for_target_logical_size_bytes(
                1024 * 1024 * 1024 * 1024
            ),
            BundleSizingPreset::Balanced
        );
    }

    #[test]
    fn test_compression_profile_defaults() {
        let p = CompressionProfile::default();
        assert_eq!(p.zstd_level, 3);
        assert_eq!(p.compression, "zstd");
        assert_eq!(p.compression_unit, "chunk");
    }

    #[test]
    fn test_file_manifest_serialization() {
        let fm = FileManifest {
            path: "game.exe".to_string(),
            size: 12345,
            executable: true,
            full_file_hash: "abc123".to_string(),
            chunks: vec![
                ChunkRef {
                    chunk_hash: "def456".to_string(),
                    raw_size: 8192,
                },
                ChunkRef {
                    chunk_hash: "ghi789".to_string(),
                    raw_size: 4153,
                },
            ],
        };
        let json = serde_json::to_string(&fm).unwrap();
        let fm2: FileManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(fm.path, fm2.path);
        assert_eq!(fm.chunks.len(), 2);
    }
