use super::*;
use crate::hash::hash_hex;
use crate::manifest::{self, FileManifest, SnapshotTagSet};
use crate::runtime::RuntimeResourceHints;
use crate::snapshot_ops::transfer;
use crate::snapshot_ops::{
    BundleDownload, SnapshotFetchRequest, fetch_snapshot, repair_snapshot_from_repo,
    resolve_snapshot_state,
};
use crate::{GutchRepoPaths, write_chunk_index};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::TcpListener;

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
    manifest
}

fn test_hash(label: &str) -> String {
    hash_hex(&blake3::hash(label.as_bytes()))
}

fn write_repair_test_repo(root: &std::path::Path, manifest: &SnapshotManifest) {
    let paths = GutchRepoPaths::from_repo_root(root);
    crate::ensure_gutch_repo_layout(&paths, "test").unwrap();
    crate::write_snapshot_manifest_artifacts(&paths, manifest).unwrap();
}

#[test]
fn bundle_download_deserializes_download_response() {
    let bundle: BundleDownload = serde_json::from_str(
        r#"{"bundleId":"abc123","downloadUrl":"https://example.com/bundle","expiresInSeconds":3600}"#,
    )
    .unwrap();
    assert_eq!(bundle.bundle_id, "abc123");
    assert_eq!(bundle.url, "https://example.com/bundle");
}

#[test]
fn bundle_url_provider_rejects_unexpected_bundle_ids() {
    let required_bundle_ids = HashSet::from(["bundle-a".to_string()]);
    let request = SnapshotFetchRequest {
        repo_path: std::path::PathBuf::from("C:\\repo"),
        snapshot_id: "snapshot".to_string(),
        repo_metadata_url: None,
        metadata_url: "https://example.com/metadata".to_string(),
        product_metadata_url: None,
        blueprint_url: "https://example.com/blueprint".to_string(),
        chunk_index_url: None,
        bundles: Vec::new(),
        bundle_url_provider: Some(std::sync::Arc::new(
            |_bundle_ids: &[String], _control: &crate::GutchOperationControl| {
                Ok(vec![BundleDownload {
                    bundle_id: "bundle-b".to_string(),
                    url: "https://example.com/bundle-b".to_string(),
                }])
            },
        )),
        control: crate::GutchOperationControl::default(),
        transfer_policy: crate::TransferConcurrencyPolicy::default(),
    };

    let error = transfer::fetch_bundle_urls(&request, &required_bundle_ids, &HashSet::new())
        .expect_err("unexpected bundle IDs must fail resolution");
    assert!(error.to_string().contains("unexpected bundle 'bundle-b'"));
}

#[test]
fn direct_bundle_urls_reject_duplicate_bundle_ids() {
    let required_bundle_ids = HashSet::from(["bundle-a".to_string()]);
    let request = SnapshotFetchRequest {
        repo_path: std::path::PathBuf::from("C:\\repo"),
        snapshot_id: "snapshot".to_string(),
        repo_metadata_url: None,
        metadata_url: "https://example.com/metadata".to_string(),
        product_metadata_url: None,
        blueprint_url: "https://example.com/blueprint".to_string(),
        chunk_index_url: None,
        bundles: vec![
            BundleDownload {
                bundle_id: "bundle-a".to_string(),
                url: "https://example.com/a".to_string(),
            },
            BundleDownload {
                bundle_id: " bundle-a ".to_string(),
                url: "https://example.com/a2".to_string(),
            },
        ],
        bundle_url_provider: None,
        control: crate::GutchOperationControl::default(),
        transfer_policy: crate::TransferConcurrencyPolicy::default(),
    };

    let error = transfer::fetch_bundle_urls(&request, &required_bundle_ids, &HashSet::new())
        .expect_err("duplicate bundle IDs must fail normalization");
    assert!(error.to_string().contains("duplicate direct bundle URL"));
}

