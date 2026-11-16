use super::*;
use crate::capture::reuse::add_built_bundles_to_chunk_index;
use crate::repo::index::load_chunk_index;
use crate::repo::index::required_bundle_ids;
use crate::repo::index::write_chunk_index;
use crate::repo::paths::RepoPaths;
use crate::repo::paths::bundle_path;
use crate::repo::paths::chunk_path;
use crate::repo::paths::ensure_repo_layout;
use crate::repo::paths::load_repo_manifest;
use crate::repo::paths::write_repo_manifest;
use crate::repo::recovery::rebuild_chunk_index_from_artifacts;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use crate::repo::snapshot_manifests::save_snapshot_product_metadata_for_snapshots;
use crate::repo::snapshot_manifests::snapshot_blueprint_path;
use crate::repo::snapshot_manifests::snapshot_metadata_path;
use crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts;
use crate::test_util::{test_chunk_hash, test_hash};
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use crate::artifact::bundle::BundleBuilder;
use crate::artifact::bundle::read_bundle;
use crate::artifact::hash::ChunkHash;
use crate::artifact::profiles::ArtifactStorageMode;
use crate::artifact::profiles::BundleSizingPreset;
use crate::artifact::profiles::ChunkSizingPreset;
use crate::artifact::profiles::CompressionKind;
use crate::artifact::profiles::CompressionProfile;
use crate::capture::build::types::BuiltBundle;
use crate::capture::build::types::StagedArtifactFile;
use crate::manifest;
use crate::manifest::ChunkRef;
use crate::manifest::snapshot::SnapshotManifest;
use crate::repo::index::ChunkIndex;
use crate::repo::index::ChunkIndexEntries;
use crate::repo::index::ChunkIndexEntry;
use crate::repo::paths::bundle_id_from_bundle_path;
use crate::repo::paths::chunk_path_in_root;
use crate::repo::verify::RepoArtifactVerification;
use crate::repo::verify::verify_repo;
use crate::runtime::build_thread_pool;
use crate::snapshot::identity::snapshot_id_from_entries;
use crate::test_util::chunk_entries;
use crate::test_util::index_manifest;
use crate::test_util::loose_bytes;
use std::fs;
use std::slice::from_ref;

fn exclusive_tags(version: &str, os: &str, arch: &str, tier: &str) -> manifest::SnapshotTagSet {
    let mut tags = manifest::SnapshotTagSet::default();
    tags.insert("version", version).unwrap();
    tags.insert("os", os).unwrap();
    tags.insert("arch", arch).unwrap();
    tags.insert("tier", tier).unwrap();
    tags
}

