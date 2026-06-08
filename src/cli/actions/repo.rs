use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::cli::common::{load_optional_rules, parse_version_parts, session_id_for_cli};
use g::remote::backend::{
    CreatorRepoTargetsResponse, UpsertTargetResponse, request_json_from_backend,
    slugify_like_backend, upload_raw_file,
};
use gutch as g;

#[derive(Debug, Serialize)]
pub(crate) struct RepoSyncUpStats {
    pub(crate) command: &'static str,
    pub(crate) repo_root: String,
    pub(crate) product_id: i64,
    pub(crate) gutch_repo_id: i64,
    pub(crate) repo_manifest_uploaded: bool,
    pub(crate) chunk_index_uploaded: bool,
    pub(crate) target_manifest_uploaded_count: usize,
    pub(crate) bundle_uploaded_count: usize,
    pub(crate) bundle_skipped_count: usize,
    pub(crate) target_count: usize,
    pub(crate) bundle_count: usize,
}

pub(crate) fn init_repo(path: &Path, schema: Option<&PathBuf>) -> g::Result<()> {
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

pub(crate) fn unlink_repo(repo: &Path) -> g::Result<()> {
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

pub(crate) fn sync_up_repo(
    repo: PathBuf,
    endpoint: String,
    product_id: Option<i64>,
    gutch_repo_id: Option<i64>,
    session_override: Option<String>,
) -> g::Result<RepoSyncUpStats> {
    let (product_id, gutch_repo_id) = if let (Some(pid), Some(cid)) = (product_id, gutch_repo_id) {
        (pid, cid)
    } else {
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
        (pid, linked.remote_repo_id)
    };
    let session_id = session_id_for_cli(session_override)?;
    let client = reqwest::blocking::Client::builder()
        .user_agent("Gutch CLI")
        .build()
        .map_err(|err| g::Error::manifest(format!("failed to create sync client: {}", err)))?;

    let status = g::inspect_gutch_repo(&repo)?;
    if !status.is_repo {
        return Err(g::Error::manifest(format!(
            "no gutch repo found at {}",
            repo.display()
        )));
    }
    let (repo_paths, _) = g::load_gutch_repo_manifest(&repo)?;

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
        let tier_slug = manifest.target_tags.get_exclusive("tier").ok_or_else(|| {
            g::Error::manifest(format!(
                "target '{}' is missing tier tag",
                manifest.target_id
            ))
        })?;
        let tier_id = tiers_by_slug.get(tier_slug).copied().ok_or_else(|| {
            g::Error::manifest(format!(
                "no remote tier found for slug '{}' while syncing target '{}'",
                tier_slug, manifest.target_id
            ))
        })?;

        let remote_target_id = match remote_target_ids.get(&manifest.target_id).copied() {
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
                            "versionMajor": version_major,
                            "versionMinor": version_minor,
                            "versionPatch": version_patch,
                            "tierId": tier_id,
                        })),
                )?;
                remote_target_ids.insert(manifest.target_id.clone(), created.id);
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
            std::fs::read(entry.path())?,
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
    })
}
