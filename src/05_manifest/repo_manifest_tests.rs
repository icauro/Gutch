use super::*;

#[test]
fn repo_manifest_new() {
    let repo = RepoManifest::new("base".into());
    assert_eq!(repo.repository_format, RepoFormat::default());
    assert_eq!(repo.repository_name, "base");
    assert_eq!(
        repo.content_packaging_profile.chunk,
        ChunkSizingPreset::Chunk128KiB
    );
    assert_eq!(
        repo.content_packaging_profile.compression,
        CompressionKind::Zstd
    );
    assert_eq!(repo.content_packaging_profile.compression_level, Some(3));
    assert_eq!(
        repo.content_packaging_profile.bundle,
        BundleSizingPreset::Bundle8MiB
    );
    assert_eq!(
        repo.content_packaging_profile.storage,
        ArtifactStorageMode::Bundled
    );
}

#[test]
fn artifact_profile_expands_presets_for_the_engine() {
    let profile = ArtifactProfile::for_presets(
        ChunkSizingPreset::Chunk1MiB,
        CompressionKind::Zstd,
        Some(5),
        BundleSizingPreset::Bundle64MiB,
        ArtifactStorageMode::LooseChunks,
    );

    assert_eq!(
        ChunkSizingProfile::for_preset(profile.chunk).preset,
        ChunkSizingPreset::Chunk1MiB
    );
    assert_eq!(profile.compression_profile().level, Some(5));
    assert_eq!(
        BundleSizingProfile::for_preset(profile.bundle).bundle_preferred_mib,
        64
    );
    assert_eq!(profile.storage, ArtifactStorageMode::LooseChunks);
}

#[test]
fn repo_manifest_serializes_the_compact_content_packaging_profile() {
    let json = serde_json::to_value(RepoManifest::new("base".into())).unwrap();
    let profile = json
        .get("content_packaging_profile")
        .and_then(serde_json::Value::as_object)
        .expect("content_packaging_profile must be serialized as an object");

    assert_eq!(profile.get("chunk_size_profile").unwrap(), "chunk_128KiB");
    assert_eq!(profile.get("compression_profile").unwrap(), "zstd");
    assert_eq!(profile.get("compression_level").unwrap(), 3);
    assert_eq!(profile.get("bundle_size_profile").unwrap(), "bundle_8MiB");
    assert_eq!(profile.get("storage_mode").unwrap(), "bundled");
    assert!(json.get("artifact_profile").is_none());
}

#[test]
fn repo_manifest_does_not_serialize_remote_binding_ids() {
    let json = serde_json::to_value(RepoManifest::new("base".into())).unwrap();

    assert!(json.get("product_id").is_none());
    assert!(json.get("product_metadata").is_none());
    assert!(json.get("remote_repo_id").is_none());
}

#[test]
fn persisted_packaging_rejects_invalid_pairs_and_unknown_presets() {
    let original = serde_json::to_value(RepoManifest::new("base".into())).unwrap();
    let mut invalid = original.clone();
    invalid["content_packaging_profile"]["chunk_size_profile"] = "chunk_4MiB".into();
    assert!(serde_json::from_value::<RepoManifest>(invalid.clone()).is_err());
    invalid["content_packaging_profile"]["bundle_size_profile"] = "bundle_16MiB".into();
    assert!(serde_json::from_value::<RepoManifest>(invalid.clone()).is_ok());
    invalid["content_packaging_profile"]["chunk_size_profile"] = "chunk_3MiB".into();
    assert!(serde_json::from_value::<RepoManifest>(invalid).is_err());
    let mut invalid_compression = original;
    invalid_compression["content_packaging_profile"]["compression_level"] = 100.into();
    assert!(serde_json::from_value::<RepoManifest>(invalid_compression).is_err());
}
