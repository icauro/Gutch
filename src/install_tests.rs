use super::*;
use crate::{GutchRepoPaths, write_chunk_index};
use std::io::{Read, Write};
use std::net::TcpListener;

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
            "chunk-a".to_string(),
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
                    "platform": "windows",
                    "architecture": "x86_64",
                    "tier_id": "base",
                    "inserted_at": "2026-05-11T00:00:00Z",
                    "files": [
                        {
                            "path": "game.exe",
                            "size": 100,
                                                                "executable": true,
                                                                "full_file_hash": "abc",
                            "chunks": [
                                { "chunk_hash": "chunk-a", "raw_size": 100 }
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
                            "url": format!("http://{}/bundle-a", address)
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
        manifest_url: format!("http://{}/manifest", address),
        chunk_index_url: None,
        bundle_urls_endpoint: Some(format!("http://{}/bundles", address)),
        session_id: Some("session-1".to_string()),
        bundles: Vec::new(),
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
