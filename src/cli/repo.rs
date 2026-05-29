use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::Serialize;

use crate::cli::backend::{
    CreatorRepoTargetsResponse, SignedChunkIndexResponse, SignedRepoManifestResponse,
    SignedTargetManifestResponse, UpsertTargetResponse, request_json_from_backend,
    request_optional_bytes_from_signed_url, slugify_like_backend, upload_raw_file,
};
use crate::cli::common::{
    load_optional_rules, load_target_manifest_from_repo, parse_version_parts, session_id_for_cli,
};
use crate::cli::{SECTION_SEPARATOR, verbose_output_enabled};
use crate::cli::reporting::{
    ensure_parent_directory,
};
use crate::RepoCommands;
use gutch as g;

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
                            content_repo_id
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

        let signed_target_manifest: SignedTargetManifestResponse = request_json_from_backend(
            client
                .get(format!(
                    "{}/v1/users/me/creator/products/{}/repos/{}/targets/{}/manifest",
                    endpoint.trim_end_matches('/'),
                    product_id,
                    content_repo_id,
                    remote_target_id
                ))
                .header("Accept", "application/json")
                .header("Authorization", format!("Bearer {}", session_id)),
        )?;
        let remote_manifest_bytes =
            request_optional_bytes_from_signed_url(&client, &signed_target_manifest.manifest_url)?;
        let should_upload = remote_manifest_bytes
            .as_deref()
            .map(|bytes| bytes != manifest_bytes.as_slice())
            .unwrap_or(true);
        if should_upload {
            upload_raw_file(
                &client,
                &endpoint,
                &format!(
                    "/v1/users/me/creator/products/{}/repos/{}/targets/{}/manifest",
                    product_id, content_repo_id, remote_target_id
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
            return Err(g::Error::manifest(
                crate::cli::backend::parse_backend_error_response(exists_response),
            ));
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
        repo_root: repo_paths.repo_root.display().to_string(),
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

pub(crate) fn run(command: RepoCommands) -> g::Result<()> {
    match command {
        RepoCommands::Init {
            path,
            schema,
        } => {
            let repo_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .filter(|n| !n.is_empty())
                .unwrap_or("base");
            let repo_paths = g::ContentRepoPaths::from_repo_root(&path);
            let mut repo_manifest = g::ensure_content_repo_layout(&repo_paths, repo_name)?;
            repo_manifest.target_schema = load_optional_rules(&schema)?;
            g::write_content_repo_manifest(&repo_paths, &repo_manifest)?;

            println!(
                "created content repo {} at {}",
                repo_name,
                repo_paths.repo_root.display()
            );

            let mut config = g::config::load_config()?;
            config.upsert_repo(g::config::TrackedRepo {
                path: repo_paths.repo_root.display().to_string(),
                content_repo_id: repo_name.to_string(),
                product_id: None,
                creator_user_id: None,
                remote_repo_id: None,
                remote_repo_slug: None,
                source_targets_path: None,
            });
            g::config::save_config(&config)?;
        }

        RepoCommands::SchemaSet {
            repo,
            schema,
            clear,
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

        RepoCommands::Inspect { repo, json_out } => {
            let status = g::inspect_content_repo(&repo)?;
            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                std::fs::write(json_out, serde_json::to_vec_pretty(&status)?)?;
            }

            if status.is_repo {
                println!("{SECTION_SEPARATOR}");
                println!("Targets");
                for target in &status.targets {
                    println!(
                        "{} | version={} | os={} | arch={} | tier={} | files={} | missing_bundles={}",
                        target.target_id,
                        target.version_id,
                        target.platform,
                        target.architecture,
                        target.tier_id,
                        target.file_count,
                        target.missing_bundle_count
                    );
                }
                println!("{SECTION_SEPARATOR}");
                println!("Total");
                println!("content_repo_id: {}", status.content_repo_id);
                println!("repo_path: {}", status.repo_path);
                println!("targets: {}", status.targets.len());
                println!("bundles: {}", status.bundle_count);
                println!("bundle_bytes: {}", status.bundle_bytes);
            } else if let Some(error) = status.error.as_deref() {
                println!("inspect status: {}", error);
            } else {
                println!(
                    "inspect status: no content repo found at {}",
                    status.repo_path
                );
            }
        }

        RepoCommands::DiscoverSourceTargets {
            repo,
            source_targets_root,
            json_out,
        } => {
            let candidates = g::discover_source_target_installs(&repo, &source_targets_root)?;
            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                std::fs::write(json_out, serde_json::to_vec_pretty(&candidates)?)?;
            }

            println!("{SECTION_SEPARATOR}");
            println!("Targets");
            for candidate in &candidates {
                println!(
                    "{} | path={} | version={} | os={} | arch={} | tier={}",
                    candidate.target_id,
                    candidate.path,
                    candidate.version_id,
                    candidate.platform,
                    candidate.architecture,
                    candidate.tier_id
                );
            }
            println!("{SECTION_SEPARATOR}");
            println!("Total");
            println!("count: {}", candidates.len());
            println!("source_targets_root: {}", source_targets_root.display());
        }

        RepoCommands::List { repo, json_out } => {
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

            println!("{SECTION_SEPARATOR}");
            println!("Targets");
            for target in &status.targets {
                println!("{}", target.target_id);
            }
            println!("{SECTION_SEPARATOR}");
            println!("Total");
            println!("content_repo_id: {}", status.content_repo_id);
            println!("targets: {}", status.targets.len());
            println!("bundles: {}", status.bundle_count);
        }

        RepoCommands::DeleteTarget { repo, target, yes } => {
            let (repo_paths, _repo_manifest, _, _tm) =
                load_target_manifest_from_repo(&repo, &target)?;
            let manifest_path = g::target_manifest_path(&repo_paths, &target);
            let manifest_bin_path = g::target_manifest_bin_path(&repo_paths, &target);
            let manifest_sig_path = g::target_manifest_sig_path(&repo_paths, &target);
            if !yes {
                eprintln!(
                    "WARNING: this will remove '{}' and related target artifacts, and may orphan bundles. Use --yes to confirm.",
                    manifest_path.display()
                );
                return Err(g::Error::manifest("deletion requires --yes flag"));
            }

            for path in [&manifest_path, &manifest_bin_path, &manifest_sig_path] {
                if path.exists() {
                    if path.is_dir() {
                        std::fs::remove_dir_all(path)?;
                    } else {
                        std::fs::remove_file(path)?;
                    }
                }
            }
            if verbose_output_enabled() {
                println!("removed target: {}", target);
            }

            let remaining_targets: HashSet<String> = {
                let mut ids = HashSet::new();
                if repo_paths.targets_root.exists() {
                    for entry in std::fs::read_dir(&repo_paths.targets_root)? {
                        let entry = entry?;
                        if entry.path().is_file()
                            && entry.path().extension().and_then(|value| value.to_str()) == Some("json")
                        {
                            if let Some(target_id) =
                                entry.path().file_stem().and_then(|value| value.to_str())
                            {
                                ids.insert(target_id.to_string());
                            }
                        }
                    }
                }
                ids
            };

            let chunk_index = g::load_chunk_index(&repo_paths)?;
            let mut required_bundle_ids = HashSet::new();
            for target_id in &remaining_targets {
                let manifest = g::load_target_manifest(&repo_paths, target_id)?;
                required_bundle_ids.extend(g::repo::required_bundle_ids(&manifest, &chunk_index));
            }

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
                        let bundle_still_needed = required_bundle_ids.contains(bundle_id);
                        if !bundle_still_needed {
                            orphan_bundles.push(entry.path().to_path_buf());
                        }
                    }
                }
            }

            for orphan_path in &orphan_bundles {
                std::fs::remove_file(orphan_path)?;
                if verbose_output_enabled() {
                    println!("removed orphaned bundle: {}", orphan_path.display());
                }
            }

            println!(
                "deleted target '{}': removed dir, {} orphaned bundles cleaned up",
                target,
                orphan_bundles.len()
            );
        }

        RepoCommands::SyncUp {
            repo,
            endpoint,
            product_id,
            content_repo_id,
            session_id,
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
    }

    Ok(())
}
