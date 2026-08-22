use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::Result;
use crate::progress::{self, GutchOperation, GutchProgressEvent, GutchProgressPhase};
use crate::remote::backend::{
    CreatorRepoResponse, CreatorRepoSnapshotsResponse, CreatorReposResponse,
    CreatorSnapshotResponse, UpsertSnapshotResponse, request_json_from_backend, upload_raw_file,
};
use crate::repo::snapshot_descriptor::summarize_snapshot_tags;
#[derive(Debug, Serialize)]
pub struct RepoSyncUpStats {
    pub command: &'static str,
    pub repo_root: String,
    pub product_id: i64,
    pub remote_repo_id: i64,
    pub repo_manifest_uploaded: bool,
    pub chunk_index_uploaded: bool,
    pub snapshot_manifest_uploaded_count: usize,
    pub product_metadata_uploaded_count: usize,
    pub snapshot_blueprint_uploaded_count: usize,
    pub bundle_uploaded_count: usize,
    pub bundle_skipped_count: usize,
    pub remote_bundle_deleted_count: usize,
    pub snapshot_count: usize,
    pub bundle_count: usize,
    pub snapshot_added_count: usize,
    pub snapshot_removed_count: usize,
    pub snapshot_updated_count: usize,
    pub snapshot_unchanged_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoSyncSnapshotChange {
    pub snapshot_id: String,
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
    pub snapshot_count: usize,
    pub remote_snapshot_count: usize,
    pub to_add: Vec<RepoSyncSnapshotChange>,
    pub to_remove: Vec<RepoSyncSnapshotChange>,
    pub to_update: Vec<RepoSyncSnapshotChange>,
    pub unchanged: Vec<RepoSyncSnapshotChange>,
}

#[derive(Debug, Clone)]
struct SyncSnapshotRecord {
    snapshot_id: String,
    version: String,
    os: String,
    arch: String,
    tags: Vec<String>,
    snapshot_metadata_hash_b3: String,
    snapshot_product_metadata_hash_b3: String,
    snapshot_blueprint_hash_b3: String,
}

fn sync_snapshot_change(
    snapshot: &SyncSnapshotRecord,
    artifact_state: Option<String>,
    update_reason: Option<String>,
) -> RepoSyncSnapshotChange {
    RepoSyncSnapshotChange {
        snapshot_id: snapshot.snapshot_id.clone(),
        version: snapshot.version.clone(),
        os: snapshot.os.clone(),
        arch: snapshot.arch.clone(),
        artifact_state,
        tags: snapshot.tags.clone(),
        update_reason,
    }
}

fn remote_snapshot_change(snapshot: &CreatorSnapshotResponse) -> RepoSyncSnapshotChange {
    let (snapshot_record, artifact_state) = remote_sync_snapshot(snapshot);
    sync_snapshot_change(&snapshot_record, artifact_state, None)
}

fn remote_snapshot_tags(snapshot: &CreatorSnapshotResponse) -> Vec<String> {
    let mut values = Vec::new();
    values.extend(snapshot.snapshot_tags.plain.iter().cloned());
    for (key, scoped_values) in &snapshot.snapshot_tags.scoped {
        for value in scoped_values {
            values.push(format!("{}:{}", key, value));
        }
    }
    for (key, value) in &snapshot.snapshot_tags.exclusive {
        values.push(format!("{}::{}", key, value));
    }
    values.sort();
    values
}

fn remote_sync_snapshot(
    snapshot: &CreatorSnapshotResponse,
) -> (SyncSnapshotRecord, Option<String>) {
    (
        SyncSnapshotRecord {
            snapshot_id: snapshot.snapshot_id.clone(),
            version: snapshot
                .snapshot_tags
                .exclusive
                .get("version")
                .cloned()
                .unwrap_or_default(),
            os: snapshot
                .snapshot_tags
                .exclusive
                .get("os")
                .cloned()
                .unwrap_or_default(),
            arch: snapshot
                .snapshot_tags
                .exclusive
                .get("arch")
                .cloned()
                .unwrap_or_default(),
            tags: remote_snapshot_tags(snapshot),
            snapshot_metadata_hash_b3: snapshot.snapshot_metadata_hash_b3.clone(),
            snapshot_product_metadata_hash_b3: snapshot.snapshot_product_metadata_hash_b3.clone(),
            snapshot_blueprint_hash_b3: snapshot.snapshot_blueprint_hash_b3.clone(),
        },
        Some(snapshot.artifact_state.clone()),
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

fn local_snapshot_metadata_hash_b3(
    repo_paths: &crate::GutchRepoPaths,
    snapshot_id: &str,
) -> Result<String> {
    let manifest = crate::load_snapshot_manifest(repo_paths, snapshot_id)?;
    Ok(repo_file_hash(
        &crate::serialize_snapshot_metadata_for_remote(&manifest)?,
    ))
}

fn local_snapshot_product_metadata_hash_b3(
    repo_paths: &crate::GutchRepoPaths,
    snapshot_id: &str,
) -> Result<String> {
    let manifest = crate::load_snapshot_manifest(repo_paths, snapshot_id)?;
    let Some(bytes) = crate::serialize_snapshot_product_metadata_for_remote(&manifest)? else {
        return Ok(String::new());
    };
    Ok(repo_file_hash(&bytes))
}

fn local_snapshot_blueprint_hash_b3(
    repo_paths: &crate::GutchRepoPaths,
    snapshot_id: &str,
) -> Result<String> {
    let hash = crate::hash::hash_file(&crate::snapshot_blueprint_path(repo_paths, snapshot_id))?;
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
            "invalid product_id '{}' in repo_metadata.json",
            manifest.product_id
        ))
    })?;
    let remote_repo_id = manifest.remote_repo_id_value().ok_or_else(|| {
        crate::Error::manifest(format!(
            "invalid remote_repo_id '{}' in local_repo_config.json",
            manifest.remote_repo_id
        ))
    })?;
    Ok((product_id, remote_repo_id))
}

