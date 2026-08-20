use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::progress::{GutchOperation, GutchProgressEvent, GutchProgressPhase};
use crate::remote::backend::{
    CreatorProductResponse, CreatorRepoResponse, CreatorReposResponse, CreatorTargetResponse,
    request_json_from_backend,
};
use std::io::Write;

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullConflictPolicy {
    KeepLocal,
    KeepRemote,
    KeepNewer,
}

impl PullConflictPolicy {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim() {
            "keep-local" => Ok(Self::KeepLocal),
            "keep-remote" => Ok(Self::KeepRemote),
            "keep-newer" => Ok(Self::KeepNewer),
            other => Err(Error::manifest(format!(
                "invalid --on-conflict value '{}' (expected keep-local, keep-remote, or keep-newer)",
                other
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteRepoEntry {
    pub product_id: i64,
    pub product_name: String,
    pub product_slug: String,
    pub repo_id: i64,
    pub repo_name: String,
    pub is_base: bool,
}

#[derive(Debug, Serialize)]
pub struct RepoPullStats {
    pub command: &'static str,
    pub repo_root: String,
    pub product_id: i64,
    pub remote_repo_id: i64,
    pub repo_manifest_downloaded: bool,
    pub chunk_map_downloaded: bool,
    pub target_downloaded_count: usize,
    pub target_skipped_count: usize,
    pub bundle_downloaded_count: usize,
    pub bundle_skipped_count: usize,
    pub kept_newer_local_count: usize,
    pub local_only_targets_kept: usize,
    pub pruned_target_count: usize,
    pub pruned_bundle_count: usize,
}

#[derive(Debug)]
pub struct RepoPullPlan {
    pub repo_root: String,
    pub product_id: i64,
    pub remote_repo_id: i64,
    pub remote_repo_name: String,
    pub repo_manifest_needed: bool,
    pub chunk_map_needed: bool,
    pub targets_to_download: Vec<String>,
    pub targets_to_skip: Vec<String>,
    pub local_only_targets: Vec<String>,
    pub kept_newer_local_targets: Vec<String>,
    pub bundle_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArtifactDecision {
    Identical,
    Fetch,
    KeepLocalNewer,
}

struct RemoteRepoRecord {
    product_id: i64,
    remote_repo_id: i64,
    name: String,
    repo_metadata_hash_b3: String,
    chunk_map_hash_b3: String,
    updated_at: String,
}

struct RemoteTargetRecord {
    target_id: String,
    target_metadata_hash_b3: String,
    target_product_metadata_hash_b3: String,
    target_blueprint_hash_b3: String,
    uploaded_at: String,
    created_at: String,
}

fn repo_file_hash(bytes: &[u8]) -> String {
    crate::hash::hash_hex(&blake3::hash(bytes))
}

fn hashes_differ(local_hash: &str, remote_hash: &str) -> bool {
    let local_hash = local_hash.trim();
    let remote_hash = remote_hash.trim();
    if local_hash.is_empty() && remote_hash.is_empty() {
        return false;
    }
    remote_hash.is_empty() || local_hash != remote_hash
}

fn parse_remote_spec(spec: &str) -> Result<(&str, &str)> {
    let spec = spec.trim();
    let (product, repo) = spec.split_once('/').ok_or_else(|| {
        Error::manifest(format!(
            "invalid --remote '{}': expected <product>/<repo> as printed by 'repo remote list'",
            spec
        ))
    })?;
    let product = product.trim();
    let repo = repo.trim();
    if product.is_empty() || repo.is_empty() || repo.contains('/') {
        return Err(Error::manifest(format!(
            "invalid --remote '{}': expected <product>/<repo>",
            spec
        )));
    }
    Ok((product, repo))
}

fn local_file_mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn remote_updated_at_is_newer_or_equal(
    remote_updated_at: &str,
    local_modified: Option<std::time::SystemTime>,
) -> bool {
    let Ok(remote) = chrono::DateTime::parse_from_rfc3339(remote_updated_at) else {
        return true;
    };
    let remote_utc = remote.with_timezone(&chrono::Utc);
    let Some(local_modified) = local_modified else {
        return true;
    };
    let remote_system = std::time::SystemTime::from(remote_utc);
    remote_system >= local_modified
}

fn decide_artifact(
    local_exists: bool,
    local_hash: String,
    remote_hash: &str,
    remote_updated_at: &str,
    local_modified: Option<std::time::SystemTime>,
    policy: PullConflictPolicy,
) -> ArtifactDecision {
    if !local_exists || hashes_differ(&local_hash, remote_hash) {
        match policy {
            PullConflictPolicy::KeepLocal => {
                if local_exists {
                    ArtifactDecision::KeepLocalNewer
                } else {
                    ArtifactDecision::Fetch
                }
            }
            PullConflictPolicy::KeepRemote => ArtifactDecision::Fetch,
            PullConflictPolicy::KeepNewer => {
                if remote_updated_at_is_newer_or_equal(remote_updated_at, local_modified) {
                    ArtifactDecision::Fetch
                } else {
                    ArtifactDecision::KeepLocalNewer
                }
            }
        }
    } else {
        ArtifactDecision::Identical
    }
}

fn build_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent("Gutch CLI")
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(15 * 60))
        .build()
        .map_err(|err| Error::manifest(format!("failed to create pull client: {}", err)))
}

fn base_endpoint(endpoint: &str) -> String {
    endpoint.trim_end_matches('/').to_string()
}

fn fetch_creator_products(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    session_id: &str,
) -> Result<Vec<CreatorProductResponse>> {
    let response: crate::remote::backend::CreatorProductsResponse = request_json_from_backend(
        client
            .get(format!(
                "{}/v1/users/me/creator/products",
                base_endpoint(endpoint)
            ))
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", session_id)),
    )?;
    Ok(response.products)
}

fn fetch_creator_repos(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    session_id: &str,
    product_id: i64,
) -> Result<Vec<CreatorRepoResponse>> {
    let response: CreatorReposResponse = request_json_from_backend(
        client
            .get(format!(
                "{}/v1/users/me/creator/products/{}/repos",
                base_endpoint(endpoint),
                product_id
            ))
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", session_id)),
    )?;
    Ok(response.repos)
}

fn fetch_creator_targets(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    session_id: &str,
    product_id: i64,
    remote_repo_id: i64,
) -> Result<Vec<RemoteTargetRecord>> {
    let response: crate::remote::backend::CreatorRepoTargetsResponse = request_json_from_backend(
        client
            .get(format!(
                "{}/v1/users/me/creator/products/{}/repos/{}/targets",
                base_endpoint(endpoint),
                product_id,
                remote_repo_id
            ))
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", session_id)),
    )?;
    let mut records = Vec::new();
    for target in response.targets {
        records.push(remote_target_record(target));
    }
    for tier in response.tier_labels {
        for target in tier.targets {
            records.push(remote_target_record(target));
        }
    }
    Ok(records)
}

fn remote_target_record(target: CreatorTargetResponse) -> RemoteTargetRecord {
    RemoteTargetRecord {
        target_id: target.target_id,
        target_metadata_hash_b3: target.target_metadata_hash_b3,
        target_product_metadata_hash_b3: target.target_product_metadata_hash_b3,
        target_blueprint_hash_b3: target.target_blueprint_hash_b3,
        uploaded_at: target.uploaded_at,
        created_at: target.created_at,
    }
}

fn local_repo_metadata_hash_b3(repo_paths: &crate::GutchRepoPaths) -> Result<String> {
    if !repo_paths.repo_metadata_path.exists() {
        return Ok(String::new());
    }
    Ok(repo_file_hash(&crate::helpers::read(
        &repo_paths.repo_metadata_path,
    )?))
}

fn local_chunk_map_hash_b3(repo_paths: &crate::GutchRepoPaths) -> Result<String> {
    if !repo_paths.chunk_map_path.exists() {
        return Ok(String::new());
    }
    let chunk_index = crate::load_chunk_index(repo_paths)?;
    Ok(repo_file_hash(&serde_json::to_vec_pretty(&chunk_index)?))
}

fn local_target_metadata_hash_b3(
    repo_paths: &crate::GutchRepoPaths,
    target_id: &str,
) -> Result<String> {
    if !crate::target_manifest_path(repo_paths, target_id).exists()
        || !crate::target_blueprint_path(repo_paths, target_id).exists()
    {
        return Ok(String::new());
    }
    let manifest = crate::load_target_manifest(repo_paths, target_id)?;
    Ok(repo_file_hash(
        &crate::serialize_target_manifest_for_remote(&manifest)?,
    ))
}

fn local_target_product_metadata_hash_b3(
    repo_paths: &crate::GutchRepoPaths,
    target_id: &str,
) -> Result<String> {
    if !crate::target_manifest_path(repo_paths, target_id).exists()
        || !crate::target_blueprint_path(repo_paths, target_id).exists()
    {
        return Ok(String::new());
    }
    let manifest = crate::load_target_manifest(repo_paths, target_id)?;
    let Some(bytes) = crate::serialize_target_product_metadata_for_remote(&manifest)? else {
        return Ok(String::new());
    };
    Ok(repo_file_hash(&bytes))
}

fn local_target_blueprint_hash_b3(
    repo_paths: &crate::GutchRepoPaths,
    target_id: &str,
) -> Result<String> {
    let blueprint_path = crate::target_blueprint_path(repo_paths, target_id);
    if !blueprint_path.exists() {
        return Ok(String::new());
    }
    Ok(crate::hash::hash_hex(&crate::hash::hash_file(
        &blueprint_path,
    )?))
}

fn remote_repo_record(repo: &CreatorRepoResponse, product_id: i64) -> RemoteRepoRecord {
    RemoteRepoRecord {
        product_id,
        remote_repo_id: repo.id,
        name: repo.name.clone(),
        repo_metadata_hash_b3: repo.repo_metadata_hash_b3.clone(),
        chunk_map_hash_b3: repo.chunk_map_hash_b3.clone(),
        updated_at: repo.updated_at.clone(),
    }
}

fn resolve_remote_record_from_binding(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    session_id: &str,
    product_id: i64,
    remote_repo_id: i64,
) -> Result<RemoteRepoRecord> {
    let repos = fetch_creator_repos(client, endpoint, session_id, product_id)?;
    let repo = repos
        .iter()
        .find(|repo| repo.id == remote_repo_id)
        .ok_or_else(|| {
            Error::manifest(format!(
                "remote repo {} not found for product {}",
                remote_repo_id, product_id
            ))
        })?;
    Ok(remote_repo_record(repo, product_id))
}

fn resolve_remote_record_from_spec(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    session_id: &str,
    spec: &str,
) -> Result<RemoteRepoRecord> {
    let (product_spec, repo_spec) = parse_remote_spec(spec)?;
    let products = fetch_creator_products(client, endpoint, session_id)?;
    let product = products
        .iter()
        .find(|product| product.slug == product_spec || product.name == product_spec)
        .ok_or_else(|| Error::manifest(format!("no creator product matches '{}'", product_spec)))?;
    let repos = fetch_creator_repos(client, endpoint, session_id, product.id)?;
    let repo = repos
        .iter()
        .find(|repo| repo.name == repo_spec)
        .ok_or_else(|| {
            Error::manifest(format!(
                "product '{}' has no repo named '{}'",
                product.name, repo_spec
            ))
        })?;
    Ok(remote_repo_record(repo, product.id))
}

pub fn list_remote_repos(endpoint: &str, session_id: &str) -> Result<Vec<RemoteRepoEntry>> {
    let client = build_client()?;
    let products = fetch_creator_products(&client, endpoint, session_id)?;
    let mut entries = Vec::new();
    for product in products {
        let repos = fetch_creator_repos(&client, endpoint, session_id, product.id)?;
        for repo in repos {
            entries.push(RemoteRepoEntry {
                product_id: product.id,
                product_name: product.name.clone(),
                product_slug: product.slug.clone(),
                repo_id: repo.id,
                repo_name: repo.name,
                is_base: repo.is_base,
            });
        }
    }
    entries.sort_by(|left, right| {
        left.product_name
            .cmp(&right.product_name)
            .then_with(|| left.repo_name.cmp(&right.repo_name))
    });
    Ok(entries)
}

fn resolve_pull_target(
    repo: &Path,
    remote_spec: Option<&str>,
    client: &reqwest::blocking::Client,
    endpoint: &str,
    session_id: &str,
) -> Result<(RemoteRepoRecord, crate::GutchRepoPaths)> {
    let repo_paths = crate::GutchRepoPaths::from_repo_root(repo);
    let record = if repo_paths.repo_metadata_path.exists() {
        let (_, manifest) = crate::load_gutch_repo_manifest(repo)?;
        if manifest.is_linked() {
            let product_id = manifest.product_id.parse::<i64>().map_err(|_| {
                Error::manifest(format!(
                    "invalid product_id '{}' in manifest",
                    manifest.product_id
                ))
            })?;
            let remote_repo_id = manifest.remote_repo_id_value().ok_or_else(|| {
                Error::manifest(format!(
                    "invalid remote_repo_id '{}' in manifest",
                    manifest.remote_repo_id
                ))
            })?;
            resolve_remote_record_from_binding(
                client,
                endpoint,
                session_id,
                product_id,
                remote_repo_id,
            )?
        } else if let Some(spec) = remote_spec {
            resolve_remote_record_from_spec(client, endpoint, session_id, spec)?
        } else {
            return Err(Error::manifest(format!(
                "repo '{}' is not linked and no --remote was given. Run 'gutch repo remote list' to find the repo, then use --remote <product>/<repo>.",
                repo.display()
            )));
        }
    } else if let Some(spec) = remote_spec {
        resolve_remote_record_from_spec(client, endpoint, session_id, spec)?
    } else {
        return Err(Error::manifest(format!(
            "no gutch repo at '{}' and no --remote was given. Run 'gutch repo remote list' to find the repo, then use --remote <product>/<repo>.",
            repo.display()
        )));
    };
    Ok((record, repo_paths))
}

fn ensure_repo_layout(repo_paths: &crate::GutchRepoPaths, name: &str) -> Result<()> {
    crate::ensure_gutch_repo_layout(repo_paths, name)?;
    Ok(())
}

fn compute_pull_plan(
    repo_root: &Path,
    repo_paths: &crate::GutchRepoPaths,
    record: &RemoteRepoRecord,
    targets: &[RemoteTargetRecord],
    policy: PullConflictPolicy,
) -> Result<RepoPullPlan> {
    let repo_manifest_needed = decide_artifact(
        repo_paths.repo_metadata_path.exists(),
        local_repo_metadata_hash_b3(repo_paths)?,
        &record.repo_metadata_hash_b3,
        &record.updated_at,
        local_file_mtime(&repo_paths.repo_metadata_path),
        policy,
    ) != ArtifactDecision::Identical;
    let chunk_map_needed = decide_artifact(
        repo_paths.chunk_map_path.exists(),
        local_chunk_map_hash_b3(repo_paths)?,
        &record.chunk_map_hash_b3,
        &record.updated_at,
        local_file_mtime(&repo_paths.chunk_map_path),
        policy,
    ) != ArtifactDecision::Identical;
    let mut targets_to_download = Vec::new();
    let mut targets_to_skip = Vec::new();
    let mut kept_newer_local_targets = Vec::new();
    for target in targets {
        let updated_at = if target.uploaded_at.is_empty() {
            &target.created_at
        } else {
            &target.uploaded_at
        };
        let manifest_path = crate::target_manifest_path(repo_paths, &target.target_id);
        let decision = decide_artifact(
            manifest_path.exists(),
            local_target_metadata_hash_b3(repo_paths, &target.target_id)?,
            &target.target_metadata_hash_b3,
            updated_at,
            local_file_mtime(&manifest_path),
            policy,
        );
        let decision = if decision == ArtifactDecision::Identical
            && hashes_differ(
                &local_target_blueprint_hash_b3(repo_paths, &target.target_id)?,
                &target.target_blueprint_hash_b3,
            ) {
            ArtifactDecision::Fetch
        } else {
            decision
        };
        let product_metadata_path =
            crate::target_product_metadata_path(repo_paths, &target.target_id);
        let product_metadata_decision = decide_artifact(
            product_metadata_path.exists(),
            local_target_product_metadata_hash_b3(repo_paths, &target.target_id)?,
            &target.target_product_metadata_hash_b3,
            updated_at,
            local_file_mtime(&product_metadata_path),
            policy,
        );
        let decision = match (decision, product_metadata_decision) {
            (ArtifactDecision::Fetch, _) | (_, ArtifactDecision::Fetch) => ArtifactDecision::Fetch,
            (ArtifactDecision::KeepLocalNewer, _) | (_, ArtifactDecision::KeepLocalNewer) => {
                ArtifactDecision::KeepLocalNewer
            }
            _ => ArtifactDecision::Identical,
        };
        match decision {
            ArtifactDecision::Identical => targets_to_skip.push(target.target_id.clone()),
            ArtifactDecision::Fetch => targets_to_download.push(target.target_id.clone()),
            ArtifactDecision::KeepLocalNewer => {
                kept_newer_local_targets.push(target.target_id.clone());
                targets_to_skip.push(target.target_id.clone());
            }
        }
    }
    targets_to_download.sort();
    targets_to_skip.sort();
    kept_newer_local_targets.sort();

    let remote_target_ids: HashSet<&str> = targets
        .iter()
        .map(|target| target.target_id.as_str())
        .collect();
    let mut local_only_targets = Vec::new();
    for entry in walkdir::WalkDir::new(&repo_paths.targets_root).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file()
            || entry.file_name() != crate::repo::target_manifests::TARGET_METADATA_FILE
        {
            continue;
        }
        if let Some(target_id) = entry
            .path()
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|value| value.to_str())
            && !remote_target_ids.contains(target_id)
        {
            local_only_targets.push(target_id.to_string());
        }
    }
    local_only_targets.sort();

    Ok(RepoPullPlan {
        repo_root: repo_root.display().to_string(),
        product_id: record.product_id,
        remote_repo_id: record.remote_repo_id,
        remote_repo_name: record.name.clone(),
        repo_manifest_needed,
        chunk_map_needed,
        targets_to_download,
        targets_to_skip,
        local_only_targets,
        kept_newer_local_targets,
        bundle_count: 0,
    })
}

pub fn plan_repo_pull(
    repo: &Path,
    remote_spec: Option<&str>,
    endpoint: &str,
    session_id: &str,
    policy: PullConflictPolicy,
) -> Result<RepoPullPlan> {
    let client = build_client()?;
    let (record, repo_paths) =
        resolve_pull_target(repo, remote_spec, &client, endpoint, session_id)?;
    let targets = fetch_creator_targets(
        &client,
        endpoint,
        session_id,
        record.product_id,
        record.remote_repo_id,
    )?;
    compute_pull_plan(repo, &repo_paths, &record, &targets, policy)
}

fn emit(
    progress: &Option<std::sync::Arc<dyn crate::GutchProgressSink>>,
    operation: GutchOperation,
    phase: GutchProgressPhase,
    message: String,
    current: usize,
    total: usize,
    detail: Option<String>,
    done: bool,
) {
    crate::progress::emit_progress(
        progress,
        GutchProgressEvent {
            operation,
            phase,
            message,
            current,
            total,
            transferred_bytes: 0,
            total_bytes: 0,
            detail,
            done,
        },
    );
}

fn download_json(client: &reqwest::blocking::Client, url: &str) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .map_err(|err| Error::manifest(format!("download request failed: {}", err)))?;
    if !response.status().is_success() {
        return Err(Error::manifest(
            crate::remote::backend::parse_backend_error_response(response),
        ));
    }
    response
        .bytes()
        .map(|bytes| bytes.to_vec())
        .map_err(|err| Error::manifest(format!("failed to read download body: {}", err)))
}

fn download_binary(client: &reqwest::blocking::Client, url: &str) -> Result<Vec<u8>> {
    download_json(client, url)
}

fn download_to_path(
    client: &reqwest::blocking::Client,
    url: &str,
    session_id: Option<&str>,
    path: &Path,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        crate::helpers::create_dir_all(parent)?;
    }
    let temporary_path = path.with_extension("part");
    if temporary_path.exists() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    let result = (|| -> Result<()> {
        let mut request = client.get(url);
        if let Some(session_id) = session_id
            && !url.contains("X-Amz-Algorithm=")
            && !url.contains("sig=")
        {
            request = request.header("Authorization", format!("Bearer {}", session_id));
        }
        let mut response = request
            .send()
            .map_err(|err| Error::manifest(format!("download request failed: {}", err)))?;
        if !response.status().is_success() {
            return Err(Error::manifest(
                crate::remote::backend::parse_backend_error_response(response),
            ));
        }
        let mut file = std::fs::File::create(&temporary_path)?;
        std::io::copy(&mut response, &mut file)?;
        file.flush()?;
        drop(file);
        std::fs::rename(&temporary_path, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    result
}

fn request_json_authed<T: for<'de> serde::Deserialize<'de>>(
    client: &reqwest::blocking::Client,
    url: &str,
    session_id: &str,
) -> Result<T> {
    request_json_from_backend(
        client
            .get(url)
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", session_id)),
    )
}

fn fetch_repo_metadata_url(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    session_id: &str,
    product_id: i64,
    remote_repo_id: i64,
) -> Result<String> {
    #[derive(serde::Deserialize)]
    struct RepoMetadataResponse {
        metadata_url: String,
    }
    let response: RepoMetadataResponse = request_json_authed(
        client,
        &format!(
            "{}/v1/products/{}/repos/{}/metadata",
            base_endpoint(endpoint),
            product_id,
            remote_repo_id
        ),
        session_id,
    )?;
    Ok(response.metadata_url)
}

fn fetch_chunk_map_url(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    session_id: &str,
    product_id: i64,
    remote_repo_id: i64,
) -> Result<String> {
    #[derive(serde::Deserialize)]
    struct ChunkMapResponse {
        chunk_index_url: String,
    }
    let response: ChunkMapResponse = request_json_authed(
        client,
        &format!(
            "{}/v1/products/{}/repos/{}/chunks",
            base_endpoint(endpoint),
            product_id,
            remote_repo_id
        ),
        session_id,
    )?;
    Ok(response.chunk_index_url)
}

fn fetch_target_metadata_url(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    session_id: &str,
    product_id: i64,
    remote_repo_id: i64,
    target_id: &str,
) -> Result<(String, String, Option<String>)> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct TargetMetadataResponse {
        metadata_url: String,
        blueprint_url: String,
        #[serde(default)]
        target_product_metadata_url: Option<String>,
    }
    let response: TargetMetadataResponse = request_json_authed(
        client,
        &format!(
            "{}/v1/products/{}/repos/{}/targets/{}/metadata",
            base_endpoint(endpoint),
            product_id,
            remote_repo_id,
            url_encode_path_segment(target_id)
        ),
        session_id,
    )?;
    Ok((
        response.metadata_url,
        response.blueprint_url,
        response.target_product_metadata_url,
    ))
}

fn url_encode_path_segment(value: &str) -> String {
    value.replace(' ', "%20")
}

#[derive(Default)]
struct BundleDownloadSet {
    urls: HashMap<String, String>,
}

fn fetch_bundle_download_urls(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    session_id: &str,
    product_id: i64,
    remote_repo_id: i64,
    target_id: &str,
    bundle_ids: &[String],
) -> Result<BundleDownloadSet> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct BundleDownloadsResponse {
        bundles: Vec<BundleDownloadEntry>,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct BundleDownloadEntry {
        bundle_id: String,
        download_url: String,
    }
    if bundle_ids.is_empty() {
        return Ok(BundleDownloadSet::default());
    }
    let ids = bundle_ids.join(",");
    let response: BundleDownloadsResponse = request_json_from_backend(
        client
            .get(format!(
                "{}/v1/products/{}/repos/{}/targets/{}/bundle-downloads?ids={}",
                base_endpoint(endpoint),
                product_id,
                remote_repo_id,
                url_encode_path_segment(target_id),
                url_encode_query(&ids)
            ))
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", session_id)),
    )?;
    let mut set = BundleDownloadSet::default();
    for entry in response.bundles {
        set.urls.insert(entry.bundle_id, entry.download_url);
    }
    Ok(set)
}

fn url_encode_query(value: &str) -> String {
    value.replace(',', "%2C").replace(' ', "%20")
}

fn required_bundle_ids_by_target(
    repo_paths: &crate::GutchRepoPaths,
) -> Result<Vec<(String, Vec<String>)>> {
    let chunk_index = crate::load_chunk_index(repo_paths)?;
    let mut by_target = Vec::new();
    for entry in walkdir::WalkDir::new(&repo_paths.targets_root).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file()
            || entry.file_name() != crate::repo::target_manifests::TARGET_METADATA_FILE
        {
            continue;
        }
        let Some(target_id) = entry
            .path()
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|value| value.to_str())
        else {
            continue;
        };
        let manifest = crate::load_target_manifest(repo_paths, target_id)?;
        let mut ids: Vec<String> = crate::repo::required_bundle_ids(&manifest, &chunk_index)
            .into_iter()
            .collect();
        ids.sort();
        by_target.push((target_id.to_string(), ids));
    }
    Ok(by_target)
}

