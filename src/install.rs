use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::manifest::{self, TargetManifest};
use crate::reconstruct::{
    ReconstructTimings, VerifiedSourceInstall,
    reconstruct_from_bundle_paths_and_verified_source_with_timings,
};

use crate::verify::{
    VerifyTimings, classify_directory_with_timings, verify_directory_with_timings,
};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

fn derive_chunk_count(manifest: &TargetManifest) -> usize {
    let mut seen = HashSet::new();
    for file in &manifest.files {
        for chunk in &file.chunks {
            seen.insert(&chunk.chunk_hash);
        }
    }
    seen.len()
}

#[derive(Debug, Clone)]
pub struct TargetInstallApplySource<'a> {
    pub target_manifest: &'a TargetManifest,
    pub install_dir: &'a Path,
}

#[derive(Debug, Clone)]
pub struct TargetInstallApplyResult {
    pub target_id: String,
    pub install_dir: PathBuf,
    pub staging_dir: PathBuf,
    pub backup_dir: Option<PathBuf>,
    pub reused_source_chunk_count: usize,
    pub required_chunk_count: usize,
    pub required_bundle_count: usize,
    pub reconstruct_timings: ReconstructTimings,
    pub verify_timings: VerifyTimings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GutchBundleDownload {
    pub bundle_id: String,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct TargetInstallFetchRequest {
    pub repo_path: PathBuf,
    pub target_id: String,
    pub manifest_url: String,
    pub manifest_sig_url: Option<String>,
    pub bundle_urls_endpoint: Option<String>,
    pub session_id: Option<String>,
    pub bundles: Vec<GutchBundleDownload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetInstallFetchResult {
    pub target_id: String,
    pub repo_path: PathBuf,
    pub required_bundle_count: usize,
    pub downloaded_bundle_count: usize,
    pub skipped_existing_bundle_count: usize,
    pub wrote_manifest_signature: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GutchBundleUrlsResponse {
    #[serde(default)]
    bundles: Vec<GutchBundleDownload>,
}

pub fn apply_target_install_with_timings(
    target_manifest: &TargetManifest,
    bundle_paths: &[PathBuf],
    install_dir: &Path,
    staging_dir: &Path,
    source: Option<TargetInstallApplySource<'_>>,
) -> Result<TargetInstallApplyResult> {
    if install_dir == staging_dir {
        return Err(Error::PathError {
            path: install_dir.to_path_buf(),
            reason: "install and staging directories must be different".to_string(),
        });
    }

    let verified_source = match source {
        Some(source) => {
            let (source_classification, _) =
                classify_directory_with_timings(source.target_manifest, source.install_dir)?;
            if !source_classification.missing_files.is_empty()
                || !source_classification.corrupt_files.is_empty()
            {
                let source_errors = source_classification
                    .missing_files
                    .iter()
                    .chain(source_classification.corrupt_files.iter())
                    .map(|issue| issue.detail.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(Error::verify(format!(
                    "source install did not verify: {}",
                    source_errors
                )));
            }

            Some(VerifiedSourceInstall {
                target_manifest: source.target_manifest,
                install_dir: source.install_dir,
            })
        }
        None => None,
    };

    if staging_dir.exists() {
        std::fs::remove_dir_all(staging_dir)?;
    }
    if let Some(parent) = staging_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let reused_source_chunk_count = verified_source
        .as_ref()
        .map(|source| count_reusable_source_chunks(target_manifest, source.target_manifest))
        .unwrap_or(0);

    let (_, reconstruct_timings) = reconstruct_from_bundle_paths_and_verified_source_with_timings(
        target_manifest,
        bundle_paths,
        verified_source,
        staging_dir,
    )?;

    let (verify_errors, verify_timings) =
        verify_directory_with_timings(target_manifest, staging_dir)?;
    if !verify_errors.is_empty() {
        return Err(Error::verify(format!(
            "staged install did not verify: {}",
            verify_errors.join("; ")
        )));
    }

    let backup_dir = promote_staging_install(install_dir, staging_dir, target_manifest)?;

    Ok(TargetInstallApplyResult {
        target_id: target_manifest.target_id.clone(),
        install_dir: install_dir.to_path_buf(),
        staging_dir: staging_dir.to_path_buf(),
        backup_dir,
        reused_source_chunk_count,
        required_chunk_count: derive_chunk_count(target_manifest),
        required_bundle_count: 0,
        reconstruct_timings,
        verify_timings,
    })
}

pub fn fetch_target_install_content(
    request: &TargetInstallFetchRequest,
) -> Result<TargetInstallFetchResult> {
    let repo_paths = crate::ContentRepoPaths::from_repo_root(&request.repo_path);
    let target_root = repo_paths.targets_root.join(request.target_id.trim());

    std::fs::create_dir_all(&target_root)?;
    std::fs::create_dir_all(&repo_paths.bundles_root)?;

    let manifest_bytes = download_url(request.manifest_url.trim())?;
    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes).map_err(|err| {
        Error::manifest(format!(
            "downloaded target manifest is invalid JSON: {}",
            err
        ))
    })?;
    write_repo_manifest(&request.repo_path, &manifest)?;

    let mut wrote_manifest_signature = false;
    if let Some(sig_url) = request.manifest_sig_url.as_deref().map(str::trim) {
        if !sig_url.is_empty() {
            let sig_bytes = download_url(sig_url)?;
            std::fs::write(target_root.join("manifest.sig"), sig_bytes)?;
            wrote_manifest_signature = true;
        }
    }

    let bundle_ids_from_chunk_index: Vec<String> = {
        let chunk_index_path = repo_paths.chunk_index_path.clone();
        let chunk_index = if chunk_index_path.exists() {
            let json = std::fs::read_to_string(&chunk_index_path).ok();
            json.and_then(|data| serde_json::from_str::<manifest::ChunkIndex>(&data).ok())
        } else {
            None
        };

        if let Some(index) = chunk_index {
            let target: manifest::TargetManifest = serde_json::from_slice(&manifest_bytes)
                .map_err(|err| Error::manifest(format!("invalid target manifest: {}", err)))?;
            let mut bundle_set = HashSet::new();
            for file in &target.files {
                for chunk in &file.chunks {
                    if let Some(bundle_id) = index.entries.get(&chunk.chunk_hash) {
                        bundle_set.insert(bundle_id.clone());
                    }
                }
            }
            bundle_set.into_iter().collect()
        } else {
            Vec::new()
        }
    };
    std::fs::write(target_root.join("manifest.json"), &manifest_bytes)?;
    let bundles = if request.bundles.is_empty() && !bundle_ids_from_chunk_index.is_empty() {
        fetch_bundle_urls(request, &bundle_ids_from_chunk_index)?
    } else {
        request.bundles.clone()
    };

    let mut downloaded_bundle_count = 0usize;
    let mut skipped_existing_bundle_count = 0usize;
    for bundle in &bundles {
        let bundle_id = bundle.bundle_id.trim();
        if bundle_id.is_empty() {
            continue;
        }

        let bundle_path = repo_paths
            .bundles_root
            .join(format!("{}.bundle", bundle_id));
        if bundle_path.exists() {
            skipped_existing_bundle_count += 1;
            continue;
        }

        let bundle_bytes = download_url(bundle.url.trim())?;
        std::fs::write(&bundle_path, bundle_bytes)?;
        downloaded_bundle_count += 1;
    }

    Ok(TargetInstallFetchResult {
        target_id: request.target_id.clone(),
        repo_path: request.repo_path.clone(),
        required_bundle_count: bundle_ids_from_chunk_index.len(),
        downloaded_bundle_count,
        skipped_existing_bundle_count,
        wrote_manifest_signature,
    })
}

fn collect_chunk_hashes(manifest: &TargetManifest) -> HashSet<String> {
    let mut hashes = HashSet::new();
    for file in &manifest.files {
        for chunk in &file.chunks {
            hashes.insert(chunk.chunk_hash.clone());
        }
    }
    hashes
}

fn count_reusable_source_chunks(
    target_manifest: &TargetManifest,
    source_manifest: &TargetManifest,
) -> usize {
    let target_chunks = collect_chunk_hashes(target_manifest);
    let source_chunks = collect_chunk_hashes(source_manifest);
    source_chunks
        .iter()
        .filter(|chunk_hash| target_chunks.contains(chunk_hash.as_str()))
        .count()
}

fn promote_staging_install(
    install_dir: &Path,
    staging_dir: &Path,
    target_manifest: &TargetManifest,
) -> Result<Option<PathBuf>> {
    if let Some(parent) = install_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let backup_dir = if install_dir.exists() {
        let backup_dir = backup_dir_for_install(install_dir, &target_manifest.target_id);
        if backup_dir.exists() {
            std::fs::remove_dir_all(&backup_dir)?;
        }
        std::fs::rename(install_dir, &backup_dir)?;
        Some(backup_dir)
    } else {
        None
    };

    if let Err(err) = std::fs::rename(staging_dir, install_dir) {
        if let Some(backup_dir) = &backup_dir {
            let _ = std::fs::rename(backup_dir, install_dir);
        }
        return Err(Error::Io(err));
    }

    if let Some(backup_dir) = &backup_dir {
        std::fs::remove_dir_all(backup_dir)?;
    }

    Ok(backup_dir)
}

fn backup_dir_for_install(install_dir: &Path, target_id: &str) -> PathBuf {
    let install_name = install_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("install");
    let backup_name = format!(
        ".gutch-backup-{}-{}-{}",
        install_name,
        target_id,
        std::process::id()
    );
    install_dir.with_file_name(backup_name)
}

fn download_url(url: &str) -> Result<Vec<u8>> {
    if url.is_empty() {
        return Err(Error::bundle("download URL is required"));
    }

    let response = reqwest::blocking::get(url)
        .map_err(|err| Error::bundle(format!("failed to download Gutch content: {}", err)))?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::bundle(format!(
            "Gutch content download failed with status {}.",
            status
        )));
    }

    response
        .bytes()
        .map(|bytes| bytes.to_vec())
        .map_err(|err| Error::bundle(format!("failed to read Gutch download body: {}", err)))
}

fn fetch_bundle_urls(
    request: &TargetInstallFetchRequest,
    bundle_ids: &[String],
) -> Result<Vec<GutchBundleDownload>> {
    let endpoint = request
        .bundle_urls_endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::bundle("bundle URL endpoint is required for Gutch install"))?;
    let separator = if endpoint.contains('?') { '&' } else { '?' };
    let url = format!("{}{}ids={}", endpoint, separator, bundle_ids.join(","));
    let mut builder = reqwest::blocking::Client::new().get(url);
    if let Some(session_id) = request.session_id.as_deref().map(str::trim) {
        if !session_id.is_empty() {
            builder = builder.header("Authorization", format!("Bearer {}", session_id));
        }
    }

    let response = builder
        .send()
        .map_err(|err| Error::bundle(format!("failed to request Gutch bundle URLs: {}", err)))?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::bundle(format!(
            "Gutch bundle URL request failed with status {}.",
            status
        )));
    }

    response
        .json::<GutchBundleUrlsResponse>()
        .map(|body| body.bundles)
        .map_err(|err| Error::bundle(format!("failed to parse Gutch bundle URLs: {}", err)))
}

