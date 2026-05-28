use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::Serialize;

use crate::cli_backend::{
    CreatorRepoTargetsResponse, SignedChunkIndexResponse, SignedRepoManifestResponse,
    SignedTargetManifestResponse, UpsertTargetResponse, request_json_from_backend,
    request_optional_bytes_from_signed_url, slugify_like_backend, upload_raw_file,
};
use crate::cli_common::{
    collect_input_sizing_stats, directory_has_any_entries, load_optional_rules,
    load_target_manifest_from_repo, median_u64, parse_version_parts, session_id_for_cli,
    write_bundles, write_target_manifest,
};
use crate::cli_reporting::{
    accumulate_build_timings, ensure_parent_directory, print_build_timings,
    print_repo_init_storage_summary,
};
use crate::{RepoCommands, RepoInitMode, RepoInitSpec};
use g::cli_identity::resolve_target_identity;
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
        let manifest_bytes = std::fs::read(&target.manifest_path)?;
        let manifest: g::manifest::target::TargetManifest =
            serde_json::from_slice(&manifest_bytes)?;
        let target_id = manifest.target_id.parse::<i64>().map_err(|_| {
            g::Error::manifest(format!(
                "target '{}' has a non-numeric local target id; repo sync-up expects numeric target ids",
                manifest.target_id
            ))
        })?;
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
                            "targetId": target_id,
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
                crate::cli_backend::parse_backend_error_response(exists_response),
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
        RepoCommands::Create {
            content_repo,
            schema,
            repo,
        } => {
            let repo_paths = g::ContentRepoPaths::from_repo_root(&repo);
            let mut repo_manifest = g::ensure_content_repo_layout(&repo_paths, &content_repo)?;
            repo_manifest.target_schema = load_optional_rules(&schema)?;
            g::write_content_repo_manifest(&repo_paths, &repo_manifest)?;

            println!(
                "created content repo {} at {}",
                content_repo,
                repo_paths.repo_root.display()
            );
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

        RepoCommands::Init {
            content_repo,
            spec,
            init_mode,
            chunk_profile,
            bundle_profile,
            compression_unit,
            repo,
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

            let mut repo_manifest = g::ensure_content_repo_layout(&repo_paths, &content_repo)?;
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
                target_input.target_id = g::generate_target_id();
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
                        &content_repo,
                        &target_inputs,
                        &chunk_sizing_profile,
                        &bundle_sizing_profile,
                        &compression_profile,
                    )?;
                    let mut target_manifest_sizes = HashMap::new();

                    for manifest in &mut result.target_manifests {
                        let target_root = repo_paths.targets_root.join(&manifest.target_id);
                        let (_, manifest_size) = write_target_manifest(&target_root, &*manifest)?;
                        target_manifest_sizes.insert(manifest.target_id.clone(), manifest_size);
                    }

                    let bundle_sizes = write_bundles(&repo_paths.bundles_root, &result.bundles)?;

                    println!(
                        "initialized content repo {} with {} targets using batch mode",
                        content_repo,
                        result.target_manifests.len()
                    );
                    let ordered_target_manifests: Vec<&g::manifest::target::TargetManifest> =
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
                        let (result, timings) = g::build_from_directory_with_timings_against_existing_store_and_manifests_and_profiles(
                            &target.dir,
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
                        let (_, manifest_size) =
                            write_target_manifest(&target_root, &target_manifest)?;
                        target_manifest_sizes
                            .insert(target_manifest.target_id.clone(), manifest_size);
                        ordered_target_manifests.push(target_manifest.clone());

                        for file in &target_manifest.files {
                            previous_file_manifests
                                .entry(file.path.clone())
                                .or_insert_with(|| file.clone());
                        }

                        let written_bundle_sizes =
                            write_bundles(&repo_paths.bundles_root, &result.bundles)?;
                        for bundle in &result.bundles {
                            if let Some(bundle_size) = written_bundle_sizes.get(&bundle.bundle_id) {
                                bundle_sizes.insert(bundle.bundle_id.clone(), *bundle_size);
                            }
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
                    let ordered_target_manifest_refs: Vec<&g::manifest::target::TargetManifest> =
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

            repo_manifest.chunk_sizing_profile = chunk_sizing_profile;
            repo_manifest.bundle_sizing_profile = bundle_sizing_profile;
            repo_manifest.compression_profile = compression_profile;
            g::write_content_repo_manifest(&repo_paths, &repo_manifest)?;
        }

        RepoCommands::Inspect { repo, json_out } => {
            let status = g::inspect_content_repo(&repo)?;
            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                std::fs::write(json_out, serde_json::to_vec_pretty(&status)?)?;
            }

            if status.is_repo {
                println!(
                    "inspected content repo {} at {}",
                    status.content_repo_id, status.repo_path
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

            println!(
                "discovered {} matching source target folders in {}",
                candidates.len(),
                source_targets_root.display()
            );
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

        RepoCommands::DeleteTarget { repo, target, yes } => {
            let (repo_paths, _repo_manifest, _, _tm) =
                load_target_manifest_from_repo(&repo, &target)?;
            let target_root = repo_paths.targets_root.join(&target);
            if !yes {
                eprintln!(
                    "WARNING: this will remove '{}' and may orphan bundles. Use --yes to confirm.",
                    target_root.display()
                );
                return Err(g::Error::manifest("deletion requires --yes flag"));
            }

            if target_root.exists() {
                std::fs::remove_dir_all(&target_root)?;
                println!("removed target: {}", target);
            }

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