fn fetch_creator_snapshots(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    session_id: &str,
    product_id: i64,
    remote_repo_id: i64,
) -> Result<HashMap<String, CreatorSnapshotResponse>> {
    let creator_snapshots: CreatorRepoSnapshotsResponse = request_json_from_backend(
        client
            .get(format!(
                "{}/v1/users/me/creator/products/{}/repos/{}/snapshots",
                endpoint.trim_end_matches('/'),
                product_id,
                remote_repo_id
            ))
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", session_id)),
    )?;

    let mut remote_snapshots = HashMap::new();
    if !creator_snapshots.snapshots.is_empty() {
        for snapshot in creator_snapshots.snapshots {
            remote_snapshots.insert(snapshot.snapshot_id.clone(), snapshot);
        }
    } else {
        for tier in creator_snapshots.tier_labels {
            for snapshot in tier.snapshots {
                remote_snapshots.insert(snapshot.snapshot_id.clone(), snapshot);
            }
        }
    }
    Ok(remote_snapshots)
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

fn collect_local_sync_snapshots(
    repo: &Path,
) -> Result<(
    crate::RepoCheckStatus,
    crate::GutchRepoPaths,
    Vec<SyncSnapshotRecord>,
)> {
    let status = crate::check_gutch_repo(repo)?;
    if !status.is_repo {
        return Err(crate::Error::manifest(format!(
            "no gutch repo found at {}",
            repo.display()
        )));
    }
    let (repo_paths, _) = crate::load_gutch_repo_manifest(repo)?;
    let mut local_snapshots = Vec::with_capacity(status.snapshots.len());
    for snapshot in &status.snapshots {
        let manifest = crate::load_snapshot_manifest(&repo_paths, &snapshot.snapshot_id)?;
        let summary = summarize_snapshot_tags(&manifest.snapshot_tags, None);
        local_snapshots.push(SyncSnapshotRecord {
            snapshot_id: manifest.snapshot_id.clone(),
            version: summary.version_id,
            os: summary.os,
            arch: summary.arch,
            tags: summary.canonical_tags,
            snapshot_metadata_hash_b3: local_snapshot_metadata_hash_b3(
                &repo_paths,
                &manifest.snapshot_id,
            )?,
            snapshot_product_metadata_hash_b3: local_snapshot_product_metadata_hash_b3(
                &repo_paths,
                &manifest.snapshot_id,
            )?,
            snapshot_blueprint_hash_b3: local_snapshot_blueprint_hash_b3(
                &repo_paths,
                &manifest.snapshot_id,
            )?,
        });
    }
    Ok((status, repo_paths, local_snapshots))
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

    let (status, repo_paths, local_snapshots) = collect_local_sync_snapshots(&repo)?;
    let remote_repo =
        fetch_creator_repo(&client, &endpoint, &session_id, product_id, remote_repo_id)?;
    let remote_snapshots =
        fetch_creator_snapshots(&client, &endpoint, &session_id, product_id, remote_repo_id)?;
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
    for local_snapshot in &local_snapshots {
        match remote_snapshots.get(&local_snapshot.snapshot_id) {
            None => to_add.push(sync_snapshot_change(local_snapshot, None, None)),
            Some(remote_snapshot) => {
                let tag_mismatch = local_snapshot.tags != remote_snapshot_tags(remote_snapshot);
                let needs_state_update = remote_snapshot.artifact_state != "ready";
                let metadata_hash_changed = hashes_differ(
                    &local_snapshot.snapshot_metadata_hash_b3,
                    &remote_snapshot.snapshot_metadata_hash_b3,
                );
                let snapshot_blueprint_hash_changed = hashes_differ(
                    &local_snapshot.snapshot_blueprint_hash_b3,
                    &remote_snapshot.snapshot_blueprint_hash_b3,
                );
                let snapshot_product_metadata_hash_changed = hashes_differ(
                    &local_snapshot.snapshot_product_metadata_hash_b3,
                    &remote_snapshot.snapshot_product_metadata_hash_b3,
                );
                if tag_mismatch
                    || needs_state_update
                    || metadata_hash_changed
                    || snapshot_blueprint_hash_changed
                    || snapshot_product_metadata_hash_changed
                {
                    let mut reasons = Vec::new();
                    if tag_mismatch {
                        reasons.push("snapshot tags differ from remote");
                    }
                    if needs_state_update {
                        reasons.push("remote artifact state is not ready");
                    }
                    if metadata_hash_changed {
                        reasons.push("snapshot metadata hash differs");
                    }
                    if snapshot_blueprint_hash_changed {
                        reasons.push("snapshot blueprint hash differs");
                    }
                    if snapshot_product_metadata_hash_changed {
                        reasons.push("snapshot product metadata hash differs");
                    }
                    to_update.push(sync_snapshot_change(
                        local_snapshot,
                        Some(remote_snapshot.artifact_state.clone()),
                        Some(format!("{}.", reasons.join("; "))),
                    ));
                } else {
                    unchanged.push(sync_snapshot_change(
                        local_snapshot,
                        Some(remote_snapshot.artifact_state.clone()),
                        None,
                    ));
                }
            }
        }
    }

    let mut to_remove = Vec::new();
    for remote_snapshot in remote_snapshots.values() {
        if !local_snapshots
            .iter()
            .any(|local_snapshot| local_snapshot.snapshot_id == remote_snapshot.snapshot_id)
        {
            to_remove.push(remote_snapshot_change(remote_snapshot));
        }
    }

    to_add.sort_by(|left, right| left.snapshot_id.cmp(&right.snapshot_id));
    to_update.sort_by(|left, right| left.snapshot_id.cmp(&right.snapshot_id));
    unchanged.sort_by(|left, right| left.snapshot_id.cmp(&right.snapshot_id));
    to_remove.sort_by(|left, right| left.snapshot_id.cmp(&right.snapshot_id));

    Ok(RepoSyncPlan {
        repo_root: repo_paths.repo_root.display().to_string(),
        product_id,
        remote_repo_id,
        repo_manifest_changed,
        chunk_index_changed,
        snapshot_count: status.snapshots.len(),
        remote_snapshot_count: remote_snapshots.len(),
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
    manifest.sync_product_metadata_from_product_id();
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
    for snapshot in &status.snapshots {
        let manifest = crate::load_snapshot_manifest(repo_paths, &snapshot.snapshot_id)?;
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
    let (status, repo_paths, _) = collect_local_sync_snapshots(&repo)?;
    let mut remote_snapshots =
        fetch_creator_snapshots(&client, &endpoint, &session_id, product_id, remote_repo_id)?;

    let bundle_paths = collect_bundle_paths(&repo_paths.bundles_root)?;
    let required_bundle_ids = collect_required_bundle_ids(&repo_paths, &status)?;
    let chunk_index_steps = usize::from(repo_paths.chunk_map_path.exists());
    let snapshot_upload_count = plan.to_add.len() + plan.to_update.len();
    let total_steps = plan.to_remove.len()
        + 1
        + 1
        + chunk_index_steps
        + snapshot_upload_count
        + snapshot_upload_count
        + snapshot_upload_count
        + bundle_paths.len()
        + 1;
    let mut step = 0usize;

    for snapshot in &plan.to_remove {
        control.checkpoint()?;
        step += 1;
        progress::emit_progress(
            &progress,
            GutchProgressEvent {
                operation: GutchOperation::SyncUp,
                phase: GutchProgressPhase::RemoveSnapshots,
                message: "Removing remote snapshots".to_string(),
                current: step,
                total: total_steps,
                transferred_bytes: 0,
                total_bytes: 0,
                detail: Some(snapshot.snapshot_id.clone()),
                done: false,
            },
        );
        if let Some(remote_snapshot) = remote_snapshots.remove(&snapshot.snapshot_id) {
            let response = client
                .delete(format!(
                    "{}/v1/users/me/creator/products/{}/repos/{}/snapshots/{}",
                    endpoint.trim_end_matches('/'),
                    product_id,
                    remote_repo_id,
                    remote_snapshot.id
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

    let mut snapshot_manifest_uploaded_count = 0usize;
    let mut product_metadata_uploaded_count = 0usize;
    let mut snapshot_blueprint_uploaded_count = 0usize;
    let snapshot_ids_to_upload = plan
        .to_add
        .iter()
        .chain(plan.to_update.iter())
        .map(|change| change.snapshot_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    for snapshot in status
        .snapshots
        .iter()
        .filter(|snapshot| snapshot_ids_to_upload.contains(snapshot.snapshot_id.as_str()))
    {
        control.checkpoint()?;
        step += 1;
        progress::emit_progress(
            &progress,
            GutchProgressEvent {
                operation: GutchOperation::SyncUp,
                phase: GutchProgressPhase::UploadSnapshotMetadata,
                message: "Uploading snapshot metadata".to_string(),
                current: step,
                total: total_steps,
                transferred_bytes: 0,
                total_bytes: 0,
                detail: Some(snapshot.snapshot_id.clone()),
                done: false,
            },
        );
        let manifest = crate::load_snapshot_manifest(&repo_paths, &snapshot.snapshot_id)?;
        let manifest_bytes = crate::serialize_snapshot_metadata_for_remote(&manifest)?;
        let version = manifest
            .snapshot_tags
            .get_exclusive("version")
            .ok_or_else(|| {
                crate::Error::manifest(format!(
                    "snapshot '{}' is missing version tag",
                    manifest.snapshot_id
                ))
            })?;
        let structured_version =
            crate::manifest::StructuredVersion::parse(version).map_err(|_| {
                crate::Error::manifest(format!(
                    "snapshot '{}' has invalid version '{}'",
                    manifest.snapshot_id, version
                ))
            })?;
        let remote_snapshot_id = match remote_snapshots
            .get(&manifest.snapshot_id)
            .map(|snapshot| snapshot.id)
        {
            Some(id) => id,
            None => {
                let created: UpsertSnapshotResponse = request_json_from_backend(
                    client
                        .post(format!(
                            "{}/v1/users/me/creator/products/{}/repos/{}/snapshots",
                            endpoint.trim_end_matches('/'),
                            product_id,
                            remote_repo_id
                        ))
                        .header("Accept", "application/json")
                        .header("Authorization", format!("Bearer {}", session_id))
                        .json(&serde_json::json!({
                            "snapshotId": manifest.snapshot_id,
                            "snapshotTags": manifest.snapshot_tags,
                            "versionMajor": structured_version.major,
                            "versionMinor": structured_version.minor,
                            "versionPatch": structured_version.patch,
                            "versionRevision": structured_version.revision,
                        })),
                )?;
                remote_snapshots.insert(
                    manifest.snapshot_id.clone(),
                    CreatorSnapshotResponse {
                        id: created.id,
                        snapshot_id: manifest.snapshot_id.clone(),
                        snapshot_tags: Default::default(),
                        artifact_state: String::from("pending"),
                        snapshot_metadata_hash_b3: String::new(),
                        snapshot_blueprint_hash_b3: String::new(),
                        snapshot_product_metadata_hash_b3: String::new(),
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
                "/v1/users/me/creator/products/{}/repos/{}/snapshots/{}/metadata",
                product_id, remote_repo_id, remote_snapshot_id
            ),
            &session_id,
            manifest_bytes,
            "application/json",
            &control,
        )?;
        snapshot_manifest_uploaded_count += 1;

        control.checkpoint()?;
        step += 1;
        progress::emit_progress(
            &progress,
            GutchProgressEvent {
                operation: GutchOperation::SyncUp,
                phase: GutchProgressPhase::UploadSnapshotBlueprint,
                message: "Uploading snapshot blueprints".to_string(),
                current: step,
                total: total_steps,
                transferred_bytes: 0,
                total_bytes: 0,
                detail: Some(snapshot.snapshot_id.clone()),
                done: false,
            },
        );
        let blueprint_bytes = crate::helpers::read(&crate::snapshot_blueprint_path(
            &repo_paths,
            &snapshot.snapshot_id,
        ))?;
        upload_raw_file(
            &client,
            &endpoint,
            &format!(
                "/v1/users/me/creator/products/{}/repos/{}/snapshots/{}/blueprint",
                product_id, remote_repo_id, remote_snapshot_id
            ),
            &session_id,
            blueprint_bytes,
            "application/octet-stream",
            &control,
        )?;
        snapshot_blueprint_uploaded_count += 1;

        control.checkpoint()?;
        step += 1;
        progress::emit_progress(
            &progress,
            GutchProgressEvent {
                operation: GutchOperation::SyncUp,
                phase: GutchProgressPhase::UploadSnapshotMetadata,
                message: "Uploading snapshot product metadata".to_string(),
                current: step,
                total: total_steps,
                transferred_bytes: 0,
                total_bytes: 0,
                detail: Some(snapshot.snapshot_id.clone()),
                done: false,
            },
        );
        let snapshot_product_metadata_path = format!(
            "/v1/users/me/creator/products/{}/repos/{}/snapshots/{}/product-metadata",
            product_id, remote_repo_id, remote_snapshot_id
        );
        if let Some(product_metadata_bytes) =
            crate::serialize_snapshot_product_metadata_for_remote(&manifest)?
        {
            upload_raw_file(
                &client,
                &endpoint,
                &snapshot_product_metadata_path,
                &session_id,
                product_metadata_bytes,
                "application/json",
                &control,
            )?;
            product_metadata_uploaded_count += 1;
        } else if remote_snapshots
            .get(&manifest.snapshot_id)
            .is_some_and(|remote| !remote.snapshot_product_metadata_hash_b3.trim().is_empty())
        {
            let response = client
                .delete(format!(
                    "{}{}",
                    endpoint.trim_end_matches('/'),
                    snapshot_product_metadata_path
                ))
                .header("Accept", "application/json")
                .header("Authorization", format!("Bearer {}", session_id))
                .send()
                .map_err(|error| {
                    crate::Error::manifest(format!("backend request failed: {error}"))
                })?;
            if !response.status().is_success() {
                return Err(crate::Error::manifest(
                    crate::remote::backend::parse_backend_error_response(response),
                ));
            }
        }
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
        snapshot_manifest_uploaded_count,
        product_metadata_uploaded_count,
        snapshot_blueprint_uploaded_count,
        bundle_uploaded_count,
        bundle_skipped_count,
        remote_bundle_deleted_count: reconcile.deleted_bundle_ids.len(),
        snapshot_count: status.snapshots.len(),
        bundle_count: status.bundle_count,
        snapshot_added_count: plan.to_add.len(),
        snapshot_removed_count: plan.to_remove.len(),
        snapshot_updated_count: plan.to_update.len(),
        snapshot_unchanged_count: plan.unchanged.len(),
    })
}
