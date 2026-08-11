use super::*;
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::hash::hash_hex;
use crate::manifest;
use crate::manifest::gutch_repo::GutchRepoManifest;
use crate::manifest::target::TargetManifest;

fn exclusive_tags(version: &str, os: &str, arch: &str, tier: &str) -> manifest::TargetTagSet {
    let mut tags = manifest::TargetTagSet::default();
    tags.upsert_exclusive("version", version);
    tags.upsert_exclusive("os", os);
    tags.upsert_exclusive("arch", arch);
    tags.upsert_exclusive("tier", tier);
    tags
}

fn test_hash(label: &str) -> String {
    hash_hex(&crate::hash::hash_chunk(label.as_bytes()))
}

#[test]
fn gutch_repo_paths_from_repo_root() {
    let root = PathBuf::from("/test/repo");
    let paths = GutchRepoPaths::from_repo_root(&root);
    assert_eq!(paths.repo_root, root);
    assert_eq!(paths.repo_metadata_path, root.join("repo_metadata.json"));
    assert_eq!(paths.chunk_map_path, root.join("chunk_map.bin"));
    assert_eq!(paths.targets_root, root.join("targets"));
    assert_eq!(paths.bundles_root, root.join("bundles"));
    assert_eq!(paths.chunks_root, root.join("chunks"));
}

#[test]
fn target_manifest_files_live_in_per_target_folder() {
    let root = PathBuf::from("/test/repo");
    let paths = GutchRepoPaths::from_repo_root(&root);
    let target_id = "0123456789abcdef";
    assert_eq!(
        crate::target_manifest_path(&paths, target_id),
        root.join("targets")
            .join(target_id)
            .join("target_metadata.json")
    );
    assert_eq!(
        crate::target_manifest_bin_path(&paths, target_id),
        root.join("targets")
            .join(target_id)
            .join("target_blueprint.bin")
    );
}
#[test]
fn gutch_repo_paths_from_store_root() {
    let root = PathBuf::from("/test/store");
    let paths = GutchRepoPaths::from_store_root(&root, "my-repo");
    assert!(paths.repo_root.to_string_lossy().contains("my-repo"));
    assert!(
        paths
            .repo_metadata_path
            .to_string_lossy()
            .contains("my-repo")
    );
}

#[test]
fn required_bundle_ids_uses_chunk_index_mapping() {
    let mut manifest = TargetManifest::new(
        "1.0.0-windows-x86_64-base".into(),
        manifest::TargetTagSet::default(),
    );
    manifest.files.push(manifest::FileManifest {
        path: "game.exe".into(),
        size: 200,
        executable: true,
        full_file_hash: "hash".into(),
        chunks: vec![
            manifest::ChunkRef {
                chunk_hash: "chunk-a".into(),
                raw_size: 100,
            },
            manifest::ChunkRef {
                chunk_hash: "chunk-b".into(),
                raw_size: 100,
            },
        ],
    });

    let chunk_index = manifest::ChunkIndex {
        format_version: "1.0.0".into(),
        updated_at: String::new(),
        entries: BTreeMap::from([
            (
                "chunk-a".into(),
                manifest::ChunkIndexEntry {
                    bundle_id: "bundle-1".into(),
                    raw_size: 100,
                    compressed_size: 50,
                    storage_mode: manifest::ArtifactStorageMode::Bundled,
                },
            ),
            (
                "chunk-b".into(),
                manifest::ChunkIndexEntry {
                    bundle_id: "bundle-1".into(),
                    raw_size: 100,
                    compressed_size: 50,
                    storage_mode: manifest::ArtifactStorageMode::Bundled,
                },
            ),
        ]),
    };

    let required = required_bundle_ids(&manifest, &chunk_index);
    assert_eq!(required.len(), 1);
    assert!(required.contains("bundle-1"));
}

