use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::Serialize;
use tempfile::TempDir;

use crate::AuthCommands;
use crate::RepoInitSpec;
use crate::cli_path_compare::compare_paths;
use crate::cli_reporting::{
    BuildTimingsSummary, OptionalOutputPaths, ReconstructTimingsSummary, VerifyTimingsSummary,
    accumulate_build_timings, build_timings_summary, ensure_parent_directory, format_bytes,
    optional_output_paths, print_build_timings, print_reconstruct_timings,
    print_repo_init_storage_summary, print_verify_timings, ratio_pct, reconstruct_timings_summary,
    verify_timings_summary, write_stats_json_output,
};
use crate::{
    BundleCommands, Cli, Commands, PathCommands, RepoCommands, RepoInitMode, TargetCommands,
    TargetInstallCommands,
};
use g::cli_identity::{apply_resolved_target_identity, parse_cli_tags, resolve_target_identity};
use gutch as g;

struct InputSizingStats {
    target_logical_size_bytes: u64,
    median_file_size_bytes: u64,
}

struct PreparedInputPath {
    dir: PathBuf,
    _tempdir: Option<TempDir>,
}

#[derive(Debug, Serialize)]
struct TargetInsertStats {
    command: &'static str,
    repo_root: String,
    input_path: String,
    target_id: String,
    manifest_path: String,
    product_id: String,
    content_repo_id: String,
    version_id: String,
    platform: String,
    architecture: String,
    tier_id: String,
    file_count: usize,
    total_logical_size: u64,
    total_compressed_size_estimate: u64,
    required_chunk_count: usize,
    required_bundle_count: usize,
    generated_bundle_count: usize,
    generated_bundle_bytes: u64,
    reused_chunk_count: usize,
    new_chunk_count: usize,
    reused_chunk_percent: f64,
    reused_bundle_count: usize,
    reused_bundle_percent: f64,
    timings: BuildTimingsSummary,
    outputs: OptionalOutputPaths,
}

#[derive(Debug, Serialize)]
struct TargetExportStats {
    command: &'static str,
    repo_root: String,
    target_id: String,
    output_dir: String,
    file_count: usize,
    required_chunk_count: usize,
    required_bundle_count: usize,
    resolved_bundle_file_count: usize,
    total_logical_size: u64,
    total_compressed_size_estimate: u64,
    timings: ReconstructTimingsSummary,
    outputs: OptionalOutputPaths,
}

#[derive(Debug, Serialize)]
struct TargetInstallVerifyStats {
    command: &'static str,
    repo_root: String,
    target_id: String,
    install_dir: String,
    expected_file_count: usize,
    missing_file_count: usize,
    corrupt_file_count: usize,
    extra_file_count: usize,
    verification_passed: bool,
    errors: Vec<String>,
    timings: VerifyTimingsSummary,
    outputs: OptionalOutputPaths,
}

#[derive(Debug, Serialize)]
struct TargetInstallApplyStats {
    command: &'static str,
    repo_root: String,
    target_id: String,
    install_dir: String,
    staging_dir: String,
    source_target_id: Option<String>,
    source_install_dir: Option<String>,
    file_count: usize,
    required_chunk_count: usize,
    required_bundle_count: usize,
    resolved_bundle_file_count: usize,
    reused_source_chunk_count: usize,
    reused_source_chunk_percent: f64,
    total_logical_size: u64,
    total_compressed_size_estimate: u64,
    reconstruct_timings: ReconstructTimingsSummary,
    verify_timings: VerifyTimingsSummary,
    outputs: OptionalOutputPaths,
}

