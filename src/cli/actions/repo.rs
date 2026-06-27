use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use crate::cli::common::{load_optional_rules, parse_version_parts, session_id_for_cli};
use crate::sync_up_progress::{SyncUpProgressEvent, SyncUpProgressSink, emit_progress};
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
    pub remote_repo_id: i64,
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
    pub os: String,
    pub arch: String,
    pub tier: String,
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
    os: String,
    arch: String,
    tier: String,
    tags: Vec<String>,
}

fn local_target_change(
    target: &LocalSyncTarget,
    artifact_state: Option<String>,
    update_reason: Option<String>,
) -> RepoSyncTargetChange {
    RepoSyncTargetChange {
        target_id: target.target_id.clone(),
        version: target.version.clone(),
        os: target.os.clone(),
        arch: target.arch.clone(),
        tier: target.tier.clone(),
        artifact_state,
        tags: target.tags.clone(),
        update_reason,
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
        tier: target
            .target_tags
            .exclusive
            .get("tier")
            .cloned()
            .unwrap_or_default(),
        artifact_state: Some(target.artifact_state.clone()),
        tags: remote_target_tags(target),
        update_reason: None,
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

fn resolve_sync_binding(repo: &PathBuf) -> g::Result<(i64, i64)> {
    let (paths, manifest) = g::load_gutch_repo_manifest(repo)?;
    if !manifest.is_linked() {
        return Err(g::Error::manifest(format!(
            "repo '{}' is not linked. Use 'gutch repo link' first.",
            paths.repo_root.display()
        )));
    }
    let product_id = manifest.product_id.parse::<i64>().map_err(|_| {
        g::Error::manifest(format!(
            "invalid product_id '{}' in manifest",
            manifest.product_id
        ))
    })?;
    let remote_repo_id = manifest.remote_repo_id_value().ok_or_else(|| {
        g::Error::manifest(format!(
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
) -> g::Result<HashMap<String, CreatorTargetResponse>> {
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
    for tier in creator_targets.tier_labels {
        for target in tier.targets {
            remote_targets.insert(target.target_id.clone(), target);
        }
    }
    Ok(remote_targets)
}

fn remote_bundle_exists(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    session_id: &str,
    product_id: i64,
    remote_repo_id: i64,
    bundle_id: &str,
) -> g::Result<bool> {
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
        .map_err(|err| g::Error::manifest(format!("backend request failed: {}", err)))?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    if !response.status().is_success() {
        return Err(g::Error::manifest(
            g::remote::backend::parse_backend_error_response(response),
        ));
    }
    Ok(true)
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
            os: manifest
                .target_tags
                .get_exclusive("os")
                .unwrap_or_default()
                .to_string(),
            arch: manifest
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
    session_override: Option<String>,
) -> g::Result<RepoSyncPlan> {
    let (product_id, remote_repo_id) = resolve_sync_binding(&repo)?;
    let session_id = session_id_for_cli(session_override)?;
    let client = reqwest::blocking::Client::builder()
        .user_agent("Gutch CLI")
        .build()
        .map_err(|err| g::Error::manifest(format!("failed to create sync client: {}", err)))?;

    let (status, repo_paths, local_targets) = collect_local_sync_targets(&repo)?;
    let remote_targets =
        fetch_creator_targets(&client, &endpoint, &session_id, product_id, remote_repo_id)?;

    let mut to_add = Vec::new();
    let mut to_update = Vec::new();
    let mut unchanged = Vec::new();
    for local_target in &local_targets {
        match remote_targets.get(&local_target.target_id) {
            None => to_add.push(local_target_change(local_target, None, None)),
            Some(remote_target) => {
                let tag_mismatch = local_target.tags != remote_target_tags(remote_target);
                let needs_state_update = remote_target.artifact_state != "ready";
                if tag_mismatch || needs_state_update {
                    let update_reason = if tag_mismatch && needs_state_update {
                        Some(String::from(
                            "Target tags and remote artifact state differ.",
                        ))
                    } else if tag_mismatch {
                        Some(String::from("Target tags differ from remote."))
                    } else {
                        Some(String::from(
                            "Remote artifact state is not ready; manifest will be re-uploaded.",
                        ))
                    };
                    to_update.push(local_target_change(
                        local_target,
                        Some(remote_target.artifact_state.clone()),
                        update_reason,
                    ));
                } else {
                    unchanged.push(local_target_change(
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
        "created gutch repo '{}' at {}",
        repo_name,
        repo_paths.repo_root.display()
    );

    let mut config = g::config::load_config()?;
    config.upsert_repo(g::config::TrackedRepo {
        path: repo_paths.repo_root.display().to_string(),
        target_sources_path: None,
    });
    g::config::save_config(&config)?;
    Ok(())
}

pub fn link_repo(
    repo: &Path,
    product_id: i64,
    endpoint: &str,
    session_override: Option<String>,
) -> g::Result<()> {
    let (paths, mut manifest) = g::load_gutch_repo_manifest(repo)?;
    if manifest.is_linked() {
        return Err(g::Error::manifest(format!(
            "repo '{}' is already linked (remote_repo_id={}, product_id={}). Unlink first.",
            paths.repo_root.display(),
            manifest.remote_repo_id,
            manifest.product_id
        )));
    }

    let session_id = session_id_for_cli(session_override)?;
    let client = reqwest::blocking::Client::builder()
        .user_agent("Gutch CLI")
        .build()
        .map_err(|err| g::Error::manifest(format!("failed to create link client: {}", err)))?;

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
        .map_err(|err| g::Error::manifest(format!("backend request failed: {}", err)))?;

    if response.status() == reqwest::StatusCode::CONFLICT {
        let body = response
            .text()
            .unwrap_or_else(|_| "repo name already taken".to_string());
        return Err(g::Error::manifest(format!(
            "repo name '{}' already exists for product {}. {}",
            repo_name, product_id, body
        )));
    }

    if !response.status().is_success() {
        return Err(g::Error::manifest(
            g::remote::backend::parse_backend_error_response(response),
        ));
    }

    let created: serde_json::Value = response
        .json()
        .map_err(|err| g::Error::manifest(format!("failed to parse link response: {}", err)))?;
    let remote_repo_id = created["id"]
        .as_i64()
        .ok_or_else(|| g::Error::manifest("backend response missing repo id".to_string()))?;

    manifest.remote_repo_id = remote_repo_id.to_string();
    manifest.product_id = product_id.to_string();
    g::write_gutch_repo_manifest(&paths, &manifest)?;

    println!(
        "linked repo '{}' to remote repo {} (product {})",
        paths.repo_root.display(),
        remote_repo_id,
        product_id
    );
    Ok(())
}

pub fn unlink_repo(
    repo: &Path,
    delete_remote: bool,
    endpoint: Option<&str>,
    session_override: Option<String>,
) -> g::Result<()> {
    let (paths, mut manifest) = g::load_gutch_repo_manifest(repo)?;
    if !manifest.is_linked() {
        println!(
            "repo '{}' is not linked, nothing to do.",
            paths.repo_root.display()
        );
        return Ok(());
    }

    if delete_remote {
        let remote_repo_id = manifest.remote_repo_id_value().ok_or_else(|| {
            g::Error::manifest(format!(
                "invalid remote_repo_id '{}' in manifest",
                manifest.remote_repo_id
            ))
        })?;
        let product_id = manifest.product_id.parse::<i64>().map_err(|_| {
            g::Error::manifest(format!(
                "invalid product_id '{}' in manifest",
                manifest.product_id
            ))
        })?;

        let endpoint = endpoint.ok_or_else(|| {
            g::Error::manifest("--endpoint required when using --delete-remote".to_string())
        })?;
        let session_id = session_id_for_cli(session_override)?;
        let client = reqwest::blocking::Client::builder()
            .user_agent("Gutch CLI")
            .build()
            .map_err(|err| g::Error::manifest(format!("failed to create client: {}", err)))?;

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
            .map_err(|err| g::Error::manifest(format!("backend request failed: {}", err)))?;

        if !response.status().is_success() {
            return Err(g::Error::manifest(
                g::remote::backend::parse_backend_error_response(response),
            ));
        }

        println!(
            "deleted remote repo {} (product {})",
            remote_repo_id, product_id
        );
    }

    manifest.remote_repo_id = String::new();
    manifest.product_id = String::new();
    g::write_gutch_repo_manifest(&paths, &manifest)?;

    println!("unlinked repo '{}'", paths.repo_root.display());
    Ok(())
}

pub fn sync_up_repo(
    repo: PathBuf,
    endpoint: String,
    session_override: Option<String>,
) -> g::Result<RepoSyncUpStats> {
    sync_up_repo_with_progress(repo, endpoint, session_override, None)
}

fn collect_bundle_paths(bundles_root: &Path) -> g::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in walkdir::WalkDir::new(bundles_root).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file() {
            continue;
        }
        if g::bundle_id_from_bundle_path(entry.path()).is_some() {
            paths.push(entry.path().to_path_buf());
        }
    }
    Ok(paths)
}

pub fn sync_up_repo_with_progress(
    repo: PathBuf,
    endpoint: String,
    session_override: Option<String>,
    progress: Option<Arc<dyn SyncUpProgressSink>>,
) -> g::Result<RepoSyncUpStats> {
    let (product_id, remote_repo_id) = resolve_sync_binding(&repo)?;
    let session_id = session_id_for_cli(session_override)?;
    let client = reqwest::blocking::Client::builder()
        .user_agent("Gutch CLI")
        .build()
        .map_err(|err| g::Error::manifest(format!("failed to create sync client: {}", err)))?;

    emit_progress(
        &progress,
        SyncUpProgressEvent {
            phase: String::from("planning"),
            message: String::from("Planning sync changes"),
            current: 0,
            total: 1,
            detail: None,
            transferred_bytes: 0,
            total_bytes: 0,
            done: false,
        },
    );

    let plan = plan_sync_up_repo(repo.clone(), endpoint.clone(), Some(session_id.clone()))?;
    let (status, repo_paths, _) = collect_local_sync_targets(&repo)?;
    let mut remote_targets =
        fetch_creator_targets(&client, &endpoint, &session_id, product_id, remote_repo_id)?;

    let bundle_paths = collect_bundle_paths(&repo_paths.bundles_root)?;
    let chunk_index_steps = usize::from(repo_paths.chunk_index_path.exists());
    let total_steps =
        plan.to_remove.len() + 1 + chunk_index_steps + status.targets.len() + bundle_paths.len();
    let mut step = 0usize;

    for target in &plan.to_remove {
        step += 1;
        emit_progress(
            &progress,
            SyncUpProgressEvent {
                phase: String::from("remove_targets"),
                message: String::from("Removing remote targets"),
                current: step,
                total: total_steps,
                detail: Some(target.target_id.clone()),
                transferred_bytes: 0,
                total_bytes: 0,
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
                .map_err(|err| g::Error::manifest(format!("backend request failed: {}", err)))?;
            if !response.status().is_success() {
                return Err(g::Error::manifest(
                    g::remote::backend::parse_backend_error_response(response),
                ));
            }
        }
    }

    step += 1;
    emit_progress(
        &progress,
        SyncUpProgressEvent {
            phase: String::from("upload_repo_manifest"),
            message: String::from("Uploading repo manifest"),
            current: step,
            total: total_steps,
            detail: None,
            transferred_bytes: 0,
            total_bytes: 0,
            done: false,
        },
    );
    let repo_manifest_bytes = crate::helpers::read(&repo_paths.repo_manifest_path)?;
    let remote_repo_manifest_bytes = g::remote::backend::fetch_repo_manifest_bytes(
        &client,
        &endpoint,
        &session_id,
        product_id,
        remote_repo_id,
    )?;
    let repo_manifest_uploaded =
        if remote_repo_manifest_bytes.as_deref() == Some(repo_manifest_bytes.as_slice()) {
            false
        } else {
            upload_raw_file(
                &client,
                &endpoint,
                &format!(
                    "/v1/users/me/creator/products/{}/repos/{}/manifest",
                    product_id, remote_repo_id
                ),
                &session_id,
                repo_manifest_bytes,
                "application/json",
            )?;
            true
        };

    let mut chunk_index_uploaded = false;
    if repo_paths.chunk_index_path.exists() {
        step += 1;
        emit_progress(
            &progress,
            SyncUpProgressEvent {
                phase: String::from("upload_chunk_index"),
                message: String::from("Uploading chunk index"),
                current: step,
                total: total_steps,
                detail: None,
                transferred_bytes: 0,
                total_bytes: 0,
                done: false,
            },
        );
        let chunk_index = g::load_chunk_index(&repo_paths)?;
        let chunk_index_bytes = serde_json::to_vec_pretty(&chunk_index)?;
        let remote_chunk_index_bytes = g::remote::backend::fetch_chunk_index_bytes(
            &client,
            &endpoint,
            &session_id,
            product_id,
            remote_repo_id,
        )?;
        if remote_chunk_index_bytes.as_deref() != Some(chunk_index_bytes.as_slice()) {
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
            )?;
            chunk_index_uploaded = true;
        }
    }

    let mut target_manifest_uploaded_count = 0usize;
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
        step += 1;
        emit_progress(
            &progress,
            SyncUpProgressEvent {
                phase: String::from("upload_target_manifest"),
                message: String::from("Uploading target manifests"),
                current: step,
                total: total_steps,
                detail: Some(target.target_id.clone()),
                transferred_bytes: 0,
                total_bytes: 0,
                done: false,
            },
        );
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
                            remote_repo_id
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
                product_id, remote_repo_id, remote_target_id
            ),
            &session_id,
            manifest_bytes,
            "application/json",
        )?;
        target_manifest_uploaded_count += 1;
    }

    let mut bundle_uploaded_count = 0usize;
    let mut bundle_skipped_count = 0usize;
    for bundle_path in bundle_paths {
        step += 1;
        let Some(bundle_id) = g::bundle_id_from_bundle_path(&bundle_path) else {
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
            emit_progress(
                &progress,
                SyncUpProgressEvent {
                    phase: String::from("upload_bundle"),
                    message: String::from("Skipping bundles already present remotely"),
                    current: step,
                    total: total_steps,
                    detail: Some(bundle_id),
                    transferred_bytes: 0,
                    total_bytes: 0,
                    done: false,
                },
            );
            continue;
        }
        let bundle_bytes = crate::helpers::read(&bundle_path)?;
        let bundle_size = bundle_bytes.len() as u64;
        emit_progress(
            &progress,
            SyncUpProgressEvent {
                phase: String::from("upload_bundle"),
                message: String::from("Uploading bundles"),
                current: step,
                total: total_steps,
                detail: Some(bundle_id.clone()),
                transferred_bytes: 0,
                total_bytes: bundle_size,
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
        )?;
        bundle_uploaded_count += 1;
        emit_progress(
            &progress,
            SyncUpProgressEvent {
                phase: String::from("upload_bundle"),
                message: String::from("Uploading bundles"),
                current: step,
                total: total_steps,
                detail: Some(bundle_id),
                transferred_bytes: bundle_size,
                total_bytes: bundle_size,
                done: false,
            },
        );
    }

    emit_progress(
        &progress,
        SyncUpProgressEvent {
            phase: String::from("complete"),
            message: String::from("Sync complete"),
            current: total_steps,
            total: total_steps,
            detail: None,
            transferred_bytes: 0,
            total_bytes: 0,
            done: true,
        },
    );

    Ok(RepoSyncUpStats {
        command: "repo sync-up",
        repo_root: repo_paths.repo_root.display().to_string(),
        product_id,
        remote_repo_id,
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