#[test]
fn check_gutch_repo_reports_targets() {
    let root = std::env::temp_dir().join(format!(
        "gutch_repo_inspect_test_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let repo_root = root.join("repo");
    let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
    let _ = std::fs::remove_dir_all(&root);

    ensure_gutch_repo_layout(&repo_paths, "base").unwrap();
    let mut manifest = TargetManifest::new(
        "1".to_string(),
        exclusive_tags("1.0.0", "windows", "x86_64", "base"),
    );
    manifest.files.push(manifest::FileManifest {
        path: "game.exe".to_string(),
        size: 42,
        executable: true,
        full_file_hash: test_hash("game.exe"),
        chunks: Vec::new(),
    });

    write_target_manifest_artifacts(&repo_paths, &manifest).unwrap();
    std::fs::write(repo_paths.bundles_root.join("bundle-a"), b"bundle-a").unwrap();

    let status = check_gutch_repo(&repo_root).unwrap();

    assert!(status.exists);
    assert!(status.is_repo);
    assert_eq!(status.name, "base");
    assert_eq!(status.bundle_count, 1);
    assert_eq!(status.targets.len(), 1);
    assert_eq!(status.targets[0].target_id, "1");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn discover_source_target_installs_returns_matching_subfolders_only() {
    let root = std::env::temp_dir().join(format!(
        "gutch_source_discover_test_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let repo_root = root.join("repo");
    let source_root = root.join("source-targets");
    let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
    let _ = std::fs::remove_dir_all(&root);

    ensure_gutch_repo_layout(&repo_paths, "mods").unwrap();

    let repo_manifest = GutchRepoManifest {
        target_schema: Some(manifest::TargetTagSchema {
            target_tags: vec![
                manifest::TargetTagDefinition {
                    name: "version".to_string(),
                    kind: manifest::TargetTagKind::Exclusive,
                    required: true,
                    inference_prefixes: vec!["-".to_string(), "-v".to_string()],
                    values: Vec::new(),
                    default_value: None,
                    inference_handler: manifest::InferenceHandler::Version,
                },
                manifest::TargetTagDefinition {
                    name: "loader".to_string(),
                    kind: manifest::TargetTagKind::Scoped,
                    required: false,
                    inference_prefixes: vec!["-".to_string()],
                    values: vec![
                        manifest::TargetTagValueDefinition {
                            value: "fabric".to_string(),
                            aliases: Vec::new(),
                        },
                        manifest::TargetTagValueDefinition {
                            value: "forge".to_string(),
                            aliases: Vec::new(),
                        },
                        manifest::TargetTagValueDefinition {
                            value: "neoforge".to_string(),
                            aliases: Vec::new(),
                        },
                    ],
                    default_value: None,
                    inference_handler: manifest::InferenceHandler::Default,
                },
                manifest::TargetTagDefinition {
                    name: "mc".to_string(),
                    kind: manifest::TargetTagKind::Scoped,
                    required: false,
                    inference_prefixes: vec!["-".to_string()],
                    values: vec![
                        manifest::TargetTagValueDefinition {
                            value: "1.20.1".to_string(),
                            aliases: Vec::new(),
                        },
                        manifest::TargetTagValueDefinition {
                            value: "1.21".to_string(),
                            aliases: Vec::new(),
                        },
                    ],
                    default_value: None,
                    inference_handler: manifest::InferenceHandler::Default,
                },
            ],
        }),
        ..GutchRepoManifest::new("mods".to_string())
    };
    std::fs::write(
        &repo_paths.repo_metadata_path,
        serde_json::to_string_pretty(&repo_manifest).unwrap(),
    )
    .unwrap();

    std::fs::create_dir_all(source_root.join("ae2-v15.4.10-fabric-1.21")).unwrap();
    std::fs::create_dir_all(source_root.join("not-a-target-folder")).unwrap();

    let candidates = discover_source_target_installs(&repo_root, &source_root).unwrap();

    assert_eq!(candidates.len(), 2);
    // First is the matched folder (sorted by path)
    assert_eq!(candidates[0].folder_name, "ae2-v15.4.10-fabric-1.21");
    assert_eq!(candidates[0].version_id, "15.4.10");
    assert_eq!(
        candidates[0].disposition,
        SourceTargetInstallDisposition::Unchanged
    );
    // Second is the unmatched folder (included but with empty fields)
    assert_eq!(candidates[1].folder_name, "not-a-target-folder");
    assert_eq!(candidates[1].target_id, "");
    assert_eq!(
        candidates[1].disposition,
        SourceTargetInstallDisposition::Invalid
    );
    assert_eq!(
        candidates[0]
            .target_tags
            .scoped
            .get("loader")
            .and_then(|values| values.iter().next())
            .map(String::as_str),
        Some("fabric")
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn chunk_index_write_and_load_roundtrip() {
    let root = std::env::temp_dir().join("gutch_chunk_index_test");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let paths = GutchRepoPaths::from_repo_root(&root);

    let mut entries = BTreeMap::new();
    entries.insert(
        test_hash("chunk-a"),
        manifest::ChunkIndexEntry {
            bundle_id: "bundle-1".to_string(),
            raw_size: 100,
            compressed_size: 40,
            storage_mode: manifest::ArtifactStorageMode::Bundled,
        },
    );
    entries.insert(
        test_hash("chunk-b"),
        manifest::ChunkIndexEntry {
            bundle_id: "bundle-1".to_string(),
            raw_size: 100,
            compressed_size: 40,
            storage_mode: manifest::ArtifactStorageMode::Bundled,
        },
    );
    entries.insert(
        test_hash("chunk-c"),
        manifest::ChunkIndexEntry {
            bundle_id: "bundle-2".to_string(),
            raw_size: 100,
            compressed_size: 40,
            storage_mode: manifest::ArtifactStorageMode::Bundled,
        },
    );
    let index = manifest::ChunkIndex {
        format_version: "1.0.0".to_string(),
        updated_at: "2026-05-14T00:00:00Z".to_string(),
        entries,
    };
    write_chunk_index(&paths, &index).unwrap();
    assert!(paths.chunk_map_path.exists());

    let loaded = load_chunk_index(&paths).unwrap();
    assert_eq!(loaded.format_version, "1.0.0");
    assert_eq!(loaded.entries.len(), 3);
    assert_eq!(
        loaded
            .entries
            .get(&test_hash("chunk-a"))
            .map(|entry| entry.bundle_id.as_str()),
        Some("bundle-1")
    );
    assert_eq!(
        loaded
            .entries
            .get(&test_hash("chunk-c"))
            .map(|entry| entry.bundle_id.as_str()),
        Some("bundle-2")
    );
    assert!(loaded.entries.get("nonexistent").is_none());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn chunk_index_load_empty_when_file_missing() {
    let root = std::env::temp_dir().join("gutch_chunk_index_empty_test");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let paths = GutchRepoPaths::from_repo_root(&root);

    let loaded = load_chunk_index(&paths).unwrap();
    assert!(loaded.entries.is_empty());
    assert_eq!(loaded.format_version, "1.0.0");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn incremental_chunk_index_matches_rebuilt_bundle_index() {
    let root = std::env::temp_dir().join("gutch_incremental_chunk_index_test");
    let _ = std::fs::remove_dir_all(&root);
    let paths = GutchRepoPaths::from_repo_root(&root);
    ensure_gutch_repo_layout(&paths, "base").unwrap();

    let chunk_hash = crate::hash::ChunkHash::from_blake3(crate::hash::hash_chunk(b"hello"));
    let mut builder = crate::bundle::BundleBuilder::new(manifest::CompressionProfile::default());
    builder.add_chunk(chunk_hash.clone(), b"hello".to_vec());
    let (bundle_id, data) = builder.build_with_id().unwrap();
    let built_bundle = crate::BuiltBundle {
        bundle_id: bundle_id.clone(),
        data,
    };

    let mut incremental = manifest::ChunkIndex {
        format_version: manifest::FORMAT_VERSION.to_string(),
        updated_at: String::new(),
        entries: BTreeMap::new(),
    };
    add_built_bundles_to_chunk_index(&mut incremental, std::slice::from_ref(&built_bundle))
        .unwrap();

    std::fs::write(bundle_path(&paths, &bundle_id), &built_bundle.data).unwrap();
    let rebuilt = rebuild_chunk_index_from_bundles(&paths).unwrap();

    assert_eq!(incremental.entries, rebuilt.entries);
    assert_eq!(
        rebuilt
            .entries
            .get(&chunk_hash.to_string())
            .map(|entry| entry.bundle_id.as_str()),
        Some(bundle_id.as_str())
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn inspect_repo_shows_target_tags() {
    let root = std::env::temp_dir().join("gutch_inspect_tags_test");
    let _ = std::fs::remove_dir_all(&root);
    let paths = GutchRepoPaths::from_repo_root(&root);
    ensure_gutch_repo_layout(&paths, "base").unwrap();

    let mut manifest = TargetManifest::new(
        "2".to_string(),
        exclusive_tags("2.0.0", "linux", "x86_64", "premium"),
    );
    manifest.files.push(manifest::FileManifest {
        path: "game.bin".to_string(),
        size: 100,
        executable: true,
        full_file_hash: test_hash("game.bin"),
        chunks: vec![],
    });
    write_target_manifest_artifacts(&paths, &manifest).unwrap();

    let status = check_gutch_repo(&root).unwrap();
    assert_eq!(status.targets.len(), 1);
    assert_eq!(status.targets[0].target_id, "2");
    assert_eq!(status.targets[0].version_id, "2.0.0");
    assert_eq!(status.targets[0].os, "linux");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn target_manifest_bin_roundtrip_preserves_hash_fields() {
    let root = std::env::temp_dir().join("gutch_target_manifest_bin_roundtrip");
    let _ = std::fs::remove_dir_all(&root);
    let paths = GutchRepoPaths::from_repo_root(&root);
    ensure_gutch_repo_layout(&paths, "base").unwrap();

    let chunk_one = test_hash("chunk-one");
    let chunk_two = test_hash("chunk-two");
    let mut manifest = TargetManifest::new(
        "target-a".to_string(),
        exclusive_tags("1.0.0", "windows", "x86_64", "base"),
    );
    manifest.files.push(manifest::FileManifest {
        path: "game.exe".to_string(),
        size: 300,
        executable: true,
        full_file_hash: test_hash("game.exe"),
        chunks: vec![
            manifest::ChunkRef {
                chunk_hash: chunk_one.clone(),
                raw_size: 100,
            },
            manifest::ChunkRef {
                chunk_hash: chunk_two.clone(),
                raw_size: 200,
            },
        ],
    });

    write_target_manifest_artifacts(&paths, &manifest).unwrap();
    let local_json = std::fs::read_to_string(target_manifest_path(&paths, "target-a")).unwrap();
    let local_value: serde_json::Value = serde_json::from_str(&local_json).unwrap();
    assert!(local_value.get("source_modified_at").is_some());
    assert!(local_value.get("files").is_some());
    let loaded = load_target_manifest(&paths, "target-a").unwrap();

    assert_eq!(loaded.files.len(), 1);
    assert_eq!(
        loaded.files[0].full_file_hash,
        manifest.files[0].full_file_hash
    );
    assert_eq!(loaded.files[0].chunks[0].chunk_hash, chunk_one);
    assert_eq!(loaded.files[0].chunks[1].chunk_hash, chunk_two);

    let _ = std::fs::remove_dir_all(&root);
}
