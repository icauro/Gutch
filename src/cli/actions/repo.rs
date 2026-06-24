use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::cli::common::{load_optional_rules, parse_version_parts, session_id_for_cli};
use g::remote::backend::{
    CreatorRepoTargetsResponse, CreatorTargetResponse, UpsertTargetResponse,
    request_json_from_backend, upload_raw_file,
};
use gutch as g;

#[derive(Debug, Serialize)]
pub struct RepoSyncUpStats {
    pub command: &'static str,
    pub repo_root: String,
    pub product_id: i64,
    pub gutch_repo_id: i64,
    pub repo_manifest_uploaded: bool,
    pub chunk_index_uploaded: bool,
    pub target_manifest_uploaded_count: usize,
    pub bundle_uploaded_count: usize,
    pub bundle_skipped_count: usize,
    pub target_count: usize,
    pub bundle_count: usize,
    pub target_added_count: usize,
    pub target_removed_count: usize,
    pub target_updated_count: usize,
    pub target_unchanged_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoSyncTargetChange {
    pub target_id: String,
    pub version: String,
    pub platform: String,
    pub architecture: String,
    pub tier: String,
    pub artifact_state: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoSyncPlan {
    pub repo_root: String,
    pub product_id: i64,
    pub gutch_repo_id: i64,
    pub target_count: usize,
    pub remote_target_count: usize,
    pub to_add: Vec<RepoSyncTargetChange>,
    pub to_remove: Vec<RepoSyncTargetChange>,
    pub to_update: Vec<RepoSyncTargetChange>,
    pub unchanged: Vec<RepoSyncTargetChange>,
}

#[derive(Debug, Clone)]
struct LocalSyncTarget {
    target_id: String,
    version: String,
    platform: String,
    architecture: String,
    tier: String,
    tags: Vec<String>,
}

fn local_target_change(
    target: &LocalSyncTarget,
    artifact_state: Option<String>,
) -> RepoSyncTargetChange {
    RepoSyncTargetChange {
        target_id: target.target_id.clone(),
        version: target.version.clone(),
        platform: target.platform.clone(),
        architecture: target.architecture.clone(),
        tier: target.tier.clone(),
        artifact_state,
        tags: target.tags.clone(),
    }
}

fn remote_target_change(target: &CreatorTargetResponse) -> RepoSyncTargetChange {
    RepoSyncTargetChange {
        target_id: target.target_id.clone(),
        version: target
            .target_tags
            .exclusive
            .get("version")
            .cloned()
            .unwrap_or_default(),
        platform: target
            .target_tags
            .exclusive
            .get("os")
            .cloned()
            .unwrap_or_default(),
        architecture: target
            .target_tags
            .exclusive
            .get("arch")
            .cloned()
            .unwrap_or_default(),
        tier: target
            .target_tags
            .exclusive
            .get("tier")
            .cloned()
            .unwrap_or_default(),
        artifact_state: Some(target.artifact_state.clone()),
        tags: remote_target_tags(target),
    }
}

fn remote_target_tags(target: &CreatorTargetResponse) -> Vec<String> {
    let mut values = Vec::new();
    values.extend(target.target_tags.plain.iter().cloned());
    for (key, scoped_values) in &target.target_tags.scoped {
        for value in scoped_values {
            values.push(format!("{}:{}", key, value));
        }
    }
    for (key, value) in &target.target_tags.exclusive {
        values.push(format!("{}::{}", key, value));
    }
    values.sort();
    values
}

fn resolve_sync_binding(
    repo: &PathBuf,
    product_id: Option<i64>,
    gutch_repo_id: Option<i64>,
) -> g::Result<(i64, i64)> {
    if let (Some(pid), Some(cid)) = (product_id, gutch_repo_id) {
        return Ok((pid, cid));
    }

    let config = g::config::load_config()?;
    let tracked = config
        .find_repo(&repo.display().to_string())
        .ok_or_else(|| {
            g::Error::manifest(format!(
                "repo '{}' is not tracked. Use repo link first or pass --product-id and --gutch-repo-id.",
                repo.display()
            ))
        })?;
    let linked = tracked.linked.as_ref().ok_or_else(|| {
        g::Error::manifest(format!(
            "repo '{}' has no linked remote. Use repo link first or pass --product-id and --gutch-repo-id.",
            repo.display()
        ))
    })?;
    let pid = linked.product_id.parse::<i64>().map_err(|_| {
        g::Error::manifest(format!("invalid linked product_id: {}", linked.product_id))
    })?;
    Ok((pid, linked.remote_repo_id))
}

fn fetch_creator_targets(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    session_id: &str,
    product_id: i64,
    gutch_repo_id: i64,
) -> g::Result<HashMap<String, CreatorTargetResponse>> {
    let creator_targets: CreatorRepoTargetsResponse = request_json_from_backend(
        client
            .get(format!(
                "{}/v1/users/me/creator/products/{}/repos/{}/targets",
                endpoint.trim_end_matches('/'),
                product_id,
                gutch_repo_id
            ))
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", session_id)),
    )?;

    let mut remote_targets = HashMap::new();
    for tier in creator_targets.tier_labels {
        for target in tier.targets {
            remote_targets.insert(target.target_id.clone(), target);
        }
    }
    Ok(remote_targets)
}

fn collect_local_sync_targets(
    repo: &PathBuf,
) -> g::Result<(
    g::RepoInspectStatus,
    g::GutchRepoPaths,
    Vec<LocalSyncTarget>,
)> {
    let status = g::inspect_gutch_repo(repo)?;
    if !status.is_repo {
        return Err(g::Error::manifest(format!(
            "no gutch repo found at {}",
            repo.display()
        )));
    }
    let (repo_paths, _) = g::load_gutch_repo_manifest(repo)?;
    let mut local_targets = Vec::with_capacity(status.targets.len());
    for target in &status.targets {
        let manifest = g::load_target_manifest(&repo_paths, &target.target_id)?;
        let mut tags = manifest.target_tags.to_canonical_strings();
        tags.sort();
        local_targets.push(LocalSyncTarget {
            target_id: manifest.target_id.clone(),
            version: manifest
                .target_tags
                .get_exclusive("version")
                .unwrap_or_default()
                .to_string(),
            platform: manifest
                .target_tags
                .get_exclusive("os")
                .unwrap_or_default()
                .to_string(),
            architecture: manifest
                .target_tags
                .get_exclusive("arch")
                .unwrap_or_default()
                .to_string(),
            tier: manifest
                .target_tags
                .get_exclusive("tier")
                .unwrap_or_default()
                .to_string(),
            tags,
        });
    }
    Ok((status, repo_paths, local_targets))
}

pub fn plan_sync_up_repo(
    repo: PathBuf,
    endpoint: String,
    product_id: Option<i64>,
    gutch_repo_id: Option<i64>,
    session_override: Option<String>,
) -> g::Result<RepoSyncPlan> {
    let (product_id, gutch_repo_id) = resolve_sync_binding(&repo, product_id, gutch_repo_id)?;
    let session_id = session_id_for_cli(session_override)?;
    let client = reqwest::blocking::Client::builder()
        .user_agent("Gutch CLI")
        .build()
        .map_err(|err| g::Error::manifest(format!("failed to create sync client: {}", err)))?;

    let (status, repo_paths, local_targets) = collect_local_sync_targets(&repo)?;
    let remote_targets =
        fetch_creator_targets(&client, &endpoint, &session_id, product_id, gutch_repo_id)?;

    let mut to_add = Vec::new();
    let mut to_update = Vec::new();
    let mut unchanged = Vec::new();
    for local_target in &local_targets {
        match remote_targets.get(&local_target.target_id) {
            None => to_add.push(local_target_change(local_target, None)),
            Some(remote_target) => {
                let tag_mismatch = local_target.tags != remote_target_tags(remote_target);
                let needs_state_update = remote_target.artifact_state != "ready";
                if tag_mismatch || needs_state_update {
                    to_update.push(local_target_change(
                        local_target,
                        Some(remote_target.artifact_state.clone()),
                    ));
                } else {
                    unchanged.push(local_target_change(
                        local_target,
                        Some(remote_target.artifact_state.clone()),
                    ));
                }
            }
        }
    }

    let mut to_remove = Vec::new();
    for remote_target in remote_targets.values() {
        if !local_targets
            .iter()
            .any(|local_target| local_target.target_id == remote_target.target_id)
        {
            to_remove.push(remote_target_change(remote_target));
        }
    }

    to_add.sort_by(|left, right| left.target_id.cmp(&right.target_id));
    to_update.sort_by(|left, right| left.target_id.cmp(&right.target_id));
    unchanged.sort_by(|left, right| left.target_id.cmp(&right.target_id));
    to_remove.sort_by(|left, right| left.target_id.cmp(&right.target_id));

    Ok(RepoSyncPlan {
        repo_root: repo_paths.repo_root.display().to_string(),
        product_id,
        gutch_repo_id,
        target_count: status.targets.len(),
        remote_target_count: remote_targets.len(),
        to_add,
        to_remove,
        to_update,
        unchanged,
    })
}

pub fn init_repo(path: &Path, schema: Option<&PathBuf>) -> g::Result<()> {
    let repo_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("base");
    let repo_paths = g::GutchRepoPaths::from_repo_root(path);
    let mut repo_manifest = g::ensure_gutch_repo_layout(&repo_paths, repo_name)?;
    repo_manifest.target_schema = load_optional_rules(&schema.cloned())?;
    g::write_gutch_repo_manifest(&repo_paths, &repo_manifest)?;

    println!(
        "created gutch repo {} at {}",
        repo_name,
        repo_paths.repo_root.display()
    );

    let mut config = g::config::load_config()?;
    config.upsert_repo(g::config::TrackedRepo {
        path: repo_paths.repo_root.display().to_string(),
        target_lookup_path: None,
        linked: None,
    });
    g::config::save_config(&config)?;
    Ok(())
}

pub fn unlink_repo(repo: &Path) -> g::Result<()> {
    let mut config = g::config::load_config()?;
    let repo_path = repo.display().to_string();
    if !config.clear_repo_link(&repo_path) {
        return Err(g::Error::manifest(format!(
            "repo '{}' is not tracked in config.",
            repo_path
        )));
    }
    g::config::save_config(&config)?;
    println!("unlinked repo '{}'", repo.display());
    Ok(())
}

pub fn sync_up_repo(
    repo: PathBuf,
    endpoint: String,
    product_id: Option<i64>,
    gutch_repo_id: Option<i64>,
    session_override: Option<String>,
) -> g::Result<RepoSyncUpStats> {
    let (product_id, gutch_repo_id) = resolve_sync_binding(&repo, product_id, gutch_repo_id)?;
    let session_id = session_id_for_cli(session_override)?;
    let client = reqwest::blocking::Client::builder()
        .user_agent("Gutch CLI")
        .build()
        .map_err(|err| g::Error::manifest(format!("failed to create sync client: {}", err)))?;

    let plan = plan_sync_up_repo(
        repo.clone(),
        endpoint.clone(),
        Some(product_id),
        Some(gutch_repo_id),
        Some(session_id.clone()),
    )?;
    let (status, repo_paths, _) = collect_local_sync_targets(&repo)?;
    let mut remote_targets =
        fetch_creator_targets(&client, &endpoint, &session_id, product_id, gutch_repo_id)?;

    for target in &plan.to_remove {
        if let Some(remote_target) = remote_targets.remove(&target.target_id) {
            let response = client
                .delete(format!(
                    "{}/v1/users/me/creator/products/{}/repos/{}/targets/{}",
                    endpoint.trim_end_matches('/'),
                    product_id,
                    gutch_repo_id,
                    remote_target.id
                ))
                .header("Accept", "application/json")
                .header("Authorization", format!("Bearer {}", session_id))
                .send()
                .map_err(|err| g::Error::manifest(format!("backend request failed: {}", err)))?;
            if !response.status().is_success() {
                return Err(g::Error::manifest(
                    g::remote::backend::parse_backend_error_response(response),
                ));
            }
        }
    }

    let repo_manifest_bytes = crate::helpers::read(&repo_paths.repo_manifest_path)?;
    upload_raw_file(
        &client,
        &endpoint,
        &format!(
            "/v1/users/me/creator/products/{}/repos/{}/manifest",
            product_id, gutch_repo_id
        ),
        &session_id,
        repo_manifest_bytes,
        "application/json",
    )?;
    let repo_manifest_uploaded = true;

    let mut chunk_index_uploaded = false;
    if repo_paths.chunk_index_path.exists() {
        let chunk_index = g::load_chunk_index(&repo_paths)?;
        let chunk_index_bytes = serde_json::to_vec_pretty(&chunk_index)?;
        upload_raw_file(
            &client,
            &endpoint,
            &format!(
                "/v1/users/me/creator/products/{}/repos/{}/chunks",
                product_id, gutch_repo_id
            ),
            &session_id,
            chunk_index_bytes,
            "application/json",
        )?;
        chunk_index_uploaded = true;
    }

    let mut target_manifest_uploaded_count = 0usize;
    for target in &status.targets {
        let manifest = g::load_target_manifest(&repo_paths, &target.target_id)?;
        let manifest_bytes = g::serialize_target_manifest_for_remote(&manifest)?;
        let version = manifest
            .target_tags
            .get_exclusive("version")
            .ok_or_else(|| {
                g::Error::manifest(format!(
                    "target '{}' is missing version tag",
                    manifest.target_id
                ))
            })?;
        let (version_major, version_minor, version_patch) = parse_version_parts(version)?;
        let remote_target_id = match remote_targets
            .get(&manifest.target_id)
            .map(|target| target.id)
        {
            Some(id) => id,
            None => {
                let created: UpsertTargetResponse = request_json_from_backend(
                    client
                        .post(format!(
                            "{}/v1/users/me/creator/products/{}/repos/{}/targets",
                            endpoint.trim_end_matches('/'),
                            product_id,
                            gutch_repo_id
                        ))
                        .header("Accept", "application/json")
                        .header("Authorization", format!("Bearer {}", session_id))
                        .json(&serde_json::json!({
                            "targetId": manifest.target_id,
                            "targetTags": manifest.target_tags,
                            "versionMajor": version_major,
                            "versionMinor": version_minor,
                            "versionPatch": version_patch,
                        })),
                )?;
                remote_targets.insert(
                    manifest.target_id.clone(),
                    CreatorTargetResponse {
                        id: created.id,
                        target_id: manifest.target_id.clone(),
                        target_tags: Default::default(),
                        artifact_state: String::from("pending"),
                    },
                );
                created.id
            }
        };

        upload_raw_file(
            &client,
            &endpoint,
            &format!(
                "/v1/users/me/creator/products/{}/repos/{}/targets/{}/manifest",
                product_id, gutch_repo_id, remote_target_id
            ),
            &session_id,
            manifest_bytes,
            "application/json",
        )?;
        target_manifest_uploaded_count += 1;
    }

    let mut bundle_uploaded_count = 0usize;
    let bundle_skipped_count = 0usize;
    for entry in walkdir::WalkDir::new(&repo_paths.bundles_root).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file() {
            continue;
        }
        let Some(bundle_id) = g::bundle_id_from_bundle_path(entry.path()) else {
            continue;
        };

        upload_raw_file(
            &client,
            &endpoint,
            &format!(
                "/v1/users/me/creator/products/{}/repos/{}/bundles/{}/content",
                product_id, gutch_repo_id, bundle_id
            ),
            &session_id,
            crate::helpers::read(entry.path())?,
            "application/octet-stream",
        )?;
        bundle_uploaded_count += 1;
    }

    Ok(RepoSyncUpStats {
        command: "repo sync-up",
        repo_root: repo_paths.repo_root.display().to_string(),
        product_id,
        gutch_repo_id,
        repo_manifest_uploaded,
        chunk_index_uploaded,
        target_manifest_uploaded_count,
        bundle_uploaded_count,
        bundle_skipped_count,
        target_count: status.targets.len(),
        bundle_count: status.bundle_count,
        target_added_count: plan.to_add.len(),
        target_removed_count: plan.to_remove.len(),
        target_updated_count: plan.to_update.len(),
        target_unchanged_count: plan.unchanged.len(),
    })
}
