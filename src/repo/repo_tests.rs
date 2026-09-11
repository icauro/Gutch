use super::*;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use crate::artifact::hash::hash_hex;
use crate::manifest;
use crate::manifest::gutch_repo::GutchRepoManifest;
use crate::manifest::snapshot::SnapshotManifest;
use crate::repo::check::SourceSnapshotOperationDisposition;

fn exclusive_tags(version: &str, os: &str, arch: &str, tier: &str) -> manifest::SnapshotTagSet {
    let mut tags = manifest::SnapshotTagSet::default();
    tags.upsert_exclusive("version", version);
    tags.upsert_exclusive("os", os);
    tags.upsert_exclusive("arch", arch);
    tags.upsert_exclusive("tier", tier);
    tags
}

fn test_hash(label: &str) -> String {
    hash_hex(&crate::artifact::hash::hash_chunk(label.as_bytes()))
}

#[test]
fn existing_store_keeps_same_path_manifests_from_multiple_snapshots() {
    let temp = tempfile::tempdir().unwrap();
    let paths = GutchRepoPaths::from_repo_root(temp.path());
    ensure_gutch_repo_layout(&paths, "test-repo").unwrap();

    for (snapshot_id, content_label) in [("snapshot-a", "content-a"), ("snapshot-b", "content-b")] {
        let mut manifest =
            SnapshotManifest::new(snapshot_id.to_string(), manifest::SnapshotTagSet::default());
        manifest.push_file(manifest::FileManifest {
            path: "game.bin".to_string(),
            size: 9,
            executable: false,
            read_only: false,
            full_file_hash: test_hash(content_label),
            chunks: vec![crate::manifest::ChunkRef {
                chunk_hash: test_hash(content_label),
                raw_size: 9,
            }],
        });
        manifest.snapshot_id =
            crate::snapshot::identity::compute_snapshot_id_from_entries(&manifest.entries);
        write_snapshot_manifest_artifacts(&paths, &manifest).unwrap();
        crate::test_util::index_manifest(&paths, &manifest);
    }

    let pool = crate::runtime::build_thread_pool(1, "gutch-test").unwrap();
    let state = scan_existing_store_state_with_pool(
        &paths,
        &pool,
        &crate::progress::GutchOperationControl::default(),
    )
    .unwrap();
    let candidates = state.previous_file_manifests.get("game.bin").unwrap();

    assert_eq!(candidates.len(), 2);
    assert_ne!(candidates[0].full_file_hash, candidates[1].full_file_hash);
}