#[test]
fn batch_product_metadata_save_gives_snapshots_the_same_canonical_hash() {
    let temp = tempfile::tempdir().unwrap();
    let paths = RepoPaths::from_repo_root(temp.path());
    ensure_repo_layout(&paths, "test-repo").unwrap();
    let mut snapshot_ids = Vec::new();
    for label in ["snapshot-a", "snapshot-b"] {
        let mut snapshot =
            SnapshotManifest::new(label.to_string(), manifest::SnapshotTagSet::default());
        snapshot.push_file(manifest::FileManifest {
            path: "game.exe".into(),
            size: 0,
            executable: true,
            read_only: label == "snapshot-a",
            full_file_hash: ChunkHash::of(b""),
            chunks: Vec::new(),
        });
        snapshot.snapshot_id = snapshot_id_from_entries(&snapshot.entries);
        write_snapshot_manifest_artifacts(
            &paths,
            &snapshot,
            &crate::progress::OperationControl::default(),
        )
        .unwrap();
        snapshot_ids.push(snapshot.snapshot_id);
    }

    save_snapshot_product_metadata_for_snapshots(
        temp.path(),
        &snapshot_ids,
        Some(manifest::SnapshotProductMetadata {
            document_type: Default::default(),
            document_version: Default::default(),
            runtime: manifest::SnapshotRuntimeMetadata::HostVariants(
                manifest::SnapshotHostVariantRuntimeMetadata {
                    host_variants: [("windows-x64", "os=windows"), ("linux-x64", "os=linux")]
                        .into_iter()
                        .map(
                            |(variant_id, os_requirement)| manifest::SnapshotHostVariant {
                                variant_id: variant_id.to_string(),
                                host_tag_requirements: vec![
                                    os_requirement.to_string(),
                                    "arch=x86_64".to_string(),
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

    let status = read_repo_status(temp.path()).unwrap();
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
fn repo_paths_from_repo_root() {
    let root = PathBuf::from("/test/repo");
    let paths = RepoPaths::from_repo_root(&root);
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
    let paths = RepoPaths::from_repo_root(temp.path());
    ensure_repo_layout(&paths, "linked").unwrap();

    let mut repo_manifest = load_repo_manifest(temp.path()).unwrap().1;
    repo_manifest.remote_repo_id = Some("99".to_string());
    write_repo_manifest(&paths, &repo_manifest).unwrap();

    let local_config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&paths.local_repo_config_path).unwrap()).unwrap();
    assert_eq!(
        local_config,
        serde_json::json!({
            "document_type": "gutch_local_repo_config",
            "document_version": "1.0.0",
            "remote_repo_id": "99",
        })
    );

    let repo_metadata: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&paths.repo_metadata_path).unwrap()).unwrap();
    assert!(repo_metadata.get("product_id").is_none());
    assert!(repo_metadata.get("product_metadata").is_none());
    assert!(repo_metadata.get("remote_repo_id").is_none());
}

#[test]
fn payload_paths_are_extensionless_and_bundle_names_are_hashes() {
    let root = PathBuf::from("/test/repo");
    let chunk_hash = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
    assert_eq!(
        chunk_path_in_root(
            &root.join("chunks"),
            ChunkHash::parse_hex(chunk_hash).unwrap()
        ),
        root.join("chunks").join("ab").join(chunk_hash)
    );
    assert_eq!(
        bundle_id_from_bundle_path(Path::new(chunk_hash)),
        Some(chunk_hash.to_string())
    );
    assert!(bundle_id_from_bundle_path(Path::new("bundle.bundle")).is_none());
}

#[test]
fn snapshot_manifest_files_live_in_per_snapshot_folder() {
    let root = PathBuf::from("/test/repo");
    let paths = RepoPaths::from_repo_root(&root);
    let snapshot_id = "0123456789abcdef";
    assert_eq!(
        snapshot_metadata_path(&paths, snapshot_id).unwrap(),
        root.join("snapshots")
            .join(snapshot_id)
            .join("snapshot_metadata.json")
    );
    assert_eq!(
        snapshot_blueprint_path(&paths, snapshot_id).unwrap(),
        root.join("snapshots")
            .join(snapshot_id)
            .join("snapshot_blueprint.gtcbin")
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
        full_file_hash: ChunkHash::of(b"hash"),
        chunks: vec![
            manifest::ChunkRef {
                chunk_hash: test_chunk_hash("chunk-a"),
                raw_size: 100,
            },
            manifest::ChunkRef {
                chunk_hash: test_chunk_hash("chunk-b"),
                raw_size: 100,
            },
        ],
    });

    let chunk_index = ChunkIndex {
        entries: chunk_entries(BTreeMap::from([
            (
                test_hash("chunk-a"),
                ChunkIndexEntry::new(
                    100,
                    0,
                    (test_hash("bundle-1")).into(),
                    100,
                    ArtifactStorageMode::Bundled,
                ),
            ),
            (
                test_hash("chunk-b"),
                ChunkIndexEntry::new(
                    100,
                    0,
                    (test_hash("bundle-1")).into(),
                    100,
                    ArtifactStorageMode::Bundled,
                ),
            ),
        ])),
    };

    let required = required_bundle_ids(&manifest, &chunk_index).unwrap();
    assert_eq!(required.len(), 1);
    assert!(required.contains(&test_hash("bundle-1")));
}

#[test]
fn check_repo_reports_snapshots() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let repo_root = root.join("repo");
    let repo_paths = RepoPaths::from_repo_root(&repo_root);

    ensure_repo_layout(&repo_paths, "base").unwrap();
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
        full_file_hash: test_chunk_hash("game.exe"),
        chunks: vec![ChunkRef {
            chunk_hash: test_chunk_hash("game.exe"),
            raw_size: 42,
        }],
    });

    manifest.snapshot_id = snapshot_id_from_entries(&manifest.entries);

    write_snapshot_manifest_artifacts(
        &repo_paths,
        &manifest,
        &crate::progress::OperationControl::default(),
    )
    .unwrap();
    index_manifest(&repo_paths, &manifest);
    let bundle_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let bundle_path = bundle_path(&repo_paths, bundle_id).unwrap();
    fs::create_dir_all(bundle_path.parent().unwrap()).unwrap();
    fs::write(bundle_path, b"bundle-a").unwrap();

    let status = read_repo_status(&repo_root).unwrap();

    assert!(status.exists);
    assert!(status.is_repo);
    assert_eq!(status.name, "base");
    assert_eq!(status.bundle_count, 1);
    assert_eq!(status.snapshots.len(), 1);
    assert_eq!(status.snapshots[0].snapshot_id, manifest.snapshot_id);
}

#[test]
fn check_repo_treats_missing_artifact_roots_as_empty() {
    let temp = tempfile::tempdir().unwrap();
    let paths = RepoPaths::from_repo_root(temp.path());
    ensure_repo_layout(&paths, "empty-artifacts").unwrap();
    fs::remove_dir_all(&paths.bundles_root).unwrap();
    fs::remove_dir_all(&paths.chunks_root).unwrap();

    let status = read_repo_status(temp.path()).unwrap();

    assert!(status.is_repo);
    assert_eq!(status.bundle_count, 0);
    assert_eq!(status.artifact_count, 0);
    assert!(status.snapshots.is_empty());
}

#[test]
fn check_and_verify_report_loose_chunk_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let paths = RepoPaths::from_repo_root(temp.path());
    ensure_repo_layout(&paths, "loose-repo").unwrap();

    let raw_bytes = b"loose artifact content";
    let chunk_hash = test_chunk_hash("loose artifact content");
    let compression_profile = CompressionProfile::default();
    let compressed_bytes = loose_bytes(raw_bytes, &compression_profile);
    let chunk_path = chunk_path(&paths, chunk_hash);
    fs::create_dir_all(chunk_path.parent().unwrap()).unwrap();
    fs::write(&chunk_path, &compressed_bytes).unwrap();

    let mut repo_manifest = load_repo_manifest(temp.path()).unwrap().1;
    repo_manifest.set_content_packaging_profile(
        ChunkSizingPreset::Chunk128KiB,
        CompressionKind::Zstd,
        Some(3),
        BundleSizingPreset::Bundle16MiB,
        ArtifactStorageMode::LooseChunks,
    );
    write_repo_manifest(&paths, &repo_manifest).unwrap();

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
        full_file_hash: chunk_hash,
        chunks: vec![manifest::ChunkRef {
            chunk_hash,
            raw_size: raw_bytes.len() as u32,
        }],
    });
    snapshot.snapshot_id = snapshot_id_from_entries(&snapshot.entries);
    write_snapshot_manifest_artifacts(
        &paths,
        &snapshot,
        &crate::progress::OperationControl::default(),
    )
    .unwrap();

    let index = ChunkIndex {
        entries: chunk_entries(BTreeMap::from([(
            chunk_hash.to_hex(),
            ChunkIndexEntry::new(
                compressed_bytes.len() as u32,
                0,
                chunk_hash.to_hex().into(),
                raw_bytes.len() as u32,
                ArtifactStorageMode::LooseChunks,
            ),
        )])),
    };
    write_chunk_index(&paths, &index).unwrap();

    let status = read_repo_status(temp.path()).unwrap();
    assert_eq!(status.artifact_count, 1);
    assert_eq!(status.snapshots[0].required_artifact_count, 1);
    assert_eq!(status.snapshots[0].missing_artifact_count, 0);

    let verification = verify_repo(
        temp.path(),
        Some(&snapshot.snapshot_id),
        RepoArtifactVerification::Strict,
    )
    .unwrap();
    assert!(verification.ok);
    assert_eq!(verification.snapshots[0].required_artifact_count, 1);
    assert_eq!(verification.snapshots[0].corrupt_artifact_count, 0);
}