#[derive(Debug, Serialize)]
struct RepoSyncUpStats {
    command: &'static str,
    repo_root: String,
    product_id: i64,
    content_repo_id: i64,
    repo_manifest_uploaded: bool,
    chunk_index_uploaded: bool,
    target_manifest_uploaded_count: usize,
    bundle_uploaded_count: usize,
    bundle_skipped_count: usize,
    target_count: usize,
    bundle_count: usize,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatorRepoTargetsResponse {
    tiers: Vec<CreatorTierResponse>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatorTierResponse {
    id: i64,
    slug: String,
    tier_name: String,
    targets: Vec<CreatorTargetResponse>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatorTargetResponse {
    id: i64,
    target_id: String,
    repo_id: Option<i64>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignedRepoManifestResponse {
    manifest_url: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignedChunkIndexResponse {
    chunk_index_url: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignedTargetManifestResponse {
    manifest_url: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpsertTargetResponse {
    id: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_reporting::collect_repo_init_target_added_bytes;

    #[test]
    fn collect_repo_init_target_added_bytes_counts_only_new_bundles_once() {
        let mut target_a = g::manifest::TargetManifest::new(
            "prod".into(),
            "base".into(),
            "1.0.0-windows-x86_64-base".into(),
            g::manifest::TargetTagSet::default(),
        );
        target_a.total_logical_size = 1000;
        let mut target_b = g::manifest::TargetManifest::new(
            "prod".into(),
            "base".into(),
            "1.1.0-windows-x86_64-base".into(),
            g::manifest::TargetTagSet::default(),
        );
        target_b.total_logical_size = 2000;

        let target_manifest_sizes = HashMap::from([
            (target_a.target_id.clone(), 100u64),
            (target_b.target_id.clone(), 200u64),
        ]);
        let bundle_sizes = HashMap::from([
            ("bundle-a".to_string(), 1000u64),
            ("bundle-b".to_string(), 2000u64),
            ("bundle-c".to_string(), 3000u64),
        ]);

        let added_bytes = collect_repo_init_target_added_bytes(
            &[&target_a, &target_b],
            &target_manifest_sizes,
            &bundle_sizes,
        );

        assert_eq!(added_bytes[0].0.target_id, target_a.target_id);
        assert_eq!(added_bytes[0].1, 100);
        assert_eq!(added_bytes[1].0.target_id, target_b.target_id);
        assert_eq!(added_bytes[1].1, 200);
    }
}

fn write_optional_summary(summary_out: &Option<PathBuf>, lines: &[String]) -> g::Result<()> {
    let Some(summary_out) = summary_out else {
        return Ok(());
    };

    ensure_parent_directory(summary_out)?;
    std::fs::write(summary_out, lines.join("\n"))?;
    Ok(())
}

fn median_u64(sorted_values: &[u64]) -> u64 {
    if sorted_values.is_empty() {
        return 0;
    }

    let mid = sorted_values.len() / 2;
    if sorted_values.len() % 2 == 1 {
        sorted_values[mid]
    } else {
        (sorted_values[mid - 1] + sorted_values[mid]) / 2
    }
}

fn collect_input_sizing_stats(dir: &PathBuf) -> g::Result<InputSizingStats> {
    let mut sizes = Vec::new();
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry?;
        if entry.path().is_file() {
            sizes.push(entry.metadata()?.len());
        }
    }

    sizes.sort_unstable();
    let target_logical_size_bytes = sizes.iter().copied().sum();
    Ok(InputSizingStats {
        target_logical_size_bytes,
        median_file_size_bytes: median_u64(&sizes),
    })
}

fn prepare_input_path(path: &PathBuf) -> g::Result<PreparedInputPath> {
    if path.is_dir() {
        return Ok(PreparedInputPath {
            dir: path.clone(),
            _tempdir: None,
        });
    }

    if path.is_file() {
        let tempdir = tempfile::Builder::new().prefix("gutch_input_").tempdir()?;
        let filename = path.file_name().ok_or_else(|| {
            g::Error::manifest(format!("input file has no name: {}", path.display()))
        })?;
        let staged_path = tempdir.path().join(filename);
        std::fs::copy(path, &staged_path)?;
        return Ok(PreparedInputPath {
            dir: tempdir.path().to_path_buf(),
            _tempdir: Some(tempdir),
        });
    }

    Err(g::Error::manifest(format!(
        "input path does not exist or is unsupported: {}",
        path.display()
    )))
}

fn directory_has_any_entries(path: &std::path::Path) -> g::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    Ok(std::fs::read_dir(path)?.next().is_some())
}

fn load_optional_rules(
    rules_path: &Option<PathBuf>,
) -> g::Result<Option<g::manifest::AutoTaggingRules>> {
    let Some(rules_path) = rules_path else {
        return Ok(None);
    };

    let rules_json = std::fs::read_to_string(rules_path)?;
    let rules: g::manifest::AutoTaggingRules = serde_json::from_str(&rules_json)?;
    Ok(Some(rules))
}

fn load_target_manifest_from_repo(
    repo_root: &std::path::Path,
    target_id: &str,
) -> g::Result<(
    g::ContentRepoPaths,
    g::manifest::ContentRepoManifest,
    PathBuf,
    g::manifest::TargetManifest,
)> {
    let (repo_paths, repo_manifest) = g::load_content_repo_manifest(repo_root)?;
    let manifest_path = g::target_manifest_path(&repo_paths, target_id);
    if !manifest_path.exists() {
        return Err(g::Error::manifest(format!(
            "target manifest not found at {}",
            manifest_path.display()
        )));
    }

    let manifest_json = std::fs::read_to_string(&manifest_path)?;
    let target_manifest: g::manifest::TargetManifest = serde_json::from_str(&manifest_json)?;
    Ok((repo_paths, repo_manifest, manifest_path, target_manifest))
}

fn resolve_required_bundle_paths(
    bundles_root: &std::path::Path,
    _target_manifest: &g::manifest::TargetManifest,
) -> g::Result<Vec<PathBuf>> {
    if bundles_root.is_file() {
        return Ok(vec![bundles_root.to_path_buf()]);
    }

    if !bundles_root.is_dir() {
        return Ok(Vec::new());
    }

    let mut paths = Vec::new();
    for entry in walkdir::WalkDir::new(bundles_root).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file() {
            continue;
        }
        if !entry.path().extension().is_some_and(|e| e == "bundle") {
            continue;
        }
        paths.push(entry.path().to_path_buf());
    }

    Ok(paths)
}

fn default_staging_dir(install_dir: &std::path::Path, target_id: &str) -> PathBuf {
    let install_name = install_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("install");
    install_dir.with_file_name(format!(".gutch-staging-{}-{}", install_name, target_id))
}

fn parse_bundle_downloads(bundle_args: &[String]) -> g::Result<Vec<g::GutchBundleDownload>> {
    bundle_args
        .iter()
        .map(|bundle_arg| {
            let Some((bundle_id, url)) = bundle_arg.split_once('=') else {
                return Err(g::Error::manifest(format!(
                    "invalid --bundle value '{}'; use <bundle-id>=<url>",
                    bundle_arg
                )));
            };
            let bundle_id = bundle_id.trim();
            let url = url.trim();
            if bundle_id.is_empty() || url.is_empty() {
                return Err(g::Error::manifest(format!(
                    "invalid --bundle value '{}'; use <bundle-id>=<url>",
                    bundle_arg
                )));
            }

            Ok(g::GutchBundleDownload {
                bundle_id: bundle_id.to_string(),
                url: url.to_string(),
            })
        })
        .collect()
}

fn session_id_for_cli(session_override: Option<String>) -> g::Result<String> {
    session_override
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or(crate::auth::shared_session_id().map_err(g::Error::manifest)?)
        .ok_or_else(|| {
            g::Error::manifest(
                "authentication required. Run `gutch auth login` or pass --session-id.",
            )
        })
}

fn parse_version_parts(version: &str) -> g::Result<(i32, i32, i32)> {
    let normalized = version
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V');
    let parts = normalized.split('.').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(g::Error::manifest(format!(
            "expected semantic version like 1.2.3, got '{}'",
            version
        )));
    }

    let major = parts[0]
        .parse::<i32>()
        .map_err(|_| g::Error::manifest(format!("invalid version '{}'", version)))?;
    let minor = parts[1]
        .parse::<i32>()
        .map_err(|_| g::Error::manifest(format!("invalid version '{}'", version)))?;
    let patch = parts[2]
        .parse::<i32>()
        .map_err(|_| g::Error::manifest(format!("invalid version '{}'", version)))?;
    Ok((major, minor, patch))
}

fn request_json_from_backend<T: for<'de> serde::Deserialize<'de>>(
    request: reqwest::blocking::RequestBuilder,
) -> g::Result<T> {
    let response = request
        .send()
        .map_err(|err| g::Error::manifest(format!("backend request failed: {}", err)))?;
    if !response.status().is_success() {
        return Err(g::Error::manifest(parse_backend_error_response(response)));
    }
    response
        .json::<T>()
        .map_err(|err| g::Error::manifest(format!("failed to decode backend response: {}", err)))
}

fn request_optional_bytes_from_signed_url(
    client: &reqwest::blocking::Client,
    url: &str,
) -> g::Result<Option<Vec<u8>>> {
    let response = client
        .get(url)
        .send()
        .map_err(|err| g::Error::manifest(format!("signed url request failed: {}", err)))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(g::Error::manifest(parse_backend_error_response(response)));
    }
    response
        .bytes()
        .map(|bytes| Some(bytes.to_vec()))
        .map_err(|err| g::Error::manifest(format!("failed to read signed url response: {}", err)))
}

fn parse_backend_error_response(response: reqwest::blocking::Response) -> String {
    let status = response.status();
    let text = response.text().unwrap_or_default();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
        if let Some(message) = value
            .get("message")
            .and_then(|value| value.as_str())
            .or_else(|| value.get("error").and_then(|value| value.as_str()))
        {
            let trimmed = message.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    if text.trim().is_empty() {
        return format!("request failed with status {}", status);
    }
    format!("request failed with status {}", status)
}

fn slugify_like_backend(value: &str) -> String {
    let trimmed = value.trim().to_ascii_lowercase();
    let mut slug = String::new();
    let mut last_hyphen = false;
    for ch in trimmed.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_hyphen = false;
        } else if !last_hyphen {
            slug.push('-');
            last_hyphen = true;
        }
    }
    slug.trim_matches('-').to_string()
}

fn upload_raw_file(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    path: &str,
    session_id: &str,
    bytes: Vec<u8>,
    content_type: &str,
) -> g::Result<()> {
    let response = client
        .put(format!("{}{}", endpoint.trim_end_matches('/'), path))
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {}", session_id))
        .header("Content-Type", content_type)
        .body(bytes)
        .send()
        .map_err(|err| g::Error::manifest(format!("upload request failed: {}", err)))?;
    if !response.status().is_success() {
        return Err(g::Error::manifest(parse_backend_error_response(response)));
    }
    Ok(())
}

fn run_repo_sync_up(
    repo: PathBuf,
    endpoint: String,
    product_id: i64,
    content_repo_id: i64,
    session_override: Option<String>,
) -> g::Result<RepoSyncUpStats> {
    let session_id = session_id_for_cli(session_override)?;
    let client = reqwest::blocking::Client::builder()
        .user_agent("Gutch CLI")
        .build()
        .map_err(|err| g::Error::manifest(format!("failed to create sync client: {}", err)))?;

    let status = g::inspect_content_repo(&repo)?;
    if !status.is_repo {
        return Err(g::Error::manifest(format!(
            "no content repo found at {}",
            repo.display()
        )));
    }
    let (repo_paths, _) = g::load_content_repo_manifest(&repo)?;

    let creator_targets: CreatorRepoTargetsResponse = request_json_from_backend(
        client
            .get(format!(
                "{}/v1/users/me/creator/products/{}/repos/{}/targets",
                endpoint.trim_end_matches('/'),
                product_id,
                content_repo_id
            ))
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", session_id)),
    )?;

    let mut tiers_by_slug = HashMap::new();
    let mut remote_target_ids = HashMap::new();
    for tier in &creator_targets.tiers {
        tiers_by_slug.insert(tier.slug.clone(), tier.id);
        tiers_by_slug.insert(slugify_like_backend(&tier.tier_name), tier.id);
        for target in &tier.targets {
            remote_target_ids.insert(target.target_id.clone(), target.id);
        }
    }

    let repo_manifest_bytes = std::fs::read(&repo_paths.repo_manifest_path)?;
    let signed_repo_manifest: SignedRepoManifestResponse = request_json_from_backend(
        client
            .get(format!(
                "{}/v1/users/me/creator/products/{}/repos/{}/manifest",
                endpoint.trim_end_matches('/'),
                product_id,
                content_repo_id
            ))
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", session_id)),
    )?;
    let remote_repo_manifest_bytes =
        request_optional_bytes_from_signed_url(&client, &signed_repo_manifest.manifest_url)?;
    let repo_manifest_uploaded = remote_repo_manifest_bytes
        .as_deref()
        .map(|bytes| bytes != repo_manifest_bytes.as_slice())
        .unwrap_or(true);
    if repo_manifest_uploaded {
        upload_raw_file(
            &client,
            &endpoint,
            &format!(
                "/v1/users/me/creator/products/{}/repos/{}/manifest",
                product_id, content_repo_id
            ),
            &session_id,
            repo_manifest_bytes,
            "application/json",
        )?;
    }

    let mut chunk_index_uploaded = false;
    if repo_paths.chunk_index_path.exists() {
        let chunk_index_bytes = std::fs::read(&repo_paths.chunk_index_path)?;
        let signed_chunk_index: SignedChunkIndexResponse = request_json_from_backend(
            client
                .get(format!(
                    "{}/v1/users/me/creator/products/{}/repos/{}/chunks",
                    endpoint.trim_end_matches('/'),
                    product_id,
                    content_repo_id
                ))
                .header("Accept", "application/json")
                .header("Authorization", format!("Bearer {}", session_id)),
        )?;
        let remote_chunk_index_bytes =
            request_optional_bytes_from_signed_url(&client, &signed_chunk_index.chunk_index_url)?;
        chunk_index_uploaded = remote_chunk_index_bytes
            .as_deref()
            .map(|bytes| bytes != chunk_index_bytes.as_slice())
            .unwrap_or(true);
        if chunk_index_uploaded {
            upload_raw_file(
                &client,
                &endpoint,
                &format!(
                    "/v1/users/me/creator/products/{}/repos/{}/chunks",
                    product_id, content_repo_id
                ),
                &session_id,
                chunk_index_bytes,
                "application/json",
            )?;
        }
    }

    let mut target_manifest_uploaded_count = 0usize;
    for target in &status.targets {
        let manifest_bytes = std::fs::read(&target.manifest_path)?;
        let manifest: g::manifest::TargetManifest = serde_json::from_slice(&manifest_bytes)?;
        let target_id = manifest.target_id.parse::<i64>().map_err(|_| {
            g::Error::manifest(format!(
                "target '{}' has a non-numeric local target id; repo sync-up expects numeric target ids",
                manifest.target_id
            ))
        })?;
        let tier_key = manifest
            .target_tags
            .get_exclusive("tier")
            .unwrap_or_default()
            .to_string();
        let backend_tier_id = if !tier_key.is_empty() {
            tiers_by_slug.get(&tier_key).copied().ok_or_else(|| {
                g::Error::manifest(format!(
                    "could not match local tier tag '{}' to a backend tier slug for product {}",
                    tier_key, product_id
                ))
            })?
        } else if creator_targets.tiers.len() == 1 {
            creator_targets.tiers[0].id
        } else {
            return Err(g::Error::manifest(format!(
                "target '{}' does not declare a tier tag and product {} has multiple tiers",
                manifest.target_id, product_id
            )));
        };
        let version_tag = manifest
            .target_tags
            .get_exclusive("version")
            .ok_or_else(|| {
                g::Error::manifest(format!(
                    "target '{}' is missing required version tag",
                    manifest.target_id
                ))
            })?;
        let (version_major, version_minor, version_patch) = parse_version_parts(version_tag)?;
        let platform = manifest
            .target_tags
            .get_exclusive("os")
            .unwrap_or("any")
            .to_string();
        let architecture = manifest
            .target_tags
            .get_exclusive("arch")
            .unwrap_or("any")
            .to_string();
        let existing_target_id = remote_target_ids
            .get(&manifest.target_id)
            .copied()
            .unwrap_or(0);
        let upsert_target: UpsertTargetResponse = request_json_from_backend(
            client
                .put(format!(
                    "{}/v1/users/me/creator/products/{}/repos/{}/targets/{}",
                    endpoint.trim_end_matches('/'),
                    product_id,
                    content_repo_id,
                    existing_target_id
                ))
                .header("Accept", "application/json")
                .header("Authorization", format!("Bearer {}", session_id))
                .json(&serde_json::json!({
                    "targetId": target_id,
                    "targetTags": manifest.target_tags.clone(),
                    "tierId": backend_tier_id,
                    "versionMajor": version_major,
                    "versionMinor": version_minor,
                    "versionPatch": version_patch,
                    "platform": platform,
                    "architecture": architecture,
                    "isFullBuild": true
                })),
        )?;
        remote_target_ids.insert(manifest.target_id.clone(), upsert_target.id);

        let signed_target_manifest: SignedTargetManifestResponse = request_json_from_backend(
            client
                .get(format!(
                    "{}/v1/users/me/creator/products/{}/repos/{}/targets/{}/manifest",
                    endpoint.trim_end_matches('/'),
                    product_id,
                    content_repo_id,
                    upsert_target.id
                ))
                .header("Accept", "application/json")
                .header("Authorization", format!("Bearer {}", session_id)),
        )?;
        let remote_manifest_bytes =
            request_optional_bytes_from_signed_url(&client, &signed_target_manifest.manifest_url)?;
        let should_upload_manifest = remote_manifest_bytes
            .as_deref()
            .map(|bytes| bytes != manifest_bytes.as_slice())
            .unwrap_or(true);
        if should_upload_manifest {
            upload_raw_file(
                &client,
                &endpoint,
                &format!(
                    "/v1/users/me/creator/products/{}/repos/{}/targets/{}/manifest",
                    product_id, content_repo_id, upsert_target.id
                ),
                &session_id,
                manifest_bytes,
                "application/json",
            )?;
            target_manifest_uploaded_count += 1;
        }
    }

    let mut bundle_uploaded_count = 0usize;
    let mut bundle_skipped_count = 0usize;
    for entry in walkdir::WalkDir::new(&repo_paths.bundles_root).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file() {
            continue;
        }
        if entry.path().extension().and_then(|value| value.to_str()) != Some("bundle") {
            continue;
        }
        let Some(bundle_id) = entry.path().file_stem().and_then(|value| value.to_str()) else {
            continue;
        };

        let exists_response = client
            .get(format!(
                "{}/v1/users/me/creator/products/{}/repos/{}/bundles/{}",
                endpoint.trim_end_matches('/'),
                product_id,
                content_repo_id,
                bundle_id
            ))
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", session_id))
            .send()
            .map_err(|err| g::Error::manifest(format!("bundle lookup failed: {}", err)))?;
        if exists_response.status().is_success() {
            bundle_skipped_count += 1;
            continue;
        }
        if exists_response.status() != reqwest::StatusCode::NOT_FOUND {
            return Err(g::Error::manifest(parse_backend_error_response(
                exists_response,
            )));
        }

        upload_raw_file(
            &client,
            &endpoint,
            &format!(
                "/v1/users/me/creator/products/{}/repos/{}/bundles/{}/content",
                product_id, content_repo_id, bundle_id
            ),
            &session_id,
            std::fs::read(entry.path())?,
            "application/octet-stream",
        )?;
        bundle_uploaded_count += 1;
    }