#[test]
fn batch_product_metadata_save_gives_snapshots_the_same_canonical_hash() {
    let temp = tempfile::tempdir().unwrap();
    let paths = GutchRepoPaths::from_repo_root(temp.path());
    ensure_gutch_repo_layout(&paths, "test-repo").unwrap();
    let mut snapshot_ids = Vec::new();
    for label in ["snapshot-a", "snapshot-b"] {
        let mut snapshot =
            SnapshotManifest::new(label.to_string(), manifest::SnapshotTagSet::default());
        snapshot.push_file(manifest::FileManifest {
            path: "game.exe".to_string(),
            size: 0,
            executable: true,
            read_only: false,
            full_file_hash: test_hash(label),
            chunks: Vec::new(),
        });
        snapshot.snapshot_id =
            crate::snapshot::identity::compute_snapshot_id_from_entries(&snapshot.entries);
        write_snapshot_manifest_artifacts(&paths, &snapshot).unwrap();
        snapshot_ids.push(snapshot.snapshot_id);
    }

    save_snapshot_product_metadata_for_snapshots(
        temp.path(),
        &snapshot_ids,
        Some(manifest::SnapshotProductMetadata {
            document_version: manifest::PUBLIC_METADATA_FORMAT_VERSION.to_string(),
            runtime: manifest::SnapshotRuntimeMetadata::HostVariants(
                manifest::SnapshotHostVariantRuntimeMetadata {
                    host_variants: [("windows-x64", "os::windows"), ("linux-x64", "os::linux")]
                        .into_iter()
                        .map(
                            |(variant_id, os_requirement)| manifest::SnapshotHostVariant {
                                variant_id: variant_id.to_string(),
                                host_tag_requirements: vec![
                                    os_requirement.to_string(),
                                    "arch::x86_64".to_string(),
                                ],
                                launch_actions: vec![manifest::SnapshotLaunchAction {
                                    action_name: "Play".to_string(),
                                    executable_path: "game.exe".to_string(),
                                    command_arguments: Vec::new(),
                                    working_directory_path: None,
                                }],
                                external_dependencies: Vec::new(),
                            },
                        )
                        .collect(),
                },
            ),
        }),
    )
    .unwrap();

    let status = check_gutch_repo(temp.path()).unwrap();
    let hashes = status
        .snapshots
        .iter()
        .map(|snapshot| snapshot.snapshot_product_metadata_hash_b3.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(hashes.len(), 2);
    assert!(hashes[0].is_some());
    assert_eq!(hashes[0], hashes[1]);
    let saved_metadata = status.snapshots[0]
        .snapshot_product_metadata
        .as_ref()
        .unwrap();
    let manifest::SnapshotRuntimeMetadata::HostVariants(runtime) = &saved_metadata.runtime else {
        panic!("expected host-variant product metadata");
    };
    assert_eq!(runtime.host_variants.len(), 2);
}

#[test]
fn gutch_repo_paths_from_repo_root() {
    let root = PathBuf::from("/test/repo");
    let paths = GutchRepoPaths::from_repo_root(&root);
    assert_eq!(paths.repo_root(), root);
    assert_eq!(paths.repo_metadata_path, root.join("repo_metadata.json"));
    assert_eq!(
        paths.local_repo_config_path,
        root.join("local_repo_config.json")
    );
    assert_eq!(paths.chunk_map_path, root.join("chunk_map.gtcbin"));
    assert_eq!(paths.snapshots_root, root.join("snapshots"));
    assert_eq!(paths.bundles_root, root.join("bundles"));
    assert_eq!(paths.chunks_root, root.join("chunks"));
}

#[test]
fn local_repo_config_contains_only_the_remote_link() {
    let temp = tempfile::tempdir().unwrap();
    let paths = GutchRepoPaths::from_repo_root(temp.path());
    ensure_gutch_repo_layout(&paths, "linked").unwrap();

    let mut repo_manifest = load_gutch_repo_manifest(temp.path()).unwrap().1;
    repo_manifest.remote_repo_id = "99".to_string();
    write_gutch_repo_manifest(&paths, &repo_manifest).unwrap();

    let local_config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&paths.local_repo_config_path).unwrap())
            .unwrap();
    assert_eq!(local_config, serde_json::json!({"remote_repo_id": "99"}));

    let repo_metadata: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&paths.repo_metadata_path).unwrap()).unwrap();
    assert!(repo_metadata.get("product_id").is_none());
    assert!(repo_metadata.get("product_metadata").is_none());
    assert!(repo_metadata.get("remote_repo_id").is_none());
}

#[test]
fn payload_paths_are_extensionless_and_bundle_names_are_hashes() {
    let root = PathBuf::from("/test/repo");
    let chunk_hash = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
    assert_eq!(
        crate::repo::paths::chunk_path_in_root(&root.join("chunks"), chunk_hash).unwrap(),
        root.join("chunks").join("ab").join(chunk_hash)
    );
    assert_eq!(
        crate::repo::paths::bundle_id_from_bundle_path(Path::new(chunk_hash)),
        Some(chunk_hash.to_string())
    );
    assert!(crate::repo::paths::bundle_id_from_bundle_path(Path::new("bundle.bundle")).is_none());
}