fn write_repo_manifest(repo_path: &Path, target_manifest: &serde_json::Value) -> Result<()> {
    let product_id = target_manifest
        .get("product_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown-product");
    let content_repo_id = target_manifest
        .get("content_repo_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("base");
    let mut repo_manifest =
        manifest::ContentRepoManifest::new(product_id.to_string(), content_repo_id.to_string());
    if let Some(created_at) = target_manifest
        .get("created_at")
        .and_then(serde_json::Value::as_str)
    {
        repo_manifest.created_at = created_at.to_string();
    }

    let repo_json = serde_json::to_vec_pretty(&repo_manifest)?;
    std::fs::write(
        repo_path.join(manifest::CONTENT_REPO_MANIFEST_FILE),
        repo_json,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
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
            format_version: 1,
            updated_at: String::new(),
            entries: std::collections::BTreeMap::from([
                ("chunk-a".to_string(), "bundle-a".to_string()),
            ]),
        };
        std::fs::write(
            repo_path.join("chunk_index.json"),
            serde_json::to_string_pretty(&chunk_index).unwrap(),
        ).unwrap();

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
                            "product_id": "product-1",
                            "content_repo_id": "base",
                            "target_id": "1.0.0-windows-x86_64-base",
                            "version_id": "1.0.0",
                            "platform": "windows",
                            "architecture": "x86_64",
                            "tier_id": "base",
                            "created_at": "2026-05-11T00:00:00Z",
                            "files": [
                                {
                                    "path": "game.exe",
                                    "size": 100,
                                    "mode": 33204,
                                    "executable": true,
                                    "symlink_target": null,
                                    "full_file_hash": "abc",
                                    "chunks": [
                                        { "chunk_hash": "chunk-a", "raw_size": 100 }
                                    ]
                                }
                            ],
                            "target_tags": {"version": "1.0.0"},
                            "manifest_version": 1,
                            "chunk_sizing_profile": manifest::ChunkSizingProfile::default(),
                            "compression_profile": manifest::CompressionProfile::default(),
                            "bundle_sizing_profile": manifest::BundleSizingProfile::default(),
                            "bundle_format_version": manifest::BUNDLE_FORMAT,
                            "total_logical_size": 0,
                            "total_compressed_size_estimate": 0,
                            "signature_metadata": serde_json::Value::Null
                        }))
                        .unwrap(),
                        "application/json",
                    ),
                    "/manifest.sig" => ("200 OK", b"sig".to_vec(), "application/octet-stream"),
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
            manifest_sig_url: Some(format!("http://{}/manifest.sig", address)),
            bundle_urls_endpoint: Some(format!("http://{}/bundles", address)),
            session_id: Some("session-1".to_string()),
            bundles: Vec::new(),
        })
        .unwrap();

        assert_eq!(result.required_bundle_count, 1);
        assert_eq!(result.downloaded_bundle_count, 1);
        assert!(result.wrote_manifest_signature);
        assert!(repo_path.join("repo_manifest.json").exists() || repo_path.join("repo.json").exists());
        assert!(
            repo_path
                .join("targets")
                .join("1.0.0-windows-x86_64-base")
                .join("manifest.json")
                .exists()
        );
        assert!(
            repo_path
                .join("targets")
                .join("1.0.0-windows-x86_64-base")
                .join("manifest.sig")
                .exists()
        );
        assert!(repo_path.join("bundles").join("bundle-a.bundle").exists());

        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }
}