fn collect_local_bundle_paths(bundles_root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in walkdir::WalkDir::new(bundles_root).sort_by_file_name() {
        let entry = entry?;
        if entry.path().is_file() && crate::bundle_id_from_bundle_path(entry.path()).is_some() {
            paths.push(entry.path().to_path_buf());
        }
    }
    Ok(paths)
}

pub fn pull_repo(
    repo: PathBuf,
    remote_spec: Option<String>,
    endpoint: String,
    session_id: String,
    policy: PullConflictPolicy,
    prune: bool,
    control: crate::GutchOperationControl,
) -> Result<RepoPullStats> {
    control.checkpoint()?;
    let progress = control.progress().clone();
    let client = build_client()?;
    let (record, repo_paths) = resolve_pull_target(
        &repo,
        remote_spec.as_deref(),
        &client,
        &endpoint,
        &session_id,
    )?;
    ensure_repo_layout(&repo_paths, &record.name)?;

    emit(
        &progress,
        GutchOperation::Pull,
        GutchProgressPhase::Planning,
        "Planning pull changes".to_string(),
        0,
        1,
        None,
        false,
    );

    let targets = fetch_creator_targets(
        &client,
        &endpoint,
        &session_id,
        record.product_id,
        record.remote_repo_id,
    )?;
    let plan = compute_pull_plan(&repo, &repo_paths, &record, &targets, policy)?;
    control.checkpoint()?;

    let mut stats = RepoPullStats {
        command: "repo pull",
        repo_root: repo_paths.repo_root.display().to_string(),
        product_id: record.product_id,
        remote_repo_id: record.remote_repo_id,
        repo_manifest_downloaded: false,
        chunk_map_downloaded: false,
        target_downloaded_count: 0,
        target_skipped_count: plan.targets_to_skip.len(),
        bundle_downloaded_count: 0,
        bundle_skipped_count: 0,
        kept_newer_local_count: plan.kept_newer_local_targets.len(),
        local_only_targets_kept: 0,
        pruned_target_count: 0,
        pruned_bundle_count: 0,
    };

    let total_steps = 1
        + usize::from(plan.repo_manifest_needed)
        + usize::from(plan.chunk_map_needed)
        + plan.targets_to_download.len()
        + 2;
    let mut step = 0usize;

    if plan.repo_manifest_needed {
        control.checkpoint()?;
        step += 1;
        emit(
            &progress,
            GutchOperation::Pull,
            GutchProgressPhase::RepoMetadata,
            "Downloading repo metadata".to_string(),
            step,
            total_steps,
            None,
            false,
        );
        let url = fetch_repo_metadata_url(
            &client,
            &endpoint,
            &session_id,
            record.product_id,
            record.remote_repo_id,
        )?;
        let bytes = download_json(&client, &url)?;
        let mut manifest: crate::manifest::gutch_repo::GutchRepoManifest =
            crate::helpers::from_json_bytes(&bytes)?;
        manifest.product_id = record.product_id.to_string();
        manifest.remote_repo_id = record.remote_repo_id.to_string();
        manifest.sync_product_metadata_from_binding();
        crate::write_gutch_repo_manifest(&repo_paths, &manifest)?;
        stats.repo_manifest_downloaded = true;
    }

    if plan.chunk_map_needed {
        control.checkpoint()?;
        step += 1;
        emit(
            &progress,
            GutchOperation::Pull,
            GutchProgressPhase::ChunkIndex,
            "Downloading chunk map".to_string(),
            step,
            total_steps,
            None,
            false,
        );
        let url = fetch_chunk_map_url(
            &client,
            &endpoint,
            &session_id,
            record.product_id,
            record.remote_repo_id,
        )?;
        let bytes = download_json(&client, &url)?;
        let chunk_index: crate::manifest::ChunkIndex =
            serde_json::from_slice(&bytes).map_err(|err| {
                Error::manifest(format!("downloaded chunk map is invalid JSON: {}", err))
            })?;
        crate::repo::write_chunk_index(&repo_paths, &chunk_index)?;
        stats.chunk_map_downloaded = true;
    }

    let target_start = step;
    for target_id in &plan.targets_to_download {
        control.checkpoint()?;
        step += 1;
        emit(
            &progress,
            GutchOperation::Pull,
            GutchProgressPhase::TargetMetadata,
            format!(
                "Downloading target metadata ({}/{})",
                step - target_start,
                plan.targets_to_download.len().max(1)
            ),
            step,
            total_steps,
            Some(target_id.clone()),
            false,
        );
        let (metadata_url, blueprint_url, product_metadata_url) = fetch_target_metadata_url(
            &client,
            &endpoint,
            &session_id,
            record.product_id,
            record.remote_repo_id,
            target_id,
        )?;
        let metadata_bytes = download_json(&client, &metadata_url)?;
        let metadata = crate::target_manifest_from_download(target_id, &metadata_bytes)?;
        crate::repo::write_target_metadata_only(&repo_paths, &metadata)?;
        if let Some(product_metadata_url) = product_metadata_url {
            let product_metadata_bytes = download_json(&client, &product_metadata_url)?;
            let product_metadata: crate::manifest::TargetProductMetadata =
                crate::helpers::from_json_bytes(&product_metadata_bytes)?;
            product_metadata.validate_structure()?;
            crate::repo::write_target_product_metadata(
                &repo_paths,
                target_id,
                Some(&product_metadata),
            )?;
        } else {
            crate::repo::write_target_product_metadata(&repo_paths, target_id, None)?;
        }
        let blueprint_bytes = download_binary(&client, &blueprint_url)?;
        crate::helpers::write(
            &crate::target_blueprint_path(&repo_paths, target_id),
            &blueprint_bytes,
        )?;
        stats.target_downloaded_count += 1;
    }

    step += 1;
    emit(
        &progress,
        GutchOperation::Pull,
        GutchProgressPhase::Bundle,
        "Resolving bundles".to_string(),
        step,
        total_steps,
        None,
        false,
    );
    let bundles_by_target = required_bundle_ids_by_target(&repo_paths)?;
    let required_set: HashSet<String> = bundles_by_target
        .iter()
        .flat_map(|(_, ids)| ids.iter())
        .cloned()
        .collect();
    let mut missing_bundles: Vec<String> = required_set
        .iter()
        .filter(|bundle_id| !crate::bundle_path(&repo_paths, bundle_id).is_file())
        .cloned()
        .collect();
    missing_bundles.sort();

    let mut download_set = BundleDownloadSet::default();
    for (target_id, bundle_ids) in &bundles_by_target {
        let missing_for_target: Vec<String> = bundle_ids
            .iter()
            .filter(|id| missing_bundles.contains(id))
            .cloned()
            .collect();
        if missing_for_target.is_empty() {
            continue;
        }
        let fetched = fetch_bundle_download_urls(
            &client,
            &endpoint,
            &session_id,
            record.product_id,
            record.remote_repo_id,
            target_id,
            &missing_for_target,
        )?;
        download_set.urls.extend(fetched.urls);
    }

    let mut bundle_downloaded = 0usize;
    for bundle_id in &missing_bundles {
        control.checkpoint()?;
        let Some(url) = download_set.urls.get(bundle_id) else {
            continue;
        };
        let path = crate::bundle_path(&repo_paths, bundle_id);
        emit(
            &progress,
            GutchOperation::Pull,
            GutchProgressPhase::Bundle,
            format!(
                "Downloading bundles ({}/{})",
                bundle_downloaded + 1,
                missing_bundles.len().max(1)
            ),
            step,
            total_steps,
            Some(bundle_id.clone()),
            false,
        );
        download_to_path(&client, url, Some(&session_id), &path)?;
        bundle_downloaded += 1;
    }
    stats.bundle_downloaded_count = bundle_downloaded;
    stats.bundle_skipped_count = required_set.len().saturating_sub(bundle_downloaded);

    let mut pruned_target_count = 0usize;
    let mut pruned_bundle_count = 0usize;
    if prune {
        control.checkpoint()?;
        step += 1;
        emit(
            &progress,
            GutchOperation::Pull,
            GutchProgressPhase::Prune,
            "Removing local-only content".to_string(),
            step,
            total_steps,
            None,
            false,
        );
        for target_id in &plan.local_only_targets {
            control.checkpoint()?;
            crate::repo::cleanup::remove_target_from_repo(&repo, target_id)?;
            pruned_target_count += 1;
        }
        let local_bundles = collect_local_bundle_paths(&repo_paths.bundles_root)?;
        for path in local_bundles {
            control.checkpoint()?;
            if let Some(bundle_id) = crate::bundle_id_from_bundle_path(&path)
                && !required_set.contains(&bundle_id)
            {
                std::fs::remove_file(&path)?;
                pruned_bundle_count += 1;
            }
        }
    } else {
        stats.local_only_targets_kept = plan.local_only_targets.len();
    }
    stats.pruned_target_count = pruned_target_count;
    stats.pruned_bundle_count = pruned_bundle_count;

    emit(
        &progress,
        GutchOperation::Pull,
        GutchProgressPhase::Done,
        "Pull complete".to_string(),
        total_steps,
        total_steps,
        None,
        true,
    );

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mtime_ago(seconds: u64) -> std::time::SystemTime {
        std::time::SystemTime::now() - std::time::Duration::from_secs(seconds)
    }

    #[test]
    fn parses_remote_spec() {
        let (product, repo) = parse_remote_spec("Skyforge/windows-x86_64").unwrap();
        assert_eq!(product, "Skyforge");
        assert_eq!(repo, "windows-x86_64");
    }

    #[test]
    fn rejects_malformed_remote_specs() {
        for spec in ["no-slash", "/repo", "product/", "p/r/extra", "p//r"] {
            assert!(parse_remote_spec(spec).is_err(), "accepted {spec}");
        }
    }

    #[test]
    fn parses_conflict_policies() {
        assert_eq!(
            PullConflictPolicy::parse("keep-local").unwrap(),
            PullConflictPolicy::KeepLocal
        );
        assert_eq!(
            PullConflictPolicy::parse("keep-remote").unwrap(),
            PullConflictPolicy::KeepRemote
        );
        assert_eq!(
            PullConflictPolicy::parse("keep-newer").unwrap(),
            PullConflictPolicy::KeepNewer
        );
        assert!(PullConflictPolicy::parse("overwrite").is_err());
    }

    #[test]
    fn hashes_differ_rules() {
        assert!(!hashes_differ("", ""));
        assert!(!hashes_differ("abc", "abc"));
        assert!(hashes_differ("abc", "def"));
        assert!(hashes_differ("", "def"));
        assert!(hashes_differ("abc", ""));
    }

    #[test]
    fn identical_hashes_are_never_fetched() {
        for policy in [
            PullConflictPolicy::KeepLocal,
            PullConflictPolicy::KeepRemote,
            PullConflictPolicy::KeepNewer,
        ] {
            let decision = decide_artifact(
                true,
                "same".to_string(),
                "same",
                "2026-08-08T10:00:00Z",
                Some(mtime_ago(100)),
                policy,
            );
            assert_eq!(decision, ArtifactDecision::Identical);
        }
    }

    #[test]
    fn keep_local_never_fetches_existing_content() {
        let decision = decide_artifact(
            true,
            "local".to_string(),
            "remote",
            "2026-08-08T10:00:00Z",
            Some(mtime_ago(100)),
            PullConflictPolicy::KeepLocal,
        );
        assert_eq!(decision, ArtifactDecision::KeepLocalNewer);
        // Missing content is still fetched even under keep-local.
        let missing = decide_artifact(
            false,
            String::new(),
            "remote",
            "2026-08-08T10:00:00Z",
            None,
            PullConflictPolicy::KeepLocal,
        );
        assert_eq!(missing, ArtifactDecision::Fetch);
    }

    #[test]
    fn keep_remote_fetches_even_when_local_is_newer() {
        let decision = decide_artifact(
            true,
            "local".to_string(),
            "remote",
            "2026-08-08T10:00:00Z",
            Some(mtime_ago(100)),
            PullConflictPolicy::KeepRemote,
        );
        assert_eq!(decision, ArtifactDecision::Fetch);
    }

    #[test]
    fn keep_newer_uses_remote_updated_at() {
        let local_newer = decide_artifact(
            true,
            "local".to_string(),
            "remote",
            "2026-08-01T10:00:00Z", // remote older than local mtime
            Some(mtime_ago(100)),
            PullConflictPolicy::KeepNewer,
        );
        assert_eq!(local_newer, ArtifactDecision::KeepLocalNewer);

        let remote_newer = decide_artifact(
            true,
            "local".to_string(),
            "remote",
            "2030-08-01T10:00:00Z", // remote in the future
            Some(mtime_ago(100)),
            PullConflictPolicy::KeepNewer,
        );
        assert_eq!(remote_newer, ArtifactDecision::Fetch);
    }

    #[test]
    fn unparseable_remote_timestamp_fetches() {
        let decision = decide_artifact(
            true,
            "local".to_string(),
            "remote",
            "not-a-timestamp",
            Some(mtime_ago(100)),
            PullConflictPolicy::KeepNewer,
        );
        assert_eq!(decision, ArtifactDecision::Fetch);
    }
}
