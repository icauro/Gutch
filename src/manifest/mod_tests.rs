use super::*;

#[test]
fn test_chunking_profile_defaults() {
    let p = ChunkSizingProfile::default();
    assert_eq!(p.preset, ArtifactSizingPreset::Chunk1KiB);
    assert_eq!(p.min_chunk_bytes, 256);
    assert_eq!(p.avg_chunk_bytes, 1024);
    assert_eq!(p.max_chunk_bytes, 4 * 1024);
    assert_eq!(p.min_chunk_size_bytes(), 256);
    assert_eq!(p.avg_chunk_size_bytes(), 1024);
    assert_eq!(p.max_chunk_size_bytes(), 4 * 1024);
    assert_eq!(p.chunk_hash_algorithm, "blake3");
}

#[test]
fn test_small_chunk_profile_uses_sub_kib_range() {
    let p = ChunkSizingProfile::for_preset(ArtifactSizingPreset::Chunk1KiB);
    assert_eq!(p.preset, ArtifactSizingPreset::Chunk1KiB);
    assert_eq!(p.min_chunk_size_bytes(), 256);
    assert_eq!(p.avg_chunk_size_bytes(), 1024);
    assert_eq!(p.max_chunk_size_bytes(), 4 * 1024);
}

#[test]
fn test_chunk_auto_selection_always_resolves_small() {
    assert_eq!(
        ChunkSizingProfile::auto_preset_for_median_file_size_bytes(1),
        ArtifactSizingPreset::Chunk1KiB
    );
    assert_eq!(
        ChunkSizingProfile::auto_preset_for_median_file_size_bytes(1024 * 1024 * 1024),
        ArtifactSizingPreset::Chunk1KiB
    );
}

#[test]
fn test_bundle_sizing_profile_defaults() {
    let p = BundleSizingProfile::default();
    assert_eq!(p.preset, BundleSizingPreset::Bundle16MiB);
    assert_eq!(p.bundle_max_mib, 32);
    assert_eq!(p.bundle_preferred_mib, 16);
    assert_eq!(p.bundle_max_size_bytes(), 32 * 1024 * 1024);
    assert_eq!(p.bundle_preferred_size_bytes(), 16 * 1024 * 1024);
}

#[test]
fn test_bundle_auto_selection_always_resolves_balanced() {
    assert_eq!(
        BundleSizingProfile::auto_preset_for_target_logical_size_bytes(1),
        BundleSizingPreset::Bundle16MiB
    );
    assert_eq!(
        BundleSizingProfile::auto_preset_for_target_logical_size_bytes(1024 * 1024 * 1024 * 1024),
        BundleSizingPreset::Bundle16MiB
    );
}

#[test]
fn test_compression_profile_defaults() {
    let p = CompressionProfile::default();
    assert_eq!(p.zstd_level, 1);
    assert_eq!(p.compression, "zstd");
    assert_eq!(p.compression_unit, "bundle");
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

#[test]
fn test_structured_version_parses_semver() {
    let version = StructuredVersion::parse("v1.2.3").unwrap();
    assert_eq!(version.scheme, VersionScheme::Semver);
    assert_eq!(version.major_or_year, 1);
    assert_eq!(version.minor_or_month, 2);
    assert_eq!(version.patch_or_day, 3);
    assert_eq!(version.micro, 0);
    assert_eq!(version.canonical_string(), "1.2.3");
}

#[test]
fn test_structured_version_parses_calver() {
    let version = StructuredVersion::parse("2025.05.30.1").unwrap();
    assert_eq!(version.scheme, VersionScheme::Calver);
    assert_eq!(version.major_or_year, 2025);
    assert_eq!(version.minor_or_month, 5);
    assert_eq!(version.patch_or_day, 30);
    assert_eq!(version.micro, 1);
    assert_eq!(version.canonical_string(), "2025.05.30.1");
}

#[test]
fn test_structured_version_rejects_invalid_four_part_semver() {
    assert!(StructuredVersion::parse("1.2.3.4").is_err());
}
