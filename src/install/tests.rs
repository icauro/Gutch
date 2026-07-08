use super::*;
use crate::hash::hash_hex;
use crate::install::{
    GutchBundleDownload, TargetInstallFetchRequest, fetch_target_install_content,
    repair_target_install_with_timings_and_hints, resolve_target_install_state,
};
use crate::manifest::{self, FileManifest, TargetTagSet};
use crate::runtime::RuntimeResourceHints;
use crate::{GutchRepoPaths, write_chunk_index};
use std::io::{Read, Write};
use std::net::TcpListener;

fn test_target_manifest(files: Vec<(&str, &[u8])>) -> TargetManifest {
    let mut manifest = TargetManifest::new("repair-target".to_string(), TargetTagSet::default());
    manifest.files = files
        .into_iter()
        .map(|(path, bytes)| FileManifest {
            path: path.to_string(),
            size: bytes.len() as u64,
            executable: false,
            full_file_hash: hash_hex(&blake3::hash(bytes)),
            chunks: Vec::new(),
        })
        .collect();
    manifest
}

fn test_hash(label: &str) -> String {
    hash_hex(&blake3::hash(label.as_bytes()))
}

#[test]
fn gutch_bundle_download_deserializes_backend_bundle_downloads_response() {
    let bundle: GutchBundleDownload = serde_json::from_str(
        r#"{"bundleId":"abc123","downloadUrl":"https://example.com/bundle","expiresInSeconds":3600}"#,
    )
    .unwrap();
    assert_eq!(bundle.bundle_id, "abc123");
    assert_eq!(bundle.url, "https://example.com/bundle");
}

