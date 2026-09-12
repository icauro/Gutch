use super::*;
use crate::artifact::hash::hash_hex;
use crate::installation::repair_snapshot_from_repo;
use crate::installation::resolve_snapshot_state;
use crate::manifest;
use crate::manifest::FileManifest;
use crate::manifest::SnapshotTagSet;
use crate::repo::index::write_chunk_index;
use crate::repo::paths::GutchRepoPaths;
use crate::runtime::RuntimeResourceHints;
use crate::transfer::download as transfer;
use crate::transfer::types::ArtifactDownload;
use crate::transfer::types::SnapshotFetchRequest;
use std::collections::HashSet;

fn test_snapshot_manifest(files: Vec<(&str, &[u8])>) -> SnapshotManifest {
    let mut manifest =
        SnapshotManifest::new("repair-snapshot".to_string(), SnapshotTagSet::default());
    manifest.set_files(
        files
            .into_iter()
            .map(|(path, bytes)| FileManifest {
                path: path.to_string(),
                size: bytes.len() as u64,
                executable: false,
                read_only: false,
                full_file_hash: hash_hex(&blake3::hash(bytes)),
                chunks: Vec::new(),
            })
            .collect(),
    );
    manifest.snapshot_id =
        crate::snapshot::identity::compute_snapshot_id_from_entries(&manifest.entries);
    manifest
}

fn test_hash(label: &str) -> String {
    hash_hex(&blake3::hash(label.as_bytes()))
}

fn write_repair_test_repo(root: &std::path::Path, manifest: &SnapshotManifest) {
    let paths = GutchRepoPaths::from_repo_root(root);
    crate::repo::paths::ensure_gutch_repo_layout(&paths, "test").unwrap();
    crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts(&paths, manifest).unwrap();
}

#[test]
fn artifact_download_deserializes_artifact_response() {
    let bundle: ArtifactDownload = serde_json::from_str(
        r#"{"artifactId":"abc123","url":"https://example.com/bundle","expiresInSeconds":3600}"#,
    )
    .unwrap();
    assert_eq!(bundle.artifact_id, "abc123");
    assert_eq!(bundle.url, "https://example.com/bundle");
}

#[test]
fn artifact_url_provider_rejects_unexpected_artifact_ids() {
    let required_artifact_ids = HashSet::from(["bundle-a".to_string()]);
    let request = SnapshotFetchRequest {
        repo_path: std::path::PathBuf::from("C:\\repo"),
        snapshot_id: "snapshot".to_string(),
        repo_metadata_url: None,
        metadata_url: "https://example.com/metadata".to_string(),
        product_metadata_url: None,
        blueprint_url: "https://example.com/blueprint".to_string(),
        chunk_index_url: None,
        artifacts: Vec::new(),
        artifact_url_provider: Some(std::sync::Arc::new(
            |_artifact_ids: &[String], _control: &crate::progress::GutchOperationControl| {
                Ok(vec![ArtifactDownload {
                    artifact_id: "bundle-b".to_string(),
                    url: "https://example.com/bundle-b".to_string(),
                }])
            },
        )),
        control: crate::progress::GutchOperationControl::default(),
        transfer_policy: crate::transfer::types::TransferConcurrencyPolicy::default(),
    };

    let error = transfer::fetch_artifact_urls(&request, &required_artifact_ids, &HashSet::new())
        .expect_err("unexpected artifact IDs must fail resolution");
    assert!(error.to_string().contains("unexpected artifact 'bundle-b'"));
}

#[test]
fn direct_bundle_urls_reject_duplicate_artifact_ids() {
    let required_artifact_ids = HashSet::from(["bundle-a".to_string()]);
    let request = SnapshotFetchRequest {
        repo_path: std::path::PathBuf::from("C:\\repo"),
        snapshot_id: "snapshot".to_string(),
        repo_metadata_url: None,
        metadata_url: "https://example.com/metadata".to_string(),
        product_metadata_url: None,
        blueprint_url: "https://example.com/blueprint".to_string(),
        chunk_index_url: None,
        artifacts: vec![
            ArtifactDownload {
                artifact_id: "bundle-a".to_string(),
                url: "https://example.com/a".to_string(),
            },
            ArtifactDownload {
                artifact_id: " bundle-a ".to_string(),
                url: "https://example.com/a2".to_string(),
            },
        ],
        artifact_url_provider: None,
        control: crate::progress::GutchOperationControl::default(),
        transfer_policy: crate::transfer::types::TransferConcurrencyPolicy::default(),
    };

    let error = transfer::fetch_artifact_urls(&request, &required_artifact_ids, &HashSet::new())
        .expect_err("duplicate bundle IDs must fail normalization");
    assert!(error.to_string().contains("duplicate direct artifact URL"));
}