#[test]
fn chunk_index_write_and_load_roundtrip() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    fs::create_dir_all(&root).unwrap();
    let paths = RepoPaths::from_repo_root(&root);

    let mut entries = BTreeMap::new();
    entries.insert(
        test_hash("chunk-a"),
        ChunkIndexEntry::new(
            80,
            0,
            (test_hash("bundle-1")).into(),
            100,
            ArtifactStorageMode::Bundled,
        ),
    );
    entries.insert(
        test_hash("chunk-b"),
        ChunkIndexEntry::new(
            80,
            0,
            (test_hash("bundle-1")).into(),
            100,
            ArtifactStorageMode::Bundled,
        ),
    );
    entries.insert(
        test_hash("chunk-c"),
        ChunkIndexEntry::new(
            40,
            0,
            (test_hash("bundle-2")).into(),
            100,
            ArtifactStorageMode::Bundled,
        ),
    );
    let index = ChunkIndex {
        entries: chunk_entries(entries),
    };
    write_chunk_index(&paths, &index).unwrap();
    assert!(paths.chunk_map_path.exists());

    let loaded = load_chunk_index(&paths).unwrap();
    assert_eq!(loaded.entries.len(), 3);
    assert_eq!(
        loaded
            .entries
            .get(&test_chunk_hash("chunk-a"))
            .map(|entry| entry.bundle_id.as_str()),
        Some(test_hash("bundle-1").as_str())
    );
    assert_eq!(
        loaded
            .entries
            .get(&test_chunk_hash("chunk-c"))
            .map(|entry| entry.bundle_id.as_str()),
        Some(test_hash("bundle-2").as_str())
    );
    assert!(!loaded.entries.contains_key(&test_chunk_hash("nonexistent")));
}