#[test]
fn resolved_install_state_uses_repo_inheritance_and_target_override() {
    let root = crate::test_util::temp_path("gutch_resolved_install_launch_state");
    let _ = std::fs::remove_dir_all(&root);
    let paths = GutchRepoPaths::from_repo_root(&root);
    crate::ensure_gutch_repo_layout(&paths, "test").unwrap();
    let (_, mut repo_manifest) = crate::load_gutch_repo_manifest(&root).unwrap();
    repo_manifest.launch = Some(crate::RepoLaunchConfiguration {
        primary: None,
        actions: vec![crate::RepoLaunchAction {
            name: "Play".into(),
            commands: vec![crate::RepoLaunchCommand {
                platform: None,
                executable: "Game.exe".into(),
                arguments: vec!["--repo".into()],
                working_directory: None,
            }],
        }],
    });
    crate::write_gutch_repo_manifest(&paths, &repo_manifest).unwrap();

    let mut tags = TargetTagSet::default();
    tags.upsert_exclusive("os", "windows");
    tags.upsert_exclusive("arch", "x86_64");
    let mut target = TargetManifest::new("target".into(), tags);
    target.files.push(FileManifest {
        path: "Game.exe".into(),
        size: 1,
        executable: true,
        full_file_hash: test_hash("Game.exe"),
        chunks: Vec::new(),
    });
    crate::write_target_manifest_artifacts(&paths, &target).unwrap();

    let inherited = resolve_target_install_state(&root, "target").unwrap();
    assert_eq!(inherited.launch.unwrap().actions[0].arguments, ["--repo"]);
    assert!(!inherited.manifest_hash.is_empty());

    target.launch = Some(crate::ResolvedLaunchConfiguration {
        primary: None,
        actions: vec![crate::ResolvedLaunchAction {
            name: "Play".into(),
            executable: "Game.exe".into(),
            arguments: vec!["--target".into()],
            working_directory: None,
        }],
    });
    crate::write_target_manifest_artifacts(&paths, &target).unwrap();
    let overridden = resolve_target_install_state(&root, "target").unwrap();
    assert_eq!(
        overridden.launch.unwrap().actions[0].arguments,
        ["--target"]
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn repair_target_install_keeps_added_files_when_requested() {
    let root = crate::test_util::temp_path("gutch_repair_keep_added_files");
    let _ = std::fs::remove_dir_all(&root);
    let install_dir = root.join("install");
    let staging_dir = root.join("staging");
    std::fs::create_dir_all(&install_dir).unwrap();
    std::fs::write(install_dir.join("game.dat"), b"").unwrap();
    std::fs::write(install_dir.join("mod.txt"), b"user data").unwrap();
    let manifest = test_target_manifest(vec![("game.dat", b"")]);

    let result = repair_target_install_with_timings_and_hints(
        &manifest,
        &[],
        &install_dir,
        &staging_dir,
        false,
        &RuntimeResourceHints::default(),
    )
    .unwrap();

    assert_eq!(result.extra_file_count, 1);
    assert_eq!(result.removed_extra_file_count, 0);
    assert!(install_dir.join("mod.txt").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn repair_target_install_full_repair_deletes_added_files() {
    let root = crate::test_util::temp_path("gutch_repair_delete_added_files");
    let _ = std::fs::remove_dir_all(&root);
    let install_dir = root.join("install");
    let staging_dir = root.join("staging");
    std::fs::create_dir_all(install_dir.join("mods")).unwrap();
    std::fs::write(install_dir.join("game.dat"), b"").unwrap();
    std::fs::write(install_dir.join("mods").join("mod.txt"), b"user data").unwrap();
    let manifest = test_target_manifest(vec![("game.dat", b"")]);

    let result = repair_target_install_with_timings_and_hints(
        &manifest,
        &[],
        &install_dir,
        &staging_dir,
        true,
        &RuntimeResourceHints::default(),
    )
    .unwrap();

    assert_eq!(result.extra_file_count, 1);
    assert_eq!(result.removed_extra_file_count, 1);
    assert!(!install_dir.join("mods").join("mod.txt").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
#[ignore = "integration test: requires local mock HTTP server"]
fn fetch_target_install_content_downloads_manifest_and_bundles() {
    let root = std::env::temp_dir().join(format!(
        "gutch_fetch_install_test_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let repo_path = root.join("repo");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&repo_path).unwrap();
    let chunk_index = manifest::ChunkIndex {
        format_version: "1.0.0".to_string(),
        updated_at: String::new(),
        entries: std::collections::BTreeMap::from([(
            test_hash("chunk-a"),
            manifest::ChunkIndexEntry {
                bundle_id: "bundle-a".to_string(),
                raw_size: 100,
                compressed_size: 64,
                storage_mode: manifest::ArtifactStorageMode::Bundled,
            },
        )]),
    };
    let paths = GutchRepoPaths::from_repo_root(&repo_path);
    write_chunk_index(&paths, &chunk_index).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..4 {
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
                "/manifest" => (
                    "200 OK",
                    serde_json::to_vec(&serde_json::json!({
                    "version_id": "1.0.0",
                    "os": "windows",
                    "arch": "x86_64",
                    "inserted_at": "2026-05-11T00:00:00Z",
                    "files": [
                        {
                            "path": "game.exe",
                            "size": 100,
                                                                "executable": true,
                                                                "full_file_hash": test_hash("game.exe"),
                            "chunks": [
                                { "chunk_hash": test_hash("chunk-a"), "raw_size": 100 }
                            ]
                        }
                    ],
                    "target_tags": {"version": "1.0.0"},
                    "manifest_version": "1.0.0",
                    "chunk_sizing_profile": manifest::ChunkSizingProfile::default(),
                    "compression_profile": manifest::CompressionProfile::default(),
                    "bundle_sizing_profile": manifest::BundleSizingProfile::default(),
                    "total_logical_size": 0,
                    "total_compressed_size_estimate": 0,
                                            }))
                    .unwrap(),
                    "application/json",
                ),
                "/bundle-a" => (
                    "200 OK",
                    b"bundle-a-bytes".to_vec(),
                    "application/octet-stream",
                ),
                _ if path.starts_with("/bundles?ids=bundle-a") => (
                    "200 OK",
                    serde_json::to_vec(&serde_json::json!({
                        "bundles": [{
                            "bundleId": "bundle-a",
                            "downloadUrl": format!("http://{}/bundle-a", address),
                            "expiresInSeconds": 3600
                        }]
                    }))
                    .unwrap(),
                    "application/json",
                ),
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

    let result = fetch_target_install_content(&TargetInstallFetchRequest {
        repo_path: repo_path.clone(),
        target_id: "1.0.0-windows-x86_64-base".to_string(),
        repo_manifest_url: None,
        manifest_url: format!("http://{}/manifest", address),
        chunk_index_url: None,
        bundle_downloads_endpoint: Some(format!("http://{}/bundles", address)),
        session_id: Some("session-1".to_string()),
        bundles: Vec::new(),
        progress: None,
    })
    .unwrap();

    assert_eq!(result.required_bundle_count, 1);
    assert_eq!(result.downloaded_bundle_count, 1);
    assert!(repo_path.join("repo_manifest.json").exists() || repo_path.join("repo.json").exists());
    assert!(
        repo_path
            .join("targets")
            .join("1.0.0-windows-x86_64-base.json")
            .exists()
    );
    assert!(
        repo_path
            .join("targets")
            .join("1.0.0-windows-x86_64-base.bin")
            .exists()
    );
    assert!(repo_path.join("targets").exists());
    assert!(repo_path.join("bundles").join("bundle-a").exists());

    server.join().unwrap();
    let _ = std::fs::remove_dir_all(&root);
}