#[test]
fn resolved_snapshot_state_uses_product_runtime() {
    let root = crate::test_util::temp_path("gutch_resolved_snapshot_launch_state");
    let _ = std::fs::remove_dir_all(&root);
    let paths = GutchRepoPaths::from_repo_root(&root);
    crate::repo::paths::ensure_gutch_repo_layout(&paths, "test").unwrap();
    let mut tags = SnapshotTagSet::default();
    tags.upsert_exclusive("os", "windows");
    tags.upsert_exclusive("arch", "x86_64");
    let mut snapshot = SnapshotManifest::new("snapshot".into(), tags);
    snapshot.push_file(FileManifest {
        path: "Game.exe".into(),
        size: 1,
        executable: true,
        read_only: false,
        full_file_hash: test_hash("Game.exe"),
        chunks: vec![crate::manifest::ChunkRef {
            chunk_hash: test_hash("Game.exe"),
            raw_size: 1,
        }],
    });
    snapshot.snapshot_id =
        crate::snapshot::identity::compute_snapshot_id_from_entries(&snapshot.entries);
    crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts(&paths, &snapshot).unwrap();

    snapshot.launch = Some(crate::manifest::launch::ResolvedLaunchConfiguration {
        primary: None,
        actions: vec![crate::manifest::launch::ResolvedLaunchAction {
            name: "Play".into(),
            executable: "Game.exe".into(),
            arguments: vec!["--snapshot".into()],
            working_directory: None,
        }],
    });
    snapshot.snapshot_id =
        crate::snapshot::identity::compute_snapshot_id_from_entries(&snapshot.entries);
    crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts(&paths, &snapshot).unwrap();
    let resolved = resolve_snapshot_state(&root, &snapshot.snapshot_id).unwrap();
    assert_eq!(
        resolved.launch.unwrap().actions[0].arguments,
        ["--snapshot"]
    );
    assert!(!resolved.snapshot_metadata_hash_b3.is_empty());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn repair_snapshot_ops_keeps_added_files_when_requested() {
    let root = crate::test_util::temp_path("gutch_repair_keep_added_files");
    let _ = std::fs::remove_dir_all(&root);
    let destination_dir = root.join("destination");
    let staging_dir = root.join("staging");
    std::fs::create_dir_all(&destination_dir).unwrap();
    std::fs::write(destination_dir.join("game.dat"), b"").unwrap();
    std::fs::write(destination_dir.join("mod.txt"), b"user data").unwrap();
    let manifest = test_snapshot_manifest(vec![("game.dat", b"")]);
    write_repair_test_repo(&root, &manifest);

    let result = repair_snapshot_from_repo(crate::installation::SnapshotRepairFromRepoRequest {
        repo_path: root.clone(),
        snapshot_id: manifest.snapshot_id.clone(),
        destination_dir: destination_dir.clone(),
        staging_dir: Some(staging_dir),
        remove_added_files: false,
        runtime_hints: RuntimeResourceHints::default(),
        control: crate::progress::GutchOperationControl::default(),
    })
    .unwrap()
    .repair_result;

    assert_eq!(result.extra_file_count, 1);
    assert_eq!(result.removed_extra_file_count, 0);
    assert!(destination_dir.join("mod.txt").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn repair_snapshot_ops_full_repair_deletes_added_files() {
    let root = crate::test_util::temp_path("gutch_repair_delete_added_files");
    let _ = std::fs::remove_dir_all(&root);
    let destination_dir = root.join("destination");
    let staging_dir = root.join("staging");
    std::fs::create_dir_all(destination_dir.join("mods")).unwrap();
    std::fs::write(destination_dir.join("game.dat"), b"").unwrap();
    std::fs::write(destination_dir.join("mods").join("mod.txt"), b"user data").unwrap();
    let manifest = test_snapshot_manifest(vec![("game.dat", b"")]);
    write_repair_test_repo(&root, &manifest);

    let result = repair_snapshot_from_repo(crate::installation::SnapshotRepairFromRepoRequest {
        repo_path: root.clone(),
        snapshot_id: manifest.snapshot_id.clone(),
        destination_dir: destination_dir.clone(),
        staging_dir: Some(staging_dir),
        remove_added_files: true,
        runtime_hints: RuntimeResourceHints::default(),
        control: crate::progress::GutchOperationControl::default(),
    })
    .unwrap()
    .repair_result;

    assert_eq!(result.extra_file_count, 2);
    assert_eq!(result.removed_extra_file_count, 2);
    assert!(!destination_dir.join("mods").exists());
    assert!(!destination_dir.join("mods").join("mod.txt").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn repair_plan_only_requires_bundles_for_affected_files() {
    let root = crate::test_util::temp_path("gutch_repair_plan_bundle_subset");
    let _ = std::fs::remove_dir_all(&root);
    let paths = GutchRepoPaths::from_repo_root(&root);
    crate::repo::paths::ensure_gutch_repo_layout(&paths, "test").unwrap();

    let chunk_a = test_hash("chunk-a");
    let chunk_b = test_hash("chunk-b");
    let chunk_index = manifest::ChunkIndex {
        format_version: "1".to_string(),

        entries: crate::test_util::chunk_entries(std::collections::BTreeMap::from([
            (
                chunk_a.clone(),
                manifest::ChunkIndexEntry::new(
                    0,
                    0,
                    test_hash("bundle-a").into(),
                    4,
                    4,
                    manifest::ArtifactStorageMode::Bundled,
                ),
            ),
            (
                chunk_b.clone(),
                manifest::ChunkIndexEntry::new(
                    0,
                    0,
                    test_hash("bundle-b").into(),
                    4,
                    4,
                    manifest::ArtifactStorageMode::Bundled,
                ),
            ),
        ])),
    };
    write_chunk_index(&paths, &chunk_index).unwrap();

    let mut manifest = SnapshotManifest::new("snapshot".to_string(), SnapshotTagSet::default());
    manifest.set_files(vec![
        FileManifest {
            path: "game.exe".to_string(),
            size: 4,
            executable: true,
            read_only: false,
            full_file_hash: test_hash("game"),
            chunks: vec![manifest::ChunkRef {
                chunk_hash: chunk_a,
                raw_size: 4,
            }],
        },
        FileManifest {
            path: "data.bin".to_string(),
            size: 4,
            executable: false,
            read_only: false,
            full_file_hash: test_hash("data"),
            chunks: vec![manifest::ChunkRef {
                chunk_hash: chunk_b,
                raw_size: 4,
            }],
        },
    ]);
    manifest.snapshot_id =
        crate::snapshot::identity::compute_snapshot_id_from_entries(&manifest.entries);
    crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts(&paths, &manifest).unwrap();

    let destination_dir = root.join("destination");
    std::fs::create_dir_all(&destination_dir).unwrap();
    std::fs::write(destination_dir.join("game.exe"), b"bad!").unwrap();
    std::fs::write(destination_dir.join("data.bin"), b"data").unwrap();

    let plan = crate::installation::repair::plan_repair_snapshot_from_repo(
        crate::installation::SnapshotRepairPlanFromRepoRequest {
            repo_path: root.clone(),
            snapshot_id: manifest.snapshot_id.clone(),
            destination_dir: destination_dir.clone(),
            remove_added_files: false,
            runtime_hints: RuntimeResourceHints::default(),
            control: crate::progress::GutchOperationControl::default(),
        },
    )
    .unwrap();

    assert_eq!(plan.missing_file_count, 0);
    assert_eq!(plan.corrupt_file_count, 1);
    assert_eq!(plan.required_bundle_ids, vec![test_hash("bundle-a")]);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn repair_restores_manifest_file_attributes() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("destination");
    std::fs::create_dir_all(&destination).unwrap();
    std::fs::write(destination.join("game.dat"), b"").unwrap();
    let mut manifest = test_snapshot_manifest(vec![("game.dat", b"")]);
    if let crate::manifest::SnapshotEntry::File(file) = &mut manifest.entries[0] {
        file.read_only = true;
        file.executable = true;
    }
    manifest.snapshot_id =
        crate::snapshot::identity::compute_snapshot_id_from_entries(&manifest.entries);
    write_repair_test_repo(root.path(), &manifest);
    repair_snapshot_from_repo(crate::installation::SnapshotRepairFromRepoRequest {
        repo_path: root.path().to_path_buf(),
        snapshot_id: manifest.snapshot_id,
        destination_dir: destination.clone(),
        staging_dir: None,
        remove_added_files: true,
        runtime_hints: RuntimeResourceHints::default(),
        control: crate::progress::GutchOperationControl::default(),
    })
    .unwrap();
    let permissions = std::fs::metadata(destination.join("game.dat"))
        .unwrap()
        .permissions();
    assert!(permissions.readonly());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(permissions.mode() & 0o111, 0);
    }
    crate::filesystem::remove_dir_all(&destination).unwrap();
}