#[test]
fn chunk_index_load_empty_when_file_missing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    fs::create_dir_all(&root).unwrap();
    let paths = RepoPaths::from_repo_root(&root);

    let loaded = load_chunk_index(&paths).unwrap();
    assert!(loaded.entries.is_empty());
}

#[test]
fn incremental_chunk_index_matches_rebuilt_bundle_index() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let paths = RepoPaths::from_repo_root(&root);
    ensure_repo_layout(&paths, "base").unwrap();

    let chunk_hash = ChunkHash::of(b"hello");
    let mut builder = BundleBuilder::new(CompressionProfile::default());
    builder.add_chunk(chunk_hash, b"hello".to_vec());
    let (bundle_id, data) = builder.build_with_id().unwrap();
    let staged_path = root.join("staged-bundle");
    fs::write(&staged_path, &data).unwrap();
    let reader = read_bundle(&data).unwrap();
    let built_bundle = BuiltBundle {
        bundle_id: bundle_id.clone(),
        staged_file: StagedArtifactFile::new(staged_path),
        size: data.len() as u64,
        entries: reader.entries().to_vec(),
    };

    let mut incremental = ChunkIndex {
        entries: ChunkIndexEntries::new(),
    };
    add_built_bundles_to_chunk_index(&mut incremental, from_ref(&built_bundle)).unwrap();

    let bundle_path = bundle_path(&paths, &bundle_id).unwrap();
    fs::create_dir_all(bundle_path.parent().unwrap()).unwrap();
    fs::copy(built_bundle.staged_file.path(), bundle_path).unwrap();
    let pool = build_thread_pool(1, "gutch-test").unwrap();
    let rebuilt = rebuild_chunk_index_from_artifacts(&paths, &pool).unwrap();

    assert_eq!(incremental.entries, rebuilt.entries);
    assert_eq!(
        rebuilt
            .entries
            .get(&chunk_hash)
            .map(|entry| entry.bundle_id.as_str()),
        Some(bundle_id.as_str())
    );
}