    Ok(RepoSyncUpStats {
        command: "repo sync-up",
        repo_root: repo.display().to_string(),
        product_id,
        content_repo_id,
        repo_manifest_uploaded,
        chunk_index_uploaded,
        target_manifest_uploaded_count,
        bundle_uploaded_count,
        bundle_skipped_count,
        target_count: status.targets.len(),
        bundle_count: status.bundle_count,
    })
}

pub(crate) fn run(cli: Cli) -> g::Result<()> {
    match cli.command {
        Commands::Auth {
            command: AuthCommands::Login { endpoint, no_open },
        } => {
            let result = crate::auth::login(&endpoint, !no_open).map_err(g::Error::manifest)?;
            if let Some(username) = result.username.as_deref() {
                println!("authenticated as {}", username);
            } else if let Some(email) = result.email.as_deref() {
                println!("authenticated as {}", email);
            } else {
                println!("authentication complete");
            }
        }

        Commands::Auth {
            command:
                AuthCommands::Logout {
                    endpoint,
                    session_id,
                },
        } => {
            crate::auth::logout(&endpoint, session_id.as_deref()).map_err(g::Error::manifest)?;
            println!("logged out");
        }

        Commands::Auth {
            command:
                AuthCommands::Status {
                    endpoint,
                    session_id,
                },
        } => match crate::auth::session_status(&endpoint, session_id.as_deref())
            .map_err(g::Error::manifest)?
        {
            Some(status) => println!(
                "authenticated user {} via {} (expires {})",
                status.user_id, status.login_provider, status.expires_at
            ),
            None => println!("not authenticated"),
        },

        Commands::Repo {
            command:
                RepoCommands::Create {
                    product,
                    content_repo,
                    schema,
                    repo,
                },
        } => {
            let repo_paths = g::ContentRepoPaths::from_repo_root(&repo);
            let mut repo_manifest =
                g::ensure_content_repo_layout(&repo_paths, &product, &content_repo)?;
            repo_manifest.target_schema = load_optional_rules(&schema)?;
            g::write_content_repo_manifest(&repo_paths, &repo_manifest)?;

            println!(
                "created content repo {} for product {} at {}",
                content_repo,
                product,
                repo_paths.repo_root.display()
            );
        }

        Commands::Repo {
            command:
                RepoCommands::SchemaSet {
                    repo,
                    schema,
                    clear,
                },
        } => {
            if clear && schema.is_some() {
                return Err(g::Error::manifest(
                    "use either --schema <path> or --clear, not both",
                ));
            }

            let (repo_paths, mut repo_manifest) = g::load_content_repo_manifest(&repo)?;
            repo_manifest.target_schema = if clear {
                None
            } else {
                load_optional_rules(&schema)?
            };
            g::write_content_repo_manifest(&repo_paths, &repo_manifest)?;

            println!(
                "updated repo schema at {}",
                repo_paths.repo_manifest_path.display()
            );
        }

        Commands::Repo {
            command:
                RepoCommands::Init {
                    product,
                    content_repo,
                    spec,
                    init_mode,
                    chunk_profile,
                    bundle_profile,
                    compression_unit,
                    repo,
                },
        } => {
            let spec_json = std::fs::read_to_string(&spec)?;
            let spec: RepoInitSpec = serde_json::from_str(&spec_json)?;
            if spec.targets.is_empty() {
                return Err(g::Error::manifest(
                    "repo init spec must include at least one target",
                ));
            }

            let rules = spec.target_schema.clone().unwrap_or_default();

            let repo_paths = g::ContentRepoPaths::from_repo_root(&repo);

            if directory_has_any_entries(&repo_paths.targets_root)?
                || directory_has_any_entries(&repo_paths.bundles_root)?
            {
                return Err(g::Error::manifest(format!(
                    "content repo already exists at {}",
                    repo_paths.repo_root.display()
                )));
            }

            let mut repo_manifest =
                g::ensure_content_repo_layout(&repo_paths, &product, &content_repo)?;
            repo_manifest.target_schema = Some(rules.clone());

            let mut target_inputs: Vec<g::RepoInitTargetInput> = spec
                .targets
                .iter()
                .map(|target| {
                    let context = format!("repo init target '{}'", target.dir.display());
                    let resolved =
                        resolve_target_identity(&target.dir, &target.tags, &rules, &context)?;
                    Ok(g::RepoInitTargetInput {
                        dir: target.dir.clone(),
                        target_id: String::new(),
                        target_tags: resolved.target_tags,
                    })
                })
                .collect::<g::Result<Vec<_>>>()?;

            for target_input in &mut target_inputs {
                target_input.target_id =
                    g::reserve_next_target_id(&repo_paths, &mut repo_manifest)?;
            }

            g::write_content_repo_manifest(&repo_paths, &repo_manifest)?;

            let mut all_file_sizes = Vec::new();
            let mut all_target_logical_size = 0u64;
            for target in &target_inputs {
                let stats = collect_input_sizing_stats(&target.dir)?;
                all_target_logical_size += stats.target_logical_size_bytes;
                if stats.median_file_size_bytes > 0 {
                    all_file_sizes.push(stats.median_file_size_bytes);
                }
            }
            all_file_sizes.sort_unstable();
            let batch_median_file_size = median_u64(&all_file_sizes);

            let chunk_sizing_profile = g::manifest::ChunkSizingProfile::for_selection(
                chunk_profile,
                batch_median_file_size,
            );
            let bundle_sizing_profile = g::manifest::BundleSizingProfile::for_selection(
                bundle_profile,
                all_target_logical_size,
            );
            let compression_profile = g::manifest::CompressionProfile {
                compression: g::manifest::COMPRESSION_ZSTD.to_string(),
                compression_unit,
                zstd_level: 3,
            };

            match init_mode {
                RepoInitMode::Batch => {
                    let (mut result, timings) = g::build_repo_init_batch_with_timings(
                        &product,
                        &content_repo,
                        &target_inputs,
                        &chunk_sizing_profile,
                        &bundle_sizing_profile,
                        &compression_profile,
                    )?;
                    let mut target_manifest_sizes = HashMap::new();
                    let mut bundle_sizes = HashMap::new();

                    for manifest in &mut result.target_manifests {
                        let target_root = repo_paths.targets_root.join(&manifest.target_id);
                        std::fs::create_dir_all(&target_root)?;
                        let manifest_path = target_root.join("manifest.json");
                        let manifest_json = serde_json::to_string_pretty(&*manifest)?;
                        std::fs::write(&manifest_path, &manifest_json)?;
                        target_manifest_sizes.insert(
                            manifest.target_id.clone(),
                            std::fs::metadata(&manifest_path)?.len(),
                        );
                        println!("wrote target manifest: {}", manifest_path.display());
                    }

                    for bundle in &result.bundles {
                        let bundle_path = repo_paths
                            .bundles_root
                            .join(format!("{}.bundle", bundle.bundle_id));
                        std::fs::write(&bundle_path, &bundle.data)?;
                        bundle_sizes.insert(bundle.bundle_id.clone(), bundle.data.len() as u64);
                        println!(
                            "wrote bundle: {} ({} bytes)",
                            bundle_path.display(),
                            bundle.data.len()
                        );
                    }

                    println!(
                        "initialized content repo {} with {} targets using batch mode",
                        content_repo,
                        result.target_manifests.len()
                    );
                    let ordered_target_manifests: Vec<&g::manifest::TargetManifest> =
                        result.target_manifests.iter().collect();
                    print_repo_init_storage_summary(
                        &ordered_target_manifests,
                        &repo_paths.repo_root,
                        &target_manifest_sizes,
                        &bundle_sizes,
                    )?;
                    print_build_timings(&timings);
                }
                RepoInitMode::Sequential => {
                    let mut existing_bundle_ids = HashSet::new();
                    let mut existing_chunk_locations = HashMap::new();
                    let mut previous_file_manifests = HashMap::new();
                    let mut combined_timings = g::BuildTimings::default();
                    let mut ordered_target_manifests = Vec::new();
                    let mut target_manifest_sizes = HashMap::new();
                    let mut bundle_sizes = HashMap::new();

                    for target in &target_inputs {
                        let (result, timings) =
                            g::build_from_directory_with_timings_against_existing_store_and_manifests_and_profiles(
                                &target.dir,
                                &product,
                                &content_repo,
                                &target.target_id,
                                &target.target_tags,
                                &existing_chunk_locations,
                                &existing_bundle_ids,
                                &previous_file_manifests,
                                &chunk_sizing_profile,
                                &bundle_sizing_profile,
                                &compression_profile,
                            )?;

                        let target_manifest = result.target_manifest.clone();
                        let target_root = repo_paths.targets_root.join(&target_manifest.target_id);
                        std::fs::create_dir_all(&target_root)?;
                        let manifest_path = target_root.join("manifest.json");
                        let manifest_json = serde_json::to_string_pretty(&target_manifest)?;
                        std::fs::write(&manifest_path, &manifest_json)?;
                        target_manifest_sizes.insert(
                            target_manifest.target_id.clone(),
                            std::fs::metadata(&manifest_path)?.len(),
                        );
                        println!("wrote target manifest: {}", manifest_path.display());
                        ordered_target_manifests.push(target_manifest.clone());

                        for file in &target_manifest.files {
                            previous_file_manifests
                                .entry(file.path.clone())
                                .or_insert_with(|| file.clone());
                        }

                        for bundle in &result.bundles {
                            let bundle_path = repo_paths
                                .bundles_root
                                .join(format!("{}.bundle", bundle.bundle_id));
                            std::fs::write(&bundle_path, &bundle.data)?;
                            bundle_sizes.insert(bundle.bundle_id.clone(), bundle.data.len() as u64);
                            println!(
                                "wrote bundle: {} ({} bytes)",
                                bundle_path.display(),
                                bundle.data.len()
                            );
                            existing_bundle_ids.insert(bundle.bundle_id.clone());

                            let reader = g::bundle::read_bundle(&bundle.data)?;
                            for entry in reader.entries() {
                                existing_chunk_locations
                                    .entry(entry.chunk_hash.clone())
                                    .or_insert_with(|| g::ExistingChunkLocation {
                                        bundle_id: bundle.bundle_id.clone(),
                                        raw_size: entry.raw_size,
                                        compressed_size: entry.compressed_size,
                                    });
                            }
                        }

                        accumulate_build_timings(&mut combined_timings, &timings);
                    }

                    println!(
                        "initialized content repo {} with {} targets using sequential mode",
                        content_repo,
                        target_inputs.len()
                    );
                    let ordered_target_manifest_refs: Vec<&g::manifest::TargetManifest> =
                        ordered_target_manifests.iter().collect();
                    print_repo_init_storage_summary(
                        &ordered_target_manifest_refs,
                        &repo_paths.repo_root,
                        &target_manifest_sizes,
                        &bundle_sizes,
                    )?;
                    print_build_timings(&combined_timings);
                }
            }
        }

        Commands::Repo {
            command: RepoCommands::Inspect { repo, json_out },
        } => {
            let status = g::inspect_content_repo(&repo)?;
            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                std::fs::write(json_out, serde_json::to_vec_pretty(&status)?)?;
            }

            if status.is_repo {
                println!(
                    "inspected content repo {} for product {} at {}",
                    status.content_repo_id, status.product_id, status.repo_path
                );
                println!(
                    "targets: {}, bundles: {} ({} bytes)",
                    status.targets.len(),
                    status.bundle_count,
                    status.bundle_bytes
                );
            } else if let Some(error) = status.error.as_deref() {
                println!("inspect status: {}", error);
            } else {
                println!(
                    "inspect status: no content repo found at {}",
                    status.repo_path
                );
            }
        }

        Commands::Repo {
            command:
                RepoCommands::DiscoverSourceTargets {
                    repo,
                    source_targets_root,
                    json_out,
                },
        } => {
            let candidates = g::discover_source_target_installs(&repo, &source_targets_root)?;
            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                std::fs::write(json_out, serde_json::to_vec_pretty(&candidates)?)?;
            }

            println!(
                "discovered {} matching source target folders in {}",
                candidates.len(),
                source_targets_root.display()
            );
        }

        Commands::Repo {
            command: RepoCommands::List { repo, json_out },
        } => {
            let status = g::inspect_content_repo(&repo)?;
            if !status.is_repo {
                return Err(g::Error::manifest(format!(
                    "no content repo found at {}",
                    repo.display()
                )));
            }

            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                std::fs::write(json_out, serde_json::to_vec_pretty(&status.targets)?)?;
            }

            println!(
                "repo {} has {} targets, {} bundles",
                status.content_repo_id,
                status.targets.len(),
                status.bundle_count
            );
            for target in &status.targets {
                println!("  - {}", target.target_id);
            }
        }

        Commands::Repo {
            command: RepoCommands::DeleteTarget { repo, target, yes },
        } => {
            let (repo_paths, _repo_manifest, _, _tm) =
                load_target_manifest_from_repo(&repo, &target)?;
            let target_root = repo_paths.targets_root.join(&target);
            let confirmed = yes;
            if !confirmed {
                eprintln!(
                    "WARNING: this will remove '{}' and may orphan bundles. Use --yes to confirm.",
                    target_root.display()
                );
                return Err(g::Error::manifest("deletion requires --yes flag"));
            }

            // Remove target directory
            if target_root.exists() {
                std::fs::remove_dir_all(&target_root)?;
                println!("removed target: {}", target);
            }

            // Clean up orphaned bundles
            let remaining_targets: HashSet<String> = {
                let mut ids = HashSet::new();
                if repo_paths.targets_root.exists() {
                    for entry in std::fs::read_dir(&repo_paths.targets_root)? {
                        let entry = entry?;
                        if entry.path().is_dir() {
                            ids.insert(entry.file_name().to_string_lossy().to_string());
                        }
                    }
                }
                ids
            };

            let mut orphan_bundles = Vec::new();
            if repo_paths.bundles_root.exists() {
                for entry in walkdir::WalkDir::new(&repo_paths.bundles_root).sort_by_file_name() {
                    let entry = entry?;
                    if !entry.path().is_file() {
                        continue;
                    }
                    if entry.path().extension().and_then(|e| e.to_str()) != Some("bundle") {
                        continue;
                    }
                    if let Some(bundle_id) = entry.path().file_stem().and_then(|s| s.to_str()) {
                        let bundle_still_needed = remaining_targets.iter().any(|target_id| {
                            let manifest_path = repo_paths
                                .targets_root
                                .join(target_id)
                                .join("manifest.json");
                            if !manifest_path.exists() {
                                return false;
                            }
                            if let Ok(content) = std::fs::read_to_string(&manifest_path) {
                                if let Ok(manifest) =
                                    serde_json::from_str::<serde_json::Value>(&content)
                                {
                                    if let Some(bundle_set) = manifest
                                        .get("required_bundle_set")
                                        .and_then(|v| v.as_array())
                                    {
                                        return bundle_set
                                            .iter()
                                            .any(|b| b.as_str() == Some(bundle_id));
                                    }
                                }
                            }
                            false
                        });
                        if !bundle_still_needed {
                            orphan_bundles.push(entry.path().to_path_buf());
                        }
                    }
                }
            }

            for orphan_path in &orphan_bundles {
                std::fs::remove_file(orphan_path)?;
                println!("removed orphaned bundle: {}", orphan_path.display());
            }

            println!(
                "deleted target '{}': removed dir, {} orphaned bundles cleaned up",
                target,
                orphan_bundles.len()
            );
        }

        Commands::Repo {
            command:
                RepoCommands::SyncUp {
                    repo,
                    endpoint,
                    product_id,
                    content_repo_id,
                    session_id,
                },
        } => {
            let stats = run_repo_sync_up(repo, endpoint, product_id, content_repo_id, session_id)?;
            println!(
                "synced repo {} to product {} repo {}",
                stats.repo_root, stats.product_id, stats.content_repo_id
            );
            println!(
                "repo manifest uploaded: {}, chunk index uploaded: {}",
                stats.repo_manifest_uploaded, stats.chunk_index_uploaded
            );
            println!(
                "target manifests uploaded: {}, bundles uploaded: {}, bundles skipped: {}",
                stats.target_manifest_uploaded_count,
                stats.bundle_uploaded_count,
                stats.bundle_skipped_count
            );
        }

        Commands::Target {
            command:
                TargetCommands::Insert {
                    dir,
                    repo,
                    version,
                    platform,
                    architecture,
                    tier,
                    tags,
                    chunk_profile,
                    bundle_profile,
                    compression_unit,
                    summary_out,
                    stats_json_out,
                },
        } => {
            let prepared_input = prepare_input_path(&dir)?;
            let (repo_paths, mut repo_manifest) = g::load_content_repo_manifest(&repo)?;
            let rules = repo_manifest.target_schema.clone().unwrap_or_default();
            let mut explicit_tags = parse_cli_tags(&tags)?;
            if let Some(version) = version {
                explicit_tags.upsert_exclusive("version", version);
            }
            if let Some(platform) = platform {
                explicit_tags.upsert_exclusive("os", platform);
            }
            if let Some(architecture) = architecture {
                explicit_tags.upsert_exclusive("arch", architecture);
            }
            if let Some(tier) = tier {
                explicit_tags.upsert_exclusive("tier", tier);
            }
            let context = format!("target insert input '{}'", dir.display());
            let resolved_identity =
                resolve_target_identity(&dir, &explicit_tags, &rules, &context)?;
            let input_stats = collect_input_sizing_stats(&prepared_input.dir)?;
            let product = repo_manifest.product_id.clone();
            let content_repo = repo_manifest.content_repo_id.clone();
            let target_id = g::reserve_next_target_id(&repo_paths, &mut repo_manifest)?;

            let target_root = repo_paths.targets_root.join(&target_id);
            let bundles_root = repo_paths.bundles_root.clone();

            std::fs::create_dir_all(&target_root)?;

            let mut existing_bundle_ids = HashSet::new();
            let mut existing_chunk_locations = HashMap::new();
            let mut previous_file_manifests = HashMap::new();
            for entry in walkdir::WalkDir::new(&bundles_root).sort_by_file_name() {
                let entry = entry?;
                if !entry.path().is_file() {
                    continue;
                }
                if entry.path().extension().is_some_and(|e| e == "bundle") {
                    if let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str()) {
                        let bundle_id = stem.to_string();
                        existing_bundle_ids.insert(bundle_id.clone());

                        let file = std::fs::File::open(entry.path())?;
                        let reader = g::bundle::BundleReader::open(file)?;
                        for chunk in reader.entries() {
                            existing_chunk_locations
                                .entry(chunk.chunk_hash.clone())
                                .or_insert_with(|| g::ExistingChunkLocation {
                                    bundle_id: bundle_id.clone(),
                                    raw_size: chunk.raw_size,
                                    compressed_size: chunk.compressed_size,
                                });
                        }
                    }
                }
            }
            let existing_chunk_ids: HashSet<String> =
                existing_chunk_locations.keys().cloned().collect();
            if repo_paths.targets_root.exists() {
                for entry in walkdir::WalkDir::new(&repo_paths.targets_root).sort_by_file_name() {
                    let entry = entry?;
                    if !entry.path().is_file() || entry.file_name() != "manifest.json" {
                        continue;
                    }

                    let manifest_json = std::fs::read_to_string(entry.path())?;
                    let manifest: g::manifest::TargetManifest =
                        serde_json::from_str(&manifest_json)?;
                    for file in manifest.files {
                        previous_file_manifests
                            .entry(file.path.clone())
                            .or_insert(file);
                    }
                }
            }

            let chunk_sizing_profile = g::manifest::ChunkSizingProfile::for_selection(
                chunk_profile,
                input_stats.median_file_size_bytes,
            );
            let bundle_sizing_profile = g::manifest::BundleSizingProfile::for_selection(
                bundle_profile,
                input_stats.target_logical_size_bytes,
            );
            let compression_profile = g::manifest::CompressionProfile {
                compression: g::manifest::COMPRESSION_ZSTD.to_string(),
                compression_unit,
                zstd_level: 3,
            };

            let (result, timings) = g::build_from_directory_with_timings_against_existing_store_and_manifests_and_profiles(
                &prepared_input.dir,
                &product,
                &content_repo,
                &target_id,
                &resolved_identity.target_tags,
                &existing_chunk_locations,
                &existing_bundle_ids,
                &previous_file_manifests,
                &chunk_sizing_profile,
                &bundle_sizing_profile,
                &compression_profile,
            )?;
            let mut result = result;
            apply_resolved_target_identity(&mut result.target_manifest, &resolved_identity);

            let manifest_path = target_root.join("manifest.json");
            let manifest_json = serde_json::to_string_pretty(&result.target_manifest)?;
            std::fs::write(&manifest_path, &manifest_json)?;
            println!("wrote target manifest: {}", manifest_path.display());

            for bundle in &result.bundles {
                let bundle_path = bundles_root.join(format!("{}.bundle", bundle.bundle_id));
                std::fs::write(&bundle_path, &bundle.data)?;
                println!(
                    "wrote bundle: {} ({} bytes)",
                    bundle_path.display(),
                    bundle.data.len()
                );
            }

            println!(
                "done. {} files in manifest, total logical: {} bytes",
                result.target_manifest.files.len(),
                result.target_manifest.total_logical_size
            );

            if let Ok(est) = g::estimate_build(&result.target_manifest, &Default::default(), 0) {
                println!(
                    "estimate: {} chunks missing, {:.1} MiB logical",
                    est.missing_chunk_count,
                    est.missing_logical_bytes as f64 / (1024.0 * 1024.0)
                );
            }

            print_build_timings(&timings);

            let required_chunk_count = result
                .target_manifest
                .files
                .iter()
                .flat_map(|f| &f.chunks)
                .map(|c| &c.chunk_hash)
                .collect::<std::collections::HashSet<_>>()
                .len();
            let required_bundle_count = result
                .target_manifest
                .files
                .iter()
                .flat_map(|f| f.chunks.iter().map(|c| c.chunk_hash.as_str()))
                .collect::<std::collections::HashSet<_>>()
                .len();
            let generated_bundle_count = result.bundles.len();
            let generated_bundle_bytes: u64 = result
                .bundles
                .iter()
                .map(|bundle| bundle.data.len() as u64)
                .sum();
            let reused_chunk_count = result
                .target_manifest
                .files
                .iter()
                .flat_map(|f| &f.chunks)
                .map(|c| &c.chunk_hash)
                .collect::<std::collections::HashSet<_>>()
                .iter()
                .filter(|chunk_id| existing_chunk_ids.contains(chunk_id.as_str()))
                .count();
            let new_chunk_count = required_chunk_count.saturating_sub(reused_chunk_count);
            let reused_bundle_count = required_bundle_count.saturating_sub(generated_bundle_count);
            let reused_chunk_percent =
                ratio_pct(reused_chunk_count as u64, required_chunk_count as u64);
            let reused_bundle_percent =
                ratio_pct(reused_bundle_count as u64, required_bundle_count as u64);
            let stats = TargetInsertStats {
                command: "target insert",
                repo_root: repo_paths.repo_root.display().to_string(),
                input_path: dir.display().to_string(),
                target_id: result.target_manifest.target_id.clone(),
                manifest_path: manifest_path.display().to_string(),
                product_id: product,
                content_repo_id: content_repo,
                version_id: resolved_identity
                    .target_tags
                    .get_exclusive("version")
                    .unwrap_or_default()
                    .to_string(),
                platform: resolved_identity
                    .target_tags
                    .get_exclusive("os")
                    .unwrap_or_default()
                    .to_string(),
                architecture: resolved_identity
                    .target_tags
                    .get_exclusive("arch")
                    .unwrap_or_default()
                    .to_string(),
                tier_id: resolved_identity
                    .target_tags
                    .get_exclusive("tier")
                    .unwrap_or_default()
                    .to_string(),
                file_count: result.target_manifest.files.len(),
                total_logical_size: result.target_manifest.total_logical_size,
                total_compressed_size_estimate: result
                    .target_manifest
                    .total_compressed_size_estimate,
                required_chunk_count,
                required_bundle_count,
                generated_bundle_count,
                generated_bundle_bytes,
                reused_chunk_count,
                new_chunk_count,
                reused_chunk_percent,
                reused_bundle_count,
                reused_bundle_percent,
                timings: build_timings_summary(&timings),
                outputs: optional_output_paths(&summary_out, &stats_json_out),
            };

            let summary_lines = vec![
                format!("command: {}", stats.command),
                format!("repo_root: {}", stats.repo_root),
                format!("input_path: {}", stats.input_path),
                format!("target_id: {}", stats.target_id),
                format!("manifest_path: {}", stats.manifest_path),
                format!(
                    "identity: product={}, content_repo={}, version={}, platform={}, architecture={}, tier={}",
                    stats.product_id,
                    stats.content_repo_id,
                    stats.version_id,
                    stats.platform,
                    stats.architecture,
                    stats.tier_id
                ),
                format!(
                    "files: count={}, logical_size={}, compressed_estimate={}",
                    stats.file_count,
                    format_bytes(stats.total_logical_size),
                    format_bytes(stats.total_compressed_size_estimate)
                ),
                format!(
                    "chunks: required={}, new={}, reused={}, reuse={:.2}%",
                    stats.required_chunk_count,
                    stats.new_chunk_count,
                    stats.reused_chunk_count,
                    stats.reused_chunk_percent
                ),
                format!(
                    "bundles: required={}, generated={}, reused={}, generated_size={}, reuse={:.2}%",
                    stats.required_bundle_count,
                    stats.generated_bundle_count,
                    stats.reused_bundle_count,
                    format_bytes(stats.generated_bundle_bytes),
                    stats.reused_bundle_percent
                ),
                format!("timings_total_seconds: {:.2}", stats.timings.total_seconds),
            ];
            write_optional_summary(&summary_out, &summary_lines)?;
            write_stats_json_output(&stats_json_out, &stats)?;
        }

        Commands::Target {
            command:
                TargetCommands::Export {
                    repo,
                    target,
                    output,
                    summary_out,
                    stats_json_out,
                },
        } => {
            let (repo_paths, _repo_manifest, _manifest_path, tm) =
                load_target_manifest_from_repo(&repo, &target)?;
            let bundle_paths = resolve_required_bundle_paths(&repo_paths.bundles_root, &tm)?;

            let (_, timings) =
                g::reconstruct_from_bundle_paths_with_timings(&tm, &bundle_paths, &output)?;
            println!("reconstructed to: {}", output.display());
            print_reconstruct_timings(&timings);

            let stats = TargetExportStats {
                command: "target export",
                repo_root: repo_paths.repo_root.display().to_string(),
                target_id: tm.target_id.clone(),
                output_dir: output.display().to_string(),
                file_count: tm.files.len(),
                required_chunk_count: tm
                    .files
                    .iter()
                    .flat_map(|f| &f.chunks)
                    .map(|c| &c.chunk_hash)
                    .collect::<std::collections::HashSet<_>>()
                    .len(),
                required_bundle_count: tm
                    .files
                    .iter()
                    .flat_map(|f| f.chunks.iter().map(|c| c.chunk_hash.as_str()))
                    .collect::<std::collections::HashSet<_>>()
                    .len(),
                resolved_bundle_file_count: bundle_paths.len(),
                total_logical_size: tm.total_logical_size,
                total_compressed_size_estimate: tm.total_compressed_size_estimate,
                timings: reconstruct_timings_summary(&timings),
                outputs: optional_output_paths(&summary_out, &stats_json_out),
            };
            let summary_lines = vec![
                format!("command: {}", stats.command),
                format!("repo_root: {}", stats.repo_root),
                format!("target_id: {}", stats.target_id),
                format!("output_dir: {}", stats.output_dir),
                format!(
                    "files: count={}, logical_size={}, compressed_estimate={}",
                    stats.file_count,
                    format_bytes(stats.total_logical_size),
                    format_bytes(stats.total_compressed_size_estimate)
                ),
                format!(
                    "bundles: required={}, resolved={}",
                    stats.required_bundle_count, stats.resolved_bundle_file_count
                ),
                format!("chunks: required={}", stats.required_chunk_count),
                format!("timings_total_seconds: {:.2}", stats.timings.total_seconds),
            ];
            write_optional_summary(&summary_out, &summary_lines)?;
            write_stats_json_output(&stats_json_out, &stats)?;
        }

        Commands::TargetInstall {
            command:
                TargetInstallCommands::Fetch {
                    repo,
                    target,
                    manifest_url,
                    manifest_sig_url,
                    bundle_urls_endpoint,
                    session_id,
                    bundles,
                },
        } => {
            let result = g::fetch_target_install_content(&g::TargetInstallFetchRequest {
                repo_path: repo,
                target_id: target,
                manifest_url,
                manifest_sig_url,
                bundle_urls_endpoint,
                session_id,
                bundles: parse_bundle_downloads(&bundles)?,
            })?;

            println!(
                "fetched target install content for {} into {}",
                result.target_id,
                result.repo_path.display()
            );
            println!(
                "bundles: required={}, downloaded={}, existing={}",
                result.required_bundle_count,
                result.downloaded_bundle_count,
                result.skipped_existing_bundle_count
            );
        }

        Commands::TargetInstall {
            command:
                TargetInstallCommands::Apply {
                    repo,
                    target,
                    dir,
                    staging_dir,
                    source_target,
                    source_dir,
                    summary_out,
                    stats_json_out,
                },
        } => {
            let (repo_paths, _repo_manifest, _manifest_path, tm) =
                load_target_manifest_from_repo(&repo, &target)?;
            let bundle_paths = resolve_required_bundle_paths(&repo_paths.bundles_root, &tm)?;
            let staging_dir =
                staging_dir.unwrap_or_else(|| default_staging_dir(&dir, &tm.target_id));

            let source_manifest = match source_target.as_ref() {
                Some(source_target) => {
                    let (_, _, _, source_manifest) =
                        load_target_manifest_from_repo(&repo, source_target)?;
                    Some(source_manifest)
                }
                None => None,
            };
            let source_install_dir = source_manifest
                .as_ref()
                .map(|_| source_dir.clone().unwrap_or_else(|| dir.clone()));
            let source = source_manifest
                .as_ref()
                .zip(source_install_dir.as_ref())
                .map(
                    |(source_manifest, source_install_dir)| g::TargetInstallApplySource {
                        target_manifest: source_manifest,
                        install_dir: source_install_dir,
                    },
                );

            let result = g::apply_target_install_with_timings(
                &tm,
                &bundle_paths,
                &dir,
                &staging_dir,
                source,
            )?;
            let reused_source_chunk_percent = ratio_pct(
                result.reused_source_chunk_count as u64,
                result.required_chunk_count as u64,
            );
            let stats = TargetInstallApplyStats {
                command: "target_install apply",
                repo_root: repo_paths.repo_root.display().to_string(),
                target_id: result.target_id.clone(),
                install_dir: result.install_dir.display().to_string(),
                staging_dir: result.staging_dir.display().to_string(),
                source_target_id: source_target.clone(),
                source_install_dir: source_install_dir
                    .as_ref()
                    .map(|path| path.display().to_string()),
                file_count: tm.files.len(),
                required_chunk_count: result.required_chunk_count,
                required_bundle_count: result.required_bundle_count,
                resolved_bundle_file_count: bundle_paths.len(),
                reused_source_chunk_count: result.reused_source_chunk_count,
                reused_source_chunk_percent,
                total_logical_size: tm.total_logical_size,
                total_compressed_size_estimate: tm.total_compressed_size_estimate,
                reconstruct_timings: reconstruct_timings_summary(&result.reconstruct_timings),
                verify_timings: verify_timings_summary(&result.verify_timings),
                outputs: optional_output_paths(&summary_out, &stats_json_out),
            };
            let summary_lines = vec![
                format!("command: {}", stats.command),
                format!("repo_root: {}", stats.repo_root),
                format!("target_id: {}", stats.target_id),
                format!("install_dir: {}", stats.install_dir),
                format!("staging_dir: {}", stats.staging_dir),
                format!(
                    "source: target={}, dir={}",
                    stats.source_target_id.as_deref().unwrap_or("none"),
                    stats.source_install_dir.as_deref().unwrap_or("none")
                ),
                format!(
                    "files: count={}, logical_size={}, compressed_estimate={}",
                    stats.file_count,
                    format_bytes(stats.total_logical_size),
                    format_bytes(stats.total_compressed_size_estimate)
                ),
                format!(
                    "chunks: required={}, reused_from_source={}, source_reuse={:.2}%",
                    stats.required_chunk_count,
                    stats.reused_source_chunk_count,
                    stats.reused_source_chunk_percent
                ),
                format!(
                    "bundles: required={}, resolved={}",
                    stats.required_bundle_count, stats.resolved_bundle_file_count
                ),
                format!(
                    "timings_reconstruct_seconds: {:.2}",
                    stats.reconstruct_timings.total_seconds
                ),
                format!(
                    "timings_verify_seconds: {:.2}",
                    stats.verify_timings.total_seconds
                ),
            ];
            write_optional_summary(&summary_out, &summary_lines)?;
            write_stats_json_output(&stats_json_out, &stats)?;

            println!(
                "applied target {} to {}",
                stats.target_id, stats.install_dir
            );
            println!(
                "reused {} of {} chunks from source ({:.2}%)",
                stats.reused_source_chunk_count,
                stats.required_chunk_count,
                stats.reused_source_chunk_percent
            );
            print_reconstruct_timings(&result.reconstruct_timings);
            print_verify_timings(&result.verify_timings);
        }

        Commands::TargetInstall {
            command:
                TargetInstallCommands::Verify {
                    repo,
                    target,
                    dir,
                    summary_out,
                    stats_json_out,
                },
        } => {
            let (repo_paths, _repo_manifest, _manifest_path, tm) =
                load_target_manifest_from_repo(&repo, &target)?;

            let (errors, timings) = g::verify_directory_with_timings(&tm, &dir)?;
            let mut missing_file_count = 0usize;
            let mut corrupt_file_count = 0usize;
            let mut extra_file_count = 0usize;
            for error in &errors {
                if error.starts_with("missing: ") {
                    missing_file_count += 1;
                } else if error.starts_with("extra: ") {
                    extra_file_count += 1;
                } else {
                    corrupt_file_count += 1;
                }
            }
            let stats = TargetInstallVerifyStats {
                command: "target_install verify",
                repo_root: repo_paths.repo_root.display().to_string(),
                target_id: tm.target_id.clone(),
                install_dir: dir.display().to_string(),
                expected_file_count: tm.files.len(),
                missing_file_count,
                corrupt_file_count,
                extra_file_count,
                verification_passed: errors.is_empty(),
                errors: errors.clone(),
                timings: verify_timings_summary(&timings),
                outputs: optional_output_paths(&summary_out, &stats_json_out),
            };
            let summary_lines = vec![
                format!("command: {}", stats.command),
                format!("repo_root: {}", stats.repo_root),
                format!("target_id: {}", stats.target_id),
                format!("install_dir: {}", stats.install_dir),
                format!(
                    "status: {}",
                    if stats.verification_passed {
                        "passed"
                    } else {
                        "failed"
                    }
                ),
                format!("expected_file_count: {}", stats.expected_file_count),
                format!(
                    "issues: missing={}, corrupt={}, extra={}",
                    stats.missing_file_count, stats.corrupt_file_count, stats.extra_file_count
                ),
                format!("timings_total_seconds: {:.2}", stats.timings.total_seconds),
            ];
            write_optional_summary(&summary_out, &summary_lines)?;
            write_stats_json_output(&stats_json_out, &stats)?;

            if errors.is_empty() {
                println!("verification passed for {}", dir.display());
                print_verify_timings(&timings);
            } else {
                eprintln!("{} verification errors:", errors.len());
                for err in &errors {
                    eprintln!("  - {}", err);
                }
                std::process::exit(1);
            }
        }

        Commands::Bundle {
            command: BundleCommands::Inspect { bundle },
        } => {
            let data = std::fs::read(&bundle)?;
            let reader = g::bundle::read_bundle(&data)?;

            println!("bundle: {}", bundle.display());
            println!("  chunks: {}", reader.chunk_count());

            for entry in reader.entries() {
                println!(
                    "  - {} raw={} compressed={} ratio={:.1}%",
                    &entry.chunk_hash[..16.min(entry.chunk_hash.len())],
                    entry.raw_size,
                    entry.compressed_size,
                    100.0 * entry.compressed_size as f64 / entry.raw_size as f64
                );
            }
        }

        Commands::Path {
            command:
                PathCommands::Compare {
                    expected,
                    actual,
                    summary_out,
                    stats_json_out,
                },
        } => {
            let stats = compare_paths(
                &expected,
                &actual,
                optional_output_paths(&summary_out, &stats_json_out),
            )?;
            let mut summary_lines = vec![
                format!("command: {}", stats.command),
                format!("expected_path: {}", stats.expected_path),
                format!("actual_path: {}", stats.actual_path),
                format!(
                    "status: {}",
                    if stats.compare_passed {
                        "match"
                    } else {
                        "failed"
                    }
                ),
                format!(
                    "files: expected={}, actual={}, matched={}, mismatched={}, missing={}, extra={}",
                    stats.expected_file_count,
                    stats.actual_file_count,
                    stats.matched_file_count,
                    stats.mismatched_file_count,
                    stats.missing_file_count,
                    stats.extra_file_count
                ),
            ];
            if !stats.mismatched_files.is_empty() {
                summary_lines.push("[mismatched]".to_string());
                summary_lines.extend(stats.mismatched_files.iter().cloned());
            }
            if !stats.missing_files.is_empty() {
                summary_lines.push("[missing]".to_string());
                summary_lines.extend(stats.missing_files.iter().cloned());
            }
            if !stats.extra_files.is_empty() {
                summary_lines.push("[extra]".to_string());
                summary_lines.extend(stats.extra_files.iter().cloned());
            }
            write_optional_summary(&summary_out, &summary_lines)?;
            write_stats_json_output(&stats_json_out, &stats)?;

            if stats.compare_passed {
                println!(
                    "binary compare passed: {} matched, {} expected files",
                    stats.matched_file_count, stats.expected_file_count
                );
            } else {
                eprintln!(
                    "binary compare failed: mismatched={}, missing={}, extra={}",
                    stats.mismatched_file_count, stats.missing_file_count, stats.extra_file_count
                );
                for relative_path in &stats.mismatched_files {
                    eprintln!("  MISMATCH: {}", relative_path);
                }
                for relative_path in &stats.missing_files {
                    eprintln!("  MISSING: {}", relative_path);
                }
                for relative_path in &stats.extra_files {
                    eprintln!("  EXTRA: {}", relative_path);
                }
                std::process::exit(1);
            }
        }

        Commands::Target {
            command:
                TargetCommands::Estimate {
                    dir,
                    product,
                    version,
                },
        } => {
            let prepared_input = prepare_input_path(&dir)?;
            let result = {
                let mut build_tags = g::manifest::TargetTagSet::default();
                build_tags.upsert_exclusive("version", version.clone());
                build_tags.upsert_exclusive("os", "windows");
                build_tags.upsert_exclusive("arch", "x86_64");
                build_tags.upsert_exclusive("tier", "base");
                g::build_from_directory(&prepared_input.dir, &product, "base", "1", &build_tags)?
            };

            let est = g::estimate_build(
                &result.target_manifest,
                &Default::default(),
                1024 * 1024 * 1024,
            )?;

            println!("fresh install estimate:");
            println!("  files: {}", result.target_manifest.files.len());
            println!(
                "  logical size:  {:.2} MiB",
                result.target_manifest.total_logical_size as f64 / (1024.0 * 1024.0)
            );
            println!("  missing chunks: {}", est.missing_chunk_count);
            println!(
                "  missing logical: {:.2} MiB",
                est.missing_logical_bytes as f64 / (1024.0 * 1024.0)
            );
            println!(
                "  missing compressed: {:.2} MiB",
                est.missing_compressed_bytes as f64 / (1024.0 * 1024.0)
            );
            println!("  estimated bundles: {}", est.required_bundle_count);
            println!("  fragmentation score: {:.1}", est.fragmentation_score);
        }
    }

    Ok(())
}
