use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::Result;
use crate::progress::{self, GutchOperation, GutchProgressEvent, GutchProgressPhase};
use crate::remote::backend::{
    CreatorRepoResponse, CreatorRepoTargetsResponse, CreatorReposResponse, CreatorTargetResponse,
    UpsertTargetResponse, request_json_from_backend, upload_raw_file,
};
use crate::repo::target_descriptor::summarize_target_tags;
#[derive(Debug, Serialize)]
pub struct RepoSyncUpStats {
    pub command: &'static str,
    pub repo_root: String,
    pub product_id: i64,
    pub remote_repo_id: i64,
    pub repo_manifest_uploaded: bool,
    pub chunk_index_uploaded: bool,
    pub target_manifest_uploaded_count: usize,
    pub target_blueprint_uploaded_count: usize,
    pub bundle_uploaded_count: usize,
    pub bundle_skipped_count: usize,
    pub remote_bundle_deleted_count: usize,
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
    pub os: String,
    pub arch: String,
    pub artifact_state: Option<String>,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoSyncPlan {
    pub repo_root: String,
    pub product_id: i64,
    pub remote_repo_id: i64,
    pub repo_manifest_changed: bool,
    pub chunk_index_changed: bool,
    pub target_count: usize,
    pub remote_target_count: usize,
    pub to_add: Vec<RepoSyncTargetChange>,
    pub to_remove: Vec<RepoSyncTargetChange>,
    pub to_update: Vec<RepoSyncTargetChange>,
    pub unchanged: Vec<RepoSyncTargetChange>,
}

#[derive(Debug, Clone)]
struct SyncTargetSnapshot {
    target_id: String,
    version: String,
    os: String,
    arch: String,
    tags: Vec<String>,
    target_metadata_hash_b3: String,
    target_blueprint_hash_b3: String,
}

fn sync_target_change(
    target: &SyncTargetSnapshot,
    artifact_state: Option<String>,
    update_reason: Option<String>,
) -> RepoSyncTargetChange {
    RepoSyncTargetChange {
        target_id: target.target_id.clone(),
        version: target.version.clone(),
        os: target.os.clone(),
        arch: target.arch.clone(),
        artifact_state,
        tags: target.tags.clone(),
        update_reason,
    }
}

fn remote_target_change(target: &CreatorTargetResponse) -> RepoSyncTargetChange {
    let (target_snapshot, artifact_state) = remote_sync_target_snapshot(target);
    sync_target_change(&target_snapshot, artifact_state, None)
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

fn remote_sync_target_snapshot(
    target: &CreatorTargetResponse,
) -> (SyncTargetSnapshot, Option<String>) {
    (
        SyncTargetSnapshot {
            target_id: target.target_id.clone(),
            version: target
                .target_tags
                .exclusive
                .get("version")
                .cloned()
                .unwrap_or_default(),
            os: target
                .target_tags
                .exclusive
                .get("os")
                .cloned()
                .unwrap_or_default(),
            arch: target
                .target_tags
                .exclusive
                .get("arch")
                .cloned()
                .unwrap_or_default(),
            tags: remote_target_tags(target),
            target_metadata_hash_b3: target.target_metadata_hash_b3.clone(),
            target_blueprint_hash_b3: target.target_blueprint_hash_b3.clone(),
        },
        Some(target.artifact_state.clone()),
    )
}

fn repo_file_hash(bytes: &[u8]) -> String {
    crate::hash::hash_hex(&blake3::hash(bytes))
}

fn local_repo_metadata_hash_b3(repo_paths: &crate::GutchRepoPaths) -> Result<String> {
    Ok(repo_file_hash(&crate::helpers::read(
        &repo_paths.repo_metadata_path,
    )?))
}

fn local_chunk_index_hash_b3(repo_paths: &crate::GutchRepoPaths) -> Result<String> {
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
    let manifest = crate::load_target_manifest(repo_paths, target_id)?;
    Ok(repo_file_hash(
        &crate::serialize_target_manifest_for_remote(&manifest)?,
    ))
}

fn local_target_blueprint_hash_b3(
    repo_paths: &crate::GutchRepoPaths,
    target_id: &str,
) -> Result<String> {
    let hash = crate::hash::hash_file(&crate::target_blueprint_path(repo_paths, target_id))?;
    Ok(crate::hash::hash_hex(&hash))
}

fn hashes_differ(local_hash: &str, remote_hash: &str) -> bool {
    if local_hash.trim().is_empty() && remote_hash.trim().is_empty() {
        return false;
    }
    let remote_hash = remote_hash.trim();
    remote_hash.is_empty() || local_hash != remote_hash
}

fn resolve_sync_binding(repo: &Path) -> Result<(i64, i64)> {
    let (paths, manifest) = crate::load_gutch_repo_manifest(repo)?;
    if !manifest.is_linked() {
        return Err(crate::Error::manifest(format!(
            "repo '{}' is not linked. Use 'gutch repo link' first.",
            paths.repo_root.display()
        )));
    }
    let product_id = manifest.product_id.parse::<i64>().map_err(|_| {
        crate::Error::manifest(format!(
            "invalid product_id '{}' in manifest",
            manifest.product_id
        ))
    })?;
    let remote_repo_id = manifest.remote_repo_id_value().ok_or_else(|| {
        crate::Error::manifest(format!(
            "invalid remote_repo_id '{}' in manifest",
            manifest.remote_repo_id
        ))
    })?;
    Ok((product_id, remote_repo_id))
}

fn fetch_creator_targets(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    session_id: &str,
    product_id: i64,
    remote_repo_id: i64,
) -> Result<HashMap<String, CreatorTargetResponse>> {
    let creator_targets: CreatorRepoTargetsResponse = request_json_from_backend(
        client
            .get(format!(
                "{}/v1/users/me/creator/products/{}/repos/{}/targets",
                endpoint.trim_end_matches('/'),
                product_id,
                remote_repo_id
            ))
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", session_id)),
    )?;

    let mut remote_targets = HashMap::new();
    if !creator_targets.targets.is_empty() {
        for target in creator_targets.targets {
            remote_targets.insert(target.target_id.clone(), target);
        }
    } else {
        for tier in creator_targets.tier_labels {
            for target in tier.targets {
                remote_targets.insert(target.target_id.clone(), target);
            }
        }
    }
    Ok(remote_targets)
}

fn fetch_creator_repo(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    session_id: &str,
    product_id: i64,
    remote_repo_id: i64,
) -> Result<CreatorRepoResponse> {
    let creator_repos: CreatorReposResponse = request_json_from_backend(
        client
            .get(format!(
                "{}/v1/users/me/creator/products/{}/repos",
                endpoint.trim_end_matches('/'),
                product_id
            ))
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", session_id)),
    )?;
    creator_repos
        .repos
        .into_iter()
        .find(|repo| repo.id == remote_repo_id)
        .ok_or_else(|| {
            crate::Error::manifest(format!(
                "remote repo {} not found for product {}",
                remote_repo_id, product_id
            ))
        })
}

fn remote_bundle_exists(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    session_id: &str,
    product_id: i64,
    remote_repo_id: i64,
    bundle_id: &str,
) -> Result<bool> {
    let response = client
        .get(format!(
            "{}/v1/users/me/creator/products/{}/repos/{}/bundles/{}",
            endpoint.trim_end_matches('/'),
            product_id,
            remote_repo_id,
            bundle_id
        ))
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {}", session_id))
        .send()
        .map_err(|err| crate::Error::manifest(format!("backend request failed: {}", err)))?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    if !response.status().is_success() {
        return Err(crate::Error::manifest(
            crate::remote::backend::parse_backend_error_response(response),
        ));
    }
    Ok(true)
}

fn collect_local_sync_targets(
    repo: &Path,
) -> Result<(
    crate::RepoCheckStatus,
    crate::GutchRepoPaths,
    Vec<SyncTargetSnapshot>,
)> {
    let status = crate::check_gutch_repo(repo)?;
    if !status.is_repo {
        return Err(crate::Error::manifest(format!(
            "no gutch repo found at {}",
            repo.display()
        )));
    }
    let (repo_paths, _) = crate::load_gutch_repo_manifest(repo)?;
    let mut local_targets = Vec::with_capacity(status.targets.len());
    for target in &status.targets {
        let manifest = crate::load_target_manifest(&repo_paths, &target.target_id)?;
        let summary = summarize_target_tags(&manifest.target_tags, None);
        local_targets.push(SyncTargetSnapshot {
            target_id: manifest.target_id.clone(),
            version: summary.version_id,
            os: summary.os,
            arch: summary.arch,
            tags: summary.canonical_tags,
            target_metadata_hash_b3: local_target_metadata_hash_b3(
                &repo_paths,
                &manifest.target_id,
            )?,
            target_blueprint_hash_b3: local_target_blueprint_hash_b3(
                &repo_paths,
                &manifest.target_id,
            )?,
        });
    }
    Ok((status, repo_paths, local_targets))
}

pub fn plan_sync_up_repo(
    repo: PathBuf,
    endpoint: String,
    session_id: String,
) -> Result<RepoSyncPlan> {
    let (product_id, remote_repo_id) = resolve_sync_binding(&repo)?;
    let client = reqwest::blocking::Client::builder()
        .user_agent("Gutch CLI")
        .build()
        .map_err(|err| crate::Error::manifest(format!("failed to create sync client: {}", err)))?;

    let (status, repo_paths, local_targets) = collect_local_sync_targets(&repo)?;
    let remote_repo =
        fetch_creator_repo(&client, &endpoint, &session_id, product_id, remote_repo_id)?;
    let remote_targets =
        fetch_creator_targets(&client, &endpoint, &session_id, product_id, remote_repo_id)?;
    let repo_manifest_changed = hashes_differ(
        &local_repo_metadata_hash_b3(&repo_paths)?,
        &remote_repo.repo_metadata_hash_b3,
    );
    let chunk_index_changed = hashes_differ(
        &local_chunk_index_hash_b3(&repo_paths)?,
        &remote_repo.chunk_map_hash_b3,
    );

    let mut to_add = Vec::new();
    let mut to_update = Vec::new();
    let mut unchanged = Vec::new();
    for local_target in &local_targets {
        match remote_targets.get(&local_target.target_id) {
            None => to_add.push(sync_target_change(local_target, None, None)),
            Some(remote_target) => {
                let tag_mismatch = local_target.tags != remote_target_tags(remote_target);
                let needs_state_update = remote_target.artifact_state != "ready";
                let metadata_hash_changed = hashes_differ(
                    &local_target.target_metadata_hash_b3,
                    &remote_target.target_metadata_hash_b3,
                );
                let target_blueprint_hash_changed = hashes_differ(
                    &local_target.target_blueprint_hash_b3,
                    &remote_target.target_blueprint_hash_b3,
                );
                if tag_mismatch
                    || needs_state_update
                    || metadata_hash_changed
                    || target_blueprint_hash_changed
                {
                    let mut reasons = Vec::new();
                    if tag_mismatch {
                        reasons.push("target tags differ from remote");
                    }
                    if needs_state_update {
                        reasons.push("remote artifact state is not ready");
                    }
                    if metadata_hash_changed {
                        reasons.push("target metadata hash differs");
                    }
                    if target_blueprint_hash_changed {
                        reasons.push("target blueprint hash differs");
                    }
                    to_update.push(sync_target_change(
                        local_target,
                        Some(remote_target.artifact_state.clone()),
                        Some(format!("{}.", reasons.join("; "))),
                    ));
                } else {
                    unchanged.push(sync_target_change(
                        local_target,
                        Some(remote_target.artifact_state.clone()),
                        None,
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
        remote_repo_id,
        repo_manifest_changed,
        chunk_index_changed,
        target_count: status.targets.len(),
        remote_target_count: remote_targets.len(),
        to_add,
        to_remove,
        to_update,
        unchanged,
    })
}

pub fn link_repo(repo: &Path, product_id: i64, endpoint: &str, session_id: String) -> Result<()> {
    let (paths, mut manifest) = crate::load_gutch_repo_manifest(repo)?;
    if manifest.is_linked() {
        return Err(crate::Error::manifest(format!(
            "repo '{}' is already linked (remote_repo_id={}, product_id={}). Unlink first.",
            paths.repo_root.display(),
            manifest.remote_repo_id,
            manifest.product_id
        )));
    }

    let client = reqwest::blocking::Client::builder()
        .user_agent("Gutch CLI")
        .build()
        .map_err(|err| crate::Error::manifest(format!("failed to create link client: {}", err)))?;

    let repo_name = manifest.name.clone();
    let response = client
        .post(format!(
            "{}/v1/users/me/creator/products/{}/repos",
            endpoint.trim_end_matches('/'),
            product_id
        ))
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {}", session_id))
        .json(&serde_json::json!({
            "name": repo_name,
        }))
        .send()
        .map_err(|err| crate::Error::manifest(format!("backend request failed: {}", err)))?;

    if response.status() == reqwest::StatusCode::CONFLICT {
        let body = response
            .text()
            .unwrap_or_else(|_| "repo name already taken".to_string());
        return Err(crate::Error::manifest(format!(
            "repo name '{}' already exists for product {}. {}",
            repo_name, product_id, body
        )));
    }

    if !response.status().is_success() {
        return Err(crate::Error::manifest(
            crate::remote::backend::parse_backend_error_response(response),
        ));
    }

    let created: serde_json::Value = response
        .json()
        .map_err(|err| crate::Error::manifest(format!("failed to parse link response: {}", err)))?;
    let remote_repo_id = created["id"]
        .as_i64()
        .ok_or_else(|| crate::Error::manifest("backend response missing repo id".to_string()))?;

    manifest.remote_repo_id = remote_repo_id.to_string();
    manifest.product_id = product_id.to_string();
    manifest.sync_product_metadata_from_binding();
    crate::write_gutch_repo_manifest(&paths, &manifest)?;

    Ok(())
}

pub fn unlink_repo(
    repo: &Path,
    delete_remote: bool,
    endpoint: Option<&str>,
    session_id: Option<String>,
) -> Result<()> {
    let (paths, mut manifest) = crate::load_gutch_repo_manifest(repo)?;
    if !manifest.is_linked() {
        return Ok(());
    }

    if delete_remote {
        let remote_repo_id = manifest.remote_repo_id_value().ok_or_else(|| {
            crate::Error::manifest(format!(
                "invalid remote_repo_id '{}' in manifest",
                manifest.remote_repo_id
            ))
        })?;
        let product_id = manifest.product_id.parse::<i64>().map_err(|_| {
            crate::Error::manifest(format!(
                "invalid product_id '{}' in manifest",
                manifest.product_id
            ))
        })?;

        let endpoint = endpoint.ok_or_else(|| {
            crate::Error::manifest("--endpoint required when using --delete-remote".to_string())
        })?;
        let session_id = session_id.ok_or_else(|| {
            crate::Error::manifest("--session-id required when using --delete-remote".to_string())
        })?;
        let client = reqwest::blocking::Client::builder()
            .user_agent("Gutch CLI")
            .build()
            .map_err(|err| crate::Error::manifest(format!("failed to create client: {}", err)))?;

        let response = client
            .delete(format!(
                "{}/v1/users/me/creator/products/{}/repos/{}",
                endpoint.trim_end_matches('/'),
                product_id,
                remote_repo_id
            ))
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", session_id))
            .send()
            .map_err(|err| crate::Error::manifest(format!("backend request failed: {}", err)))?;

        if !response.status().is_success() {
            return Err(crate::Error::manifest(
                crate::remote::backend::parse_backend_error_response(response),
            ));
        }
    }

    manifest.remote_repo_id = String::new();
    manifest.product_id = String::new();
    manifest.product_metadata = None;
    crate::write_gutch_repo_manifest(&paths, &manifest)?;

    Ok(())
}

fn collect_bundle_paths(bundles_root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in walkdir::WalkDir::new(bundles_root).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file() {
            continue;
        }
        if crate::bundle_id_from_bundle_path(entry.path()).is_some() {
            paths.push(entry.path().to_path_buf());
        }
    }
    Ok(paths)
}

fn collect_required_bundle_ids(
    repo_paths: &crate::GutchRepoPaths,
    status: &crate::RepoCheckStatus,
) -> Result<Vec<String>> {
    let chunk_index = crate::load_chunk_index(repo_paths)?;
    let mut required = std::collections::BTreeSet::new();
    for target in &status.targets {
        let manifest = crate::load_target_manifest(repo_paths, &target.target_id)?;
        required.extend(crate::repo::required_bundle_ids(&manifest, &chunk_index));
    }
    Ok(required.into_iter().collect())
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReconcileRepoBundlesResponse {
    #[serde(default)]
    deleted_bundle_ids: Vec<String>,
}

pub fn sync_up_repo(
    repo: PathBuf,
    endpoint: String,
    session_id: String,
    control: crate::GutchOperationControl,
) -> Result<RepoSyncUpStats> {
    control.checkpoint()?;
    let progress = control.progress().clone();
    let (product_id, remote_repo_id) = resolve_sync_binding(&repo)?;
    let client = reqwest::blocking::Client::builder()
        .user_agent("Gutch CLI")
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(15 * 60))
        .build()
        .map_err(|err| crate::Error::manifest(format!("failed to create sync client: {}", err)))?;

    progress::emit_progress(
        &progress,
        GutchProgressEvent {
            operation: GutchOperation::SyncUp,
            phase: GutchProgressPhase::Planning,
            message: "Planning sync changes".to_string(),
            current: 0,
            total: 1,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: false,
        },
    );

    let plan = plan_sync_up_repo(repo.clone(), endpoint.clone(), session_id.clone())?;
    control.checkpoint()?;
    let (status, repo_paths, _) = collect_local_sync_targets(&repo)?;
    let mut remote_targets =
        fetch_creator_targets(&client, &endpoint, &session_id, product_id, remote_repo_id)?;

    let bundle_paths = collect_bundle_paths(&repo_paths.bundles_root)?;
    let required_bundle_ids = collect_required_bundle_ids(&repo_paths, &status)?;
    let chunk_index_steps = usize::from(repo_paths.chunk_map_path.exists());
    let target_upload_count = plan.to_add.len() + plan.to_update.len();
    let total_steps = plan.to_remove.len()
        + 1
        + chunk_index_steps
        + target_upload_count
        + target_upload_count
        + bundle_paths.len()
        + 1;
    let mut step = 0usize;

    for target in &plan.to_remove {
        control.checkpoint()?;
        step += 1;
        progress::emit_progress(
            &progress,
            GutchProgressEvent {
                operation: GutchOperation::SyncUp,
                phase: GutchProgressPhase::RemoveTargets,
                message: "Removing remote targets".to_string(),
                current: step,
                total: total_steps,
                transferred_bytes: 0,
                total_bytes: 0,
                detail: Some(target.target_id.clone()),
                done: false,
            },
        );
        if let Some(remote_target) = remote_targets.remove(&target.target_id) {
            let response = client
                .delete(format!(
                    "{}/v1/users/me/creator/products/{}/repos/{}/targets/{}",
                    endpoint.trim_end_matches('/'),
                    product_id,
                    remote_repo_id,
                    remote_target.id
                ))
                .header("Accept", "application/json")
                .header("Authorization", format!("Bearer {}", session_id))
                .send()
                .map_err(|err| {
                    crate::Error::manifest(format!("backend request failed: {}", err))
                })?;
            if !response.status().is_success() {
                return Err(crate::Error::manifest(
                    crate::remote::backend::parse_backend_error_response(response),
                ));
            }
        }
    }

    step += 1;
    control.checkpoint()?;
    progress::emit_progress(
        &progress,
        GutchProgressEvent {
            operation: GutchOperation::SyncUp,
            phase: GutchProgressPhase::UploadRepoMetadata,
            message: "Uploading repo metadata".to_string(),
            current: step,
            total: total_steps,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: false,
        },
    );
    let repo_manifest_bytes = crate::helpers::read(&repo_paths.repo_metadata_path)?;
    let repo_manifest_uploaded = if plan.repo_manifest_changed {
        upload_raw_file(
            &client,
            &endpoint,
            &format!(
                "/v1/users/me/creator/products/{}/repos/{}/metadata",
                product_id, remote_repo_id
            ),
            &session_id,
            repo_manifest_bytes,
            "application/json",
            &control,
        )?;
        true
    } else {
        false
    };

    let mut chunk_index_uploaded = false;
    if repo_paths.chunk_map_path.exists() {
        control.checkpoint()?;
        step += 1;
        progress::emit_progress(
            &progress,
            GutchProgressEvent {
                operation: GutchOperation::SyncUp,
                phase: GutchProgressPhase::UploadChunkIndex,
                message: "Uploading chunk index".to_string(),
                current: step,
                total: total_steps,
                transferred_bytes: 0,
                total_bytes: 0,
                detail: None,
                done: false,
            },
        );
        let chunk_index = crate::load_chunk_index(&repo_paths)?;
        let chunk_index_bytes = serde_json::to_vec_pretty(&chunk_index)?;
        if plan.chunk_index_changed {
            upload_raw_file(
                &client,
                &endpoint,
                &format!(
                    "/v1/users/me/creator/products/{}/repos/{}/chunks",
                    product_id, remote_repo_id
                ),
                &session_id,
                chunk_index_bytes,
                "application/json",
                &control,
            )?;
            chunk_index_uploaded = true;
        }
    }

    let mut target_manifest_uploaded_count = 0usize;
    let mut target_blueprint_uploaded_count = 0usize;
    let target_ids_to_upload = plan
        .to_add
        .iter()
        .chain(plan.to_update.iter())
        .map(|change| change.target_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    for target in status
        .targets
        .iter()
        .filter(|target| target_ids_to_upload.contains(target.target_id.as_str()))
    {
        control.checkpoint()?;
        step += 1;
        progress::emit_progress(
            &progress,
            GutchProgressEvent {
                operation: GutchOperation::SyncUp,
                phase: GutchProgressPhase::UploadTargetMetadata,
                message: "Uploading target metadata".to_string(),
                current: step,
                total: total_steps,
                transferred_bytes: 0,
                total_bytes: 0,
                detail: Some(target.target_id.clone()),
                done: false,
            },
        );
        let manifest = crate::load_target_manifest(&repo_paths, &target.target_id)?;
        let manifest_bytes = crate::serialize_target_manifest_for_remote(&manifest)?;
        let version = manifest
            .target_tags
            .get_exclusive("version")
            .ok_or_else(|| {
                crate::Error::manifest(format!(
                    "target '{}' is missing version tag",
                    manifest.target_id
                ))
            })?;
        let structured_version =
            crate::manifest::StructuredVersion::parse(version).map_err(|_| {
                crate::Error::manifest(format!(
                    "target '{}' has invalid version '{}'",
                    manifest.target_id, version
                ))
            })?;
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
                            remote_repo_id
                        ))
                        .header("Accept", "application/json")
                        .header("Authorization", format!("Bearer {}", session_id))
                        .json(&serde_json::json!({
                            "targetId": manifest.target_id,
                            "targetTags": manifest.target_tags,
                            "versionMajor": structured_version.major,
                            "versionMinor": structured_version.minor,
                            "versionPatch": structured_version.patch,
                            "versionRevision": structured_version.revision,
                        })),
                )?;
                remote_targets.insert(
                    manifest.target_id.clone(),
                    CreatorTargetResponse {
                        id: created.id,
                        target_id: manifest.target_id.clone(),
                        target_tags: Default::default(),
                        artifact_state: String::from("pending"),
                        target_metadata_hash_b3: String::new(),
                        target_blueprint_hash_b3: String::new(),
                        uploaded_at: String::new(),
                        created_at: String::new(),
                    },
                );
                created.id
            }
        };

        upload_raw_file(
            &client,
            &endpoint,
            &format!(
                "/v1/users/me/creator/products/{}/repos/{}/targets/{}/metadata",
                product_id, remote_repo_id, remote_target_id
            ),
            &session_id,
            manifest_bytes,
            "application/json",
            &control,
        )?;
        target_manifest_uploaded_count += 1;

        control.checkpoint()?;
        step += 1;
        progress::emit_progress(
            &progress,
            GutchProgressEvent {
                operation: GutchOperation::SyncUp,
                phase: GutchProgressPhase::UploadTargetBlueprint,
                message: "Uploading target blueprints".to_string(),
                current: step,
                total: total_steps,
                transferred_bytes: 0,
                total_bytes: 0,
                detail: Some(target.target_id.clone()),
                done: false,
            },
        );
        let blueprint_bytes = crate::helpers::read(&crate::target_blueprint_path(
            &repo_paths,
            &target.target_id,
        ))?;
        upload_raw_file(
            &client,
            &endpoint,
            &format!(
                "/v1/users/me/creator/products/{}/repos/{}/targets/{}/blueprint",
                product_id, remote_repo_id, remote_target_id
            ),
            &session_id,
            blueprint_bytes,
            "application/octet-stream",
            &control,
        )?;
        target_blueprint_uploaded_count += 1;
    }

    let mut bundle_uploaded_count = 0usize;
    let mut bundle_skipped_count = 0usize;
    for bundle_path in bundle_paths {
        control.checkpoint()?;
        step += 1;
        let Some(bundle_id) = crate::bundle_id_from_bundle_path(&bundle_path) else {
            continue;
        };
        if remote_bundle_exists(
            &client,
            &endpoint,
            &session_id,
            product_id,
            remote_repo_id,
            &bundle_id,
        )? {
            bundle_skipped_count += 1;
            progress::emit_progress(
                &progress,
                GutchProgressEvent {
                    operation: GutchOperation::SyncUp,
                    phase: GutchProgressPhase::UploadBundle,
                    message: "Skipping bundles already present remotely".to_string(),
                    current: step,
                    total: total_steps,
                    transferred_bytes: 0,
                    total_bytes: 0,
                    detail: Some(bundle_id),
                    done: false,
                },
            );
            continue;
        }
        let bundle_bytes = crate::helpers::read(&bundle_path)?;
        let bundle_size = bundle_bytes.len() as u64;
        progress::emit_progress(
            &progress,
            GutchProgressEvent {
                operation: GutchOperation::SyncUp,
                phase: GutchProgressPhase::UploadBundle,
                message: "Uploading bundles".to_string(),
                current: step,
                total: total_steps,
                transferred_bytes: 0,
                total_bytes: bundle_size,
                detail: Some(bundle_id.clone()),
                done: false,
            },
        );

        upload_raw_file(
            &client,
            &endpoint,
            &format!(
                "/v1/users/me/creator/products/{}/repos/{}/bundles/{}/content",
                product_id, remote_repo_id, bundle_id
            ),
            &session_id,
            bundle_bytes,
            "application/octet-stream",
            &control,
        )?;
        bundle_uploaded_count += 1;
        progress::emit_progress(
            &progress,
            GutchProgressEvent {
                operation: GutchOperation::SyncUp,
                phase: GutchProgressPhase::UploadBundle,
                message: "Uploading bundles".to_string(),
                current: step,
                total: total_steps,
                transferred_bytes: bundle_size,
                total_bytes: bundle_size,
                detail: Some(bundle_id),
                done: false,
            },
        );
    }

    step += 1;
    control.checkpoint()?;
    progress::emit_progress(
        &progress,
        GutchProgressEvent {
            operation: GutchOperation::SyncUp,
            phase: GutchProgressPhase::Prune,
            message: "Removing unreferenced remote bundles".to_string(),
            current: step,
            total: total_steps,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: false,
        },
    );
    let reconcile: ReconcileRepoBundlesResponse = request_json_from_backend(
        client
            .put(format!(
                "{}/v1/users/me/creator/products/{}/repos/{}/bundles",
                endpoint.trim_end_matches('/'),
                product_id,
                remote_repo_id
            ))
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", session_id))
            .json(&serde_json::json!({
                "bundleIds": required_bundle_ids,
            })),
    )?;

    progress::emit_progress(
        &progress,
        GutchProgressEvent {
            operation: GutchOperation::SyncUp,
            phase: GutchProgressPhase::Done,
            message: "Sync complete".to_string(),
            current: total_steps,
            total: total_steps,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: true,
        },
    );

    Ok(RepoSyncUpStats {
        command: "repo push",
        repo_root: repo_paths.repo_root.display().to_string(),
        product_id,
        remote_repo_id,
        repo_manifest_uploaded,
        chunk_index_uploaded,
        target_manifest_uploaded_count,
        target_blueprint_uploaded_count,
        bundle_uploaded_count,
        bundle_skipped_count,
        remote_bundle_deleted_count: reconcile.deleted_bundle_ids.len(),
        target_count: status.targets.len(),
        bundle_count: status.bundle_count,
        target_added_count: plan.to_add.len(),
        target_removed_count: plan.to_remove.len(),
        target_updated_count: plan.to_update.len(),
        target_unchanged_count: plan.unchanged.len(),
    })
}