#[test]
fn inspect_repo_shows_snapshot_tags() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let paths = RepoPaths::from_repo_root(&root);
    ensure_repo_layout(&paths, "base").unwrap();

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
        full_file_hash: test_chunk_hash("game.bin"),
        chunks: vec![ChunkRef {
            chunk_hash: test_chunk_hash("game.bin"),
            raw_size: 100,
        }],
    });
    manifest.snapshot_id = snapshot_id_from_entries(&manifest.entries);
    write_snapshot_manifest_artifacts(
        &paths,
        &manifest,
        &crate::progress::OperationControl::default(),
    )
    .unwrap();
    index_manifest(&paths, &manifest);

    let status = read_repo_status(&root).unwrap();
    assert_eq!(status.snapshots.len(), 1);
    assert_eq!(status.snapshots[0].snapshot_id, manifest.snapshot_id);
    assert_eq!(status.snapshots[0].version_tag, "2.0.0");
    assert_eq!(status.snapshots[0].os, "linux");
}

#[test]
fn snapshot_blueprint_roundtrip_preserves_hash_fields() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let paths = RepoPaths::from_repo_root(&root);
    ensure_repo_layout(&paths, "base").unwrap();

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
        full_file_hash: test_chunk_hash("game.exe"),
        chunks: vec![
            manifest::ChunkRef {
                chunk_hash: ChunkHash::parse_hex(&chunk_one).unwrap(),
                raw_size: 100,
            },
            manifest::ChunkRef {
                chunk_hash: ChunkHash::parse_hex(&chunk_two).unwrap(),
                raw_size: 200,
            },
        ],
    });

    manifest.snapshot_id = snapshot_id_from_entries(&manifest.entries);

    write_snapshot_manifest_artifacts(
        &paths,
        &manifest,
        &crate::progress::OperationControl::default(),
    )
    .unwrap();
    index_manifest(&paths, &manifest);
    let local_json =
        fs::read_to_string(snapshot_metadata_path(&paths, &manifest.snapshot_id).unwrap()).unwrap();
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
        loaded.files().next().unwrap().chunks[0].chunk_hash.to_hex(),
        chunk_one
    );
    assert_eq!(
        loaded.files().next().unwrap().chunks[1].chunk_hash.to_hex(),
        chunk_two
    );
}

#[test]
fn force_init_replaces_an_unparsable_manifest() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join(crate::manifest::REPO_METADATA_FILE),
        b"{not json",
    )
    .unwrap();
    let assessment = crate::repo::init_policy::assess_repo_init_path(temp.path()).unwrap();
    assert_eq!(
        assessment.path_kind,
        crate::repo::init_policy::RepoInitPathKind::InvalidManifest
    );
    let options = |force| crate::repo::init_policy::RepoInitOptions {
        force,
        constraints: Default::default(),
    };
    assert!(crate::repo::init_policy::initialize_repo(temp.path(), options(false)).is_err());
    crate::repo::init_policy::initialize_repo(temp.path(), options(true)).unwrap();
    load_repo_manifest(temp.path()).unwrap();
}

#[test]
fn deleting_a_missing_snapshot_reports_not_found() {
    let temp = tempfile::tempdir().unwrap();
    crate::repo::init_policy::initialize_repo(
        temp.path(),
        crate::repo::init_policy::RepoInitOptions {
            force: false,
            constraints: Default::default(),
        },
    )
    .unwrap();
    let error = crate::repo::snapshots::delete_snapshot(
        temp.path(),
        &"a".repeat(64),
        &crate::progress::OperationControl::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("snapshot not found"), "{error}");
}

#[test]
fn a_newer_repository_format_is_refused_for_reading_and_writing() {
    let temp = tempfile::tempdir().unwrap();
    crate::repo::init_policy::initialize_repo(temp.path(), Default::default()).unwrap();
    let path = temp.path().join(crate::manifest::REPO_METADATA_FILE);
    let mut metadata: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    metadata["repository_format"]["version"] = 2.into();
    fs::write(&path, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let is_unsupported = |error: crate::Error| {
        matches!(
            error,
            crate::Error::UnsupportedFormat {
                needs_newer_gutch: true,
                ..
            }
        )
    };
    assert!(is_unsupported(load_repo_manifest(temp.path()).unwrap_err()));
    assert!(is_unsupported(
        crate::repo::coordination::write(temp.path(), || Ok(())).unwrap_err()
    ));
}