#[test]
fn snapshot_manifest_files_live_in_per_snapshot_folder() {
    let root = PathBuf::from("/test/repo");
    let paths = GutchRepoPaths::from_repo_root(&root);
    let snapshot_id = "0123456789abcdef";
    assert_eq!(
        crate::repo::snapshot_manifests::snapshot_metadata_path(&paths, snapshot_id).unwrap(),
        root.join("snapshots")
            .join(snapshot_id)
            .join("snapshot_metadata.json")
    );
    assert_eq!(
        crate::repo::snapshot_blueprint_path(&paths, snapshot_id).unwrap(),
        root.join("snapshots")
            .join(snapshot_id)
            .join("snapshot_blueprint.gtcbin")
    );
}
#[test]
fn gutch_repo_paths_from_store_root() {
    let root = PathBuf::from("/test/store");
    let paths = GutchRepoPaths::from_store_root(&root, "my-repo").unwrap();
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
    let mut manifest = SnapshotManifest::new(
        "1.0.0-windows-x86_64-base".into(),
        manifest::SnapshotTagSet::default(),
    );
    manifest.push_file(manifest::FileManifest {
        path: "game.exe".into(),
        size: 200,
        executable: true,
        read_only: false,
        full_file_hash: "hash".into(),
        chunks: vec![
            manifest::ChunkRef {
                chunk_hash: test_hash("chunk-a"),
                raw_size: 100,
            },
            manifest::ChunkRef {
                chunk_hash: test_hash("chunk-b"),
                raw_size: 100,
            },
        ],
    });

    let chunk_index = manifest::ChunkIndex {
        format_version: "1".into(),
        updated_at: String::new(),
        entries: crate::test_util::chunk_entries(BTreeMap::from([
            (
                test_hash("chunk-a"),
                manifest::ChunkIndexEntry {
                    artifact_size: 0,
                    chunk_ordinal: 0,
                    bundle_id: (test_hash("bundle-1")).into(),
                    raw_size: 100,
                    compressed_size: 50,
                    storage_mode: manifest::ArtifactStorageMode::Bundled,
                },
            ),
            (
                test_hash("chunk-b"),
                manifest::ChunkIndexEntry {
                    artifact_size: 0,
                    chunk_ordinal: 0,
                    bundle_id: (test_hash("bundle-1")).into(),
                    raw_size: 100,
                    compressed_size: 50,
                    storage_mode: manifest::ArtifactStorageMode::Bundled,
                },
            ),
        ])),
    };

    let required = required_bundle_ids(&manifest, &chunk_index).unwrap();
    assert_eq!(required.len(), 1);
    assert!(required.contains(&test_hash("bundle-1")));
}