#[test]
fn resolved_snapshot_state_uses_product_runtime() {
    let root = crate::test_util::temp_path("gutch_resolved_snapshot_launch_state");
    let _ = std::fs::remove_dir_all(&root);
    let paths = GutchRepoPaths::from_repo_root(&root);
    crate::ensure_gutch_repo_layout(&paths, "test").unwrap();
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
        chunks: Vec::new(),
    });
    crate::write_snapshot_manifest_artifacts(&paths, &snapshot).unwrap();

    snapshot.launch = Some(crate::ResolvedLaunchConfiguration {
        primary: None,
        actions: vec![crate::ResolvedLaunchAction {
            name: "Play".into(),
            executable: "Game.exe".into(),
            arguments: vec!["--snapshot".into()],
            working_directory: None,
        }],
    });
    crate::write_snapshot_manifest_artifacts(&paths, &snapshot).unwrap();
    let resolved = resolve_snapshot_state(&root, "snapshot").unwrap();
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

    let result = repair_snapshot_from_repo(crate::SnapshotRepairFromRepoRequest {
        repo_path: root.clone(),
        snapshot_id: manifest.snapshot_id.clone(),
        destination_dir: destination_dir.clone(),
        staging_dir: Some(staging_dir),
        remove_added_files: false,
        runtime_hints: RuntimeResourceHints::default(),
        control: crate::GutchOperationControl::default(),
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

    let result = repair_snapshot_from_repo(crate::SnapshotRepairFromRepoRequest {
        repo_path: root.clone(),
        snapshot_id: manifest.snapshot_id.clone(),
        destination_dir: destination_dir.clone(),
        staging_dir: Some(staging_dir),
        remove_added_files: true,
        runtime_hints: RuntimeResourceHints::default(),
        control: crate::GutchOperationControl::default(),
    })
    .unwrap()
    .repair_result;

    assert_eq!(result.extra_file_count, 2);
    assert_eq!(result.removed_extra_file_count, 1);
    assert!(!destination_dir.join("mods").join("mod.txt").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn repair_plan_only_requires_bundles_for_affected_files() {
    let root = crate::test_util::temp_path("gutch_repair_plan_bundle_subset");
    let _ = std::fs::remove_dir_all(&root);
    let paths = GutchRepoPaths::from_repo_root(&root);
    crate::ensure_gutch_repo_layout(&paths, "test").unwrap();

    let chunk_a = test_hash("chunk-a");
    let chunk_b = test_hash("chunk-b");
    let chunk_index = manifest::ChunkIndex {
        format_version: "1".to_string(),
        updated_at: String::new(),
        entries: std::collections::BTreeMap::from([
            (
                chunk_a.clone(),
                manifest::ChunkIndexEntry {
                    bundle_id: "bundle-a".to_string(),
                    raw_size: 4,
                    compressed_size: 4,
                    storage_mode: manifest::ArtifactStorageMode::Bundled,
                },
            ),
            (
                chunk_b.clone(),
                manifest::ChunkIndexEntry {
                    bundle_id: "bundle-b".to_string(),
                    raw_size: 4,
                    compressed_size: 4,
                    storage_mode: manifest::ArtifactStorageMode::Bundled,
                },
            ),
        ]),
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
    crate::write_snapshot_manifest_artifacts(&paths, &manifest).unwrap();

    let destination_dir = root.join("destination");
    std::fs::create_dir_all(&destination_dir).unwrap();
    std::fs::write(destination_dir.join("game.exe"), b"bad!").unwrap();
    std::fs::write(destination_dir.join("data.bin"), b"data").unwrap();

    let plan = crate::plan_repair_snapshot_from_repo(crate::SnapshotRepairPlanFromRepoRequest {
        repo_path: root.clone(),
        snapshot_id: "snapshot".to_string(),
        destination_dir: destination_dir.clone(),
        remove_added_files: false,
        runtime_hints: RuntimeResourceHints::default(),
        control: crate::GutchOperationControl::default(),
    })
    .unwrap();

    assert_eq!(plan.missing_file_count, 0);
    assert_eq!(plan.corrupt_file_count, 1);
    assert_eq!(plan.required_bundle_ids, vec!["bundle-a".to_string()]);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
#[ignore = "integration test: requires local mock HTTP server"]
fn fetch_snapshot_downloads_manifest_and_bundles() {
    let root = std::env::temp_dir().join(format!(
        "gutch_fetch_snapshot_test_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let repo_path = root.join("repo");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&repo_path).unwrap();
    let content = vec![b'x'; 100];
    let chunk_hash = hash_hex(&blake3::hash(&content));
    let mut builder =
        crate::artifact::bundle::BundleBuilder::new(manifest::CompressionProfile::default());
    builder.add_chunk(crate::ChunkHash::parse_hex(&chunk_hash).unwrap(), content);
    let (bundle_id, bundle_bytes) = builder.build_with_id().unwrap();
    let expected_bundle_id = bundle_id.clone();
    let chunk_index = manifest::ChunkIndex {
        format_version: "1".to_string(),
        updated_at: String::new(),
        entries: std::collections::BTreeMap::from([(
            chunk_hash.clone(),
            manifest::ChunkIndexEntry {
                bundle_id: bundle_id.clone(),
                raw_size: 100,
                compressed_size: 64,
                storage_mode: manifest::ArtifactStorageMode::Bundled,
            },
        )]),
    };
    let paths = GutchRepoPaths::from_repo_root(&repo_path);
    write_chunk_index(&paths, &chunk_index).unwrap();
    let mut blueprint_manifest = SnapshotManifest::new(
        "1.0.0-windows-x86_64-base".to_string(),
        SnapshotTagSet::from_compact_strings(&["version::1.0.0".to_string()]).unwrap(),
    );
    blueprint_manifest.push_file(FileManifest {
        path: "game.exe".to_string(),
        size: 100,
        executable: true,
        read_only: false,
        full_file_hash: test_hash("game.exe"),
        chunks: vec![manifest::ChunkRef {
            chunk_hash: chunk_hash.clone(),
            raw_size: 100,
        }],
    });
    let blueprint_path = root.join("fixture-snapshot-blueprint.bin");
    manifest::binary::write_snapshot_blueprint_binary(
        &blueprint_path,
        &blueprint_manifest.blueprint(),
    )
    .unwrap();
    let blueprint_bytes = std::fs::read(&blueprint_path).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..6 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 4096];
            let bytes_read = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..bytes_read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");

            let (status, body, content_type) = match path {
                "/metadata" => (
                    "200 OK",
                    serde_json::to_vec(&serde_json::json!({
                    "snapshot_metadata_version": "1.0.0",
                    "captured_at": "2026-05-11T00:00:00Z",
                    "snapshot_tags": ["version::1.0.0"],
                                            }))
                    .unwrap(),
                    "application/json",
                ),
                "/blueprint" => (
                    "200 OK",
                    blueprint_bytes.clone(),
                    "application/octet-stream",
                ),
                "/bundle-a" => ("200 OK", bundle_bytes.clone(), "application/octet-stream"),
                _ => ("404 Not Found", b"not found".to_vec(), "text/plain"),
            };

            let response = format!(
                "HTTP/1.1 {}\r\nContent-Length: {}\r\nContent-Type: {}\r\nConnection: close\r\n\r\n",
                status,
                body.len(),
                content_type
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        }
    });

    let request = SnapshotFetchRequest {
        repo_path: repo_path.clone(),
        snapshot_id: "1.0.0-windows-x86_64-base".to_string(),
        repo_metadata_url: None,
        metadata_url: format!("http://{}/metadata", address),
        product_metadata_url: None,
        blueprint_url: format!("http://{}/blueprint", address),
        chunk_index_url: None,
        bundles: Vec::new(),
        bundle_url_provider: Some(std::sync::Arc::new(
            move |bundle_ids: &[String], _control: &crate::GutchOperationControl| {
                assert_eq!(bundle_ids, &[bundle_id.clone()]);
                Ok(vec![BundleDownload {
                    bundle_id: bundle_id.clone(),
                    url: format!("http://{}/bundle-a", address),
                }])
            },
        )),
        control: crate::GutchOperationControl::default(),
        transfer_policy: crate::TransferConcurrencyPolicy::default(),
    };
    let result = fetch_snapshot(&request).unwrap();

    assert_eq!(result.required_bundle_count, 1);
    assert_eq!(result.downloaded_bundle_count, 1);
    assert!(repo_path.join("repo_metadata.json").exists() || repo_path.join("repo.json").exists());
    assert!(
        repo_path
            .join("snapshots")
            .join("1.0.0-windows-x86_64-base")
            .join("snapshot_metadata.json")
            .exists()
    );
    assert!(
        repo_path
            .join("snapshots")
            .join("1.0.0-windows-x86_64-base")
            .join("snapshot_blueprint.gtcbin")
            .exists()
    );
    assert!(repo_path.join("snapshots").exists());
    assert!(
        crate::bundle_path(&paths, &expected_bundle_id)
            .unwrap()
            .exists()
    );

    let cached = crate::bundle_path(&paths, &expected_bundle_id).unwrap();
    std::fs::write(&cached, b"corrupted cached artifact").unwrap();
    let retry = fetch_snapshot(&request).unwrap();
    assert_eq!(retry.downloaded_bundle_count, 1);
    crate::artifact::bundle::verify_bundle_identity_file(&cached, &expected_bundle_id).unwrap();
    server.join().unwrap();
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
    write_repair_test_repo(root.path(), &manifest);
    repair_snapshot_from_repo(crate::SnapshotRepairFromRepoRequest {
        repo_path: root.path().to_path_buf(),
        snapshot_id: manifest.snapshot_id,
        destination_dir: destination.clone(),
        staging_dir: None,
        remove_added_files: true,
        runtime_hints: RuntimeResourceHints::default(),
        control: crate::GutchOperationControl::default(),
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
    crate::helpers::remove_dir_all(&destination).unwrap();
}