#[test]
fn check_gutch_repo_reports_snapshots() {
    let root = std::env::temp_dir().join(format!(
        "gutch_repo_inspect_test_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let repo_root = root.join("repo");
    let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
    let _ = std::fs::remove_dir_all(&root);

    ensure_gutch_repo_layout(&repo_paths, "base").unwrap();
    let snapshot_id = test_hash("snapshot-one");
    let mut manifest = SnapshotManifest::new(
        snapshot_id.clone(),
        exclusive_tags("1.0.0", "windows", "x86_64", "base"),
    );
    manifest.push_file(manifest::FileManifest {
        path: "game.exe".to_string(),
        size: 42,
        executable: true,
        read_only: false,
        full_file_hash: test_hash("game.exe"),
        chunks: vec![crate::manifest::ChunkRef {
            chunk_hash: test_hash("game.exe"),
            raw_size: 42,
        }],
    });

    manifest.snapshot_id =
        crate::snapshot::identity::compute_snapshot_id_from_entries(&manifest.entries);

    write_snapshot_manifest_artifacts(&repo_paths, &manifest).unwrap();
    crate::test_util::index_manifest(&repo_paths, &manifest);
    let bundle_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let bundle_path = crate::repo::paths::bundle_path(&repo_paths, bundle_id).unwrap();
    std::fs::create_dir_all(bundle_path.parent().unwrap()).unwrap();
    std::fs::write(bundle_path, b"bundle-a").unwrap();

    let status = check_gutch_repo(&repo_root).unwrap();

    assert!(status.exists);
    assert!(status.is_repo);
    assert_eq!(status.name, "base");
    assert_eq!(status.bundle_count, 1);
    assert_eq!(status.snapshots.len(), 1);
    assert_eq!(status.snapshots[0].snapshot_id, manifest.snapshot_id);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn check_gutch_repo_treats_missing_artifact_roots_as_empty() {
    let temp = tempfile::tempdir().unwrap();
    let paths = GutchRepoPaths::from_repo_root(temp.path());
    ensure_gutch_repo_layout(&paths, "empty-artifacts").unwrap();
    std::fs::remove_dir_all(&paths.bundles_root).unwrap();
    std::fs::remove_dir_all(&paths.chunks_root).unwrap();

    let status = check_gutch_repo(temp.path()).unwrap();

    assert!(status.is_repo);
    assert_eq!(status.bundle_count, 0);
    assert_eq!(status.artifact_count, 0);
    assert!(status.snapshots.is_empty());
}

#[test]
fn check_and_verify_report_loose_chunk_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let paths = GutchRepoPaths::from_repo_root(temp.path());
    ensure_gutch_repo_layout(&paths, "loose-repo").unwrap();

    let raw_bytes = b"loose artifact content";
    let chunk_hash = test_hash("loose artifact content");
    let compression_profile = manifest::CompressionProfile::default();
    let compressed_bytes = crate::test_util::loose_bytes(raw_bytes, &compression_profile);
    let chunk_path = chunk_path(&paths, &chunk_hash).unwrap();
    std::fs::create_dir_all(chunk_path.parent().unwrap()).unwrap();
    std::fs::write(&chunk_path, &compressed_bytes).unwrap();

    let mut repo_manifest = load_gutch_repo_manifest(temp.path()).unwrap().1;
    repo_manifest.set_artifact_profile(
        manifest::ArtifactSizingPreset::Chunk128KiB,
        manifest::CompressionKind::Zstd,
        Some(3),
        manifest::BundleSizingPreset::Bundle16MiB,
        manifest::ArtifactStorageMode::LooseChunks,
    );
    write_gutch_repo_manifest(&paths, &repo_manifest).unwrap();

    let snapshot_id = test_hash("loose snapshot");
    let mut snapshot = SnapshotManifest::new(
        snapshot_id.clone(),
        exclusive_tags("1.0.0", "windows", "x86_64", "base"),
    );
    snapshot.push_file(manifest::FileManifest {
        path: "game.exe".to_string(),
        size: raw_bytes.len() as u64,
        executable: true,
        read_only: false,
        full_file_hash: chunk_hash.clone(),
        chunks: vec![manifest::ChunkRef {
            chunk_hash: chunk_hash.clone(),
            raw_size: raw_bytes.len() as u32,
        }],
    });
    snapshot.snapshot_id =
        crate::snapshot::identity::compute_snapshot_id_from_entries(&snapshot.entries);
    write_snapshot_manifest_artifacts(&paths, &snapshot).unwrap();

    let index = manifest::ChunkIndex {
        format_version: manifest::FORMAT_VERSION.to_string(),
        updated_at: String::new(),
        entries: crate::test_util::chunk_entries(BTreeMap::from([(
            chunk_hash.clone(),
            manifest::ChunkIndexEntry {
                artifact_size: 0,
                chunk_ordinal: 0,
                bundle_id: (chunk_hash).into(),
                raw_size: raw_bytes.len() as u32,
                compressed_size: compressed_bytes.len() as u32,
                storage_mode: manifest::ArtifactStorageMode::LooseChunks,
            },
        )])),
    };
    write_chunk_index(&paths, &index).unwrap();

    let status = check_gutch_repo(temp.path()).unwrap();
    assert_eq!(status.artifact_count, 1);
    assert_eq!(status.snapshots[0].required_artifact_count, 1);
    assert_eq!(status.snapshots[0].missing_artifact_count, 0);

    let verification = crate::repo::verify::verify_gutch_repo_artifacts(
        temp.path(),
        Some(&snapshot.snapshot_id),
        crate::verify::VerifyMode::Strict,
    )
    .unwrap();
    assert!(verification.ok);
    assert_eq!(verification.snapshots[0].required_artifact_count, 1);
    assert_eq!(verification.snapshots[0].corrupt_artifact_count, 0);
}

#[test]
fn discover_source_snapshot_operations_returns_matching_subfolders_only() {
    let root = std::env::temp_dir().join(format!(
        "gutch_source_discover_test_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let repo_root = root.join("repo");
    let source_root = root.join("source-snapshots");
    let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
    let _ = std::fs::remove_dir_all(&root);

    ensure_gutch_repo_layout(&repo_paths, "mods").unwrap();

    let repo_manifest = GutchRepoManifest::new("mods".to_string());
    std::fs::write(
        &repo_paths.repo_metadata_path,
        serde_json::to_string_pretty(&repo_manifest).unwrap(),
    )
    .unwrap();

    std::fs::create_dir_all(source_root.join("ae2-v15.4.10-loader-fabric-mc-1.21")).unwrap();
    std::fs::create_dir_all(source_root.join("not-a-snapshot-folder")).unwrap();

    let candidates = discover_source_snapshot_operations(&repo_root, &source_root).unwrap();

    assert_eq!(candidates.len(), 2);
    // Folder names are not interpreted as tags.
    assert_eq!(
        candidates[0].folder_name,
        "ae2-v15.4.10-loader-fabric-mc-1.21"
    );
    assert_eq!(candidates[0].version_id, "");
    assert_eq!(
        candidates[0].disposition,
        SourceSnapshotOperationDisposition::Unchanged
    );
    // Every discovered directory is a valid tagless source.
    assert_eq!(candidates[1].folder_name, "not-a-snapshot-folder");
    assert_eq!(
        candidates[1].disposition,
        SourceSnapshotOperationDisposition::Unchanged
    );
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.snapshot_tags.is_empty())
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
            artifact_size: 0,
            chunk_ordinal: 0,
            bundle_id: (test_hash("bundle-1")).into(),
            raw_size: 100,
            compressed_size: 40,
            storage_mode: manifest::ArtifactStorageMode::Bundled,
        },
    );
    entries.insert(
        test_hash("chunk-b"),
        manifest::ChunkIndexEntry {
            artifact_size: 0,
            chunk_ordinal: 0,
            bundle_id: (test_hash("bundle-1")).into(),
            raw_size: 100,
            compressed_size: 40,
            storage_mode: manifest::ArtifactStorageMode::Bundled,
        },
    );
    entries.insert(
        test_hash("chunk-c"),
        manifest::ChunkIndexEntry {
            artifact_size: 0,
            chunk_ordinal: 0,
            bundle_id: (test_hash("bundle-2")).into(),
            raw_size: 100,
            compressed_size: 40,
            storage_mode: manifest::ArtifactStorageMode::Bundled,
        },
    );
    let index = manifest::ChunkIndex {
        format_version: "1".to_string(),
        updated_at: "2026-05-14T00:00:00Z".to_string(),
        entries: crate::test_util::chunk_entries(entries),
    };
    write_chunk_index(&paths, &index).unwrap();
    assert!(paths.chunk_map_path.exists());

    let loaded = load_chunk_index(&paths).unwrap();
    assert_eq!(loaded.format_version, "1");
    assert_eq!(loaded.entries.len(), 3);
    assert_eq!(
        loaded
            .entries
            .get(&test_hash("chunk-a"))
            .map(|entry| entry.bundle_id.as_str()),
        Some(test_hash("bundle-1").as_str())
    );
    assert_eq!(
        loaded
            .entries
            .get(&test_hash("chunk-c"))
            .map(|entry| entry.bundle_id.as_str()),
        Some(test_hash("bundle-2").as_str())
    );
    assert!(!loaded.entries.contains_key("nonexistent"));

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
    assert_eq!(loaded.format_version, "1");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn incremental_chunk_index_matches_rebuilt_bundle_index() {
    let root = std::env::temp_dir().join("gutch_incremental_chunk_index_test");
    let _ = std::fs::remove_dir_all(&root);
    let paths = GutchRepoPaths::from_repo_root(&root);
    ensure_gutch_repo_layout(&paths, "base").unwrap();

    let chunk_hash =
        crate::artifact::hash::ChunkHash::from_blake3(crate::artifact::hash::hash_chunk(b"hello"));
    let mut builder =
        crate::artifact::bundle::BundleBuilder::new(manifest::CompressionProfile::default());
    builder.add_chunk(chunk_hash, b"hello".to_vec());
    let (bundle_id, data) = builder.build_with_id().unwrap();
    let staged_path = root.join("staged-bundle");
    std::fs::write(&staged_path, &data).unwrap();
    let reader =
        crate::artifact::bundle::read_bundle(&data, &manifest::CompressionProfile::default())
            .unwrap();
    let built_bundle = crate::capture::types::BuiltBundle {
        bundle_id: bundle_id.clone(),
        staged_file: crate::capture::types::StagedArtifactFile::new(staged_path),
        size: data.len() as u64,
        entries: reader.entries().to_vec(),
    };

    let mut incremental = manifest::ChunkIndex {
        format_version: manifest::FORMAT_VERSION.to_string(),
        updated_at: String::new(),
        entries: crate::manifest::ChunkIndexEntries::new(),
    };
    add_built_bundles_to_chunk_index(
        &mut incremental,
        std::slice::from_ref(&built_bundle),
        &manifest::CompressionProfile::default(),
    )
    .unwrap();

    let bundle_path = bundle_path(&paths, &bundle_id).unwrap();
    std::fs::create_dir_all(bundle_path.parent().unwrap()).unwrap();
    std::fs::copy(built_bundle.staged_file.path(), bundle_path).unwrap();
    let pool = crate::runtime::build_thread_pool(1, "gutch-test").unwrap();
    let rebuilt = rebuild_chunk_index_from_bundles_with_pool(
        &paths,
        &manifest::CompressionProfile::default(),
        &pool,
    )
    .unwrap();

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
fn inspect_repo_shows_snapshot_tags() {
    let root = std::env::temp_dir().join("gutch_inspect_tags_test");
    let _ = std::fs::remove_dir_all(&root);
    let paths = GutchRepoPaths::from_repo_root(&root);
    ensure_gutch_repo_layout(&paths, "base").unwrap();

    let snapshot_id = test_hash("snapshot-two");
    let mut manifest = SnapshotManifest::new(
        snapshot_id.clone(),
        exclusive_tags("2.0.0", "linux", "x86_64", "premium"),
    );
    manifest.push_file(manifest::FileManifest {
        path: "game.bin".to_string(),
        size: 100,
        executable: true,
        read_only: false,
        full_file_hash: test_hash("game.bin"),
        chunks: vec![crate::manifest::ChunkRef {
            chunk_hash: test_hash("game.bin"),
            raw_size: 100,
        }],
    });
    manifest.snapshot_id =
        crate::snapshot::identity::compute_snapshot_id_from_entries(&manifest.entries);
    write_snapshot_manifest_artifacts(&paths, &manifest).unwrap();
    crate::test_util::index_manifest(&paths, &manifest);

    let status = check_gutch_repo(&root).unwrap();
    assert_eq!(status.snapshots.len(), 1);
    assert_eq!(status.snapshots[0].snapshot_id, manifest.snapshot_id);
    assert_eq!(status.snapshots[0].version_id, "2.0.0");
    assert_eq!(status.snapshots[0].os, "linux");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn snapshot_blueprint_roundtrip_preserves_hash_fields() {
    let root = std::env::temp_dir().join("gutch_snapshot_blueprint_roundtrip");
    let _ = std::fs::remove_dir_all(&root);
    let paths = GutchRepoPaths::from_repo_root(&root);
    ensure_gutch_repo_layout(&paths, "base").unwrap();

    let chunk_one = test_hash("chunk-one");
    let chunk_two = test_hash("chunk-two");
    let mut manifest = SnapshotManifest::new(
        "snapshot-a".to_string(),
        exclusive_tags("1.0.0", "windows", "x86_64", "base"),
    );
    manifest.push_file(manifest::FileManifest {
        path: "game.exe".to_string(),
        size: 300,
        executable: true,
        read_only: false,
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

    manifest.snapshot_id =
        crate::snapshot::identity::compute_snapshot_id_from_entries(&manifest.entries);

    write_snapshot_manifest_artifacts(&paths, &manifest).unwrap();
    crate::test_util::index_manifest(&paths, &manifest);
    let local_json =
        std::fs::read_to_string(snapshot_metadata_path(&paths, &manifest.snapshot_id).unwrap())
            .unwrap();
    let local_value: serde_json::Value = serde_json::from_str(&local_json).unwrap();
    assert!(local_value.get("source_modified_at").is_none());
    assert!(local_value.get("files").is_none());
    assert!(
        snapshot_blueprint_path(&paths, &manifest.snapshot_id)
            .unwrap()
            .is_file()
    );
    let loaded = load_snapshot_manifest(&paths, &manifest.snapshot_id).unwrap();

    assert_eq!(loaded.file_count(), 1);
    assert_eq!(
        loaded.files().next().unwrap().full_file_hash,
        manifest.files().next().unwrap().full_file_hash
    );
    assert_eq!(
        loaded.files().next().unwrap().chunks[0].chunk_hash,
        chunk_one
    );
    assert_eq!(
        loaded.files().next().unwrap().chunks[1].chunk_hash,
        chunk_two
    );

    let _ = std::fs::remove_dir_all(&root);
}
