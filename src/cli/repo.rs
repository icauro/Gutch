use std::collections::HashMap;
use std::path::PathBuf;

use serde::Serialize;

use crate::cli::backend::{
    CreatorRepoTargetsResponse, UpsertTargetResponse, request_json_from_backend,
    slugify_like_backend, upload_raw_file,
};
use crate::cli::common::{
    load_optional_rules, parse_version_parts, session_id_for_cli,
};
use crate::cli::{SECTION_SEPARATOR};
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
    product_id: Option<i64>,
    content_repo_id: Option<i64>,
    session_override: Option<String>,
) -> g::Result<RepoSyncUpStats> {
    let (product_id, content_repo_id) = if let (Some(pid), Some(cid)) = (product_id, content_repo_id) {
        (pid, cid)
    } else {
        let config = g::config::load_config()?;
        let tracked = config
            .find_repo(&repo.display().to_string())
            .ok_or_else(|| {
                g::Error::manifest(format!(
                    "repo '{}' is not tracked. Use repo link first or pass --product-id and --content-repo-id.",
                    repo.display()
                ))
            })?;
        let linked = tracked.linked.as_ref().ok_or_else(|| {
            g::Error::manifest(format!(
                "repo '{}' has no linked remote. Use repo link first or pass --product-id and --content-repo-id.",
                repo.display()
            ))
        })?;
        let pid = linked
            .product_id
            .parse::<i64>()
            .map_err(|_| g::Error::manifest(format!("invalid linked product_id: {}", linked.product_id)))?;
        (pid, linked.remote_repo_id)
    };
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
    let repo_manifest_uploaded = true;

    let mut chunk_index_uploaded = false;
    if repo_paths.chunk_index_path.exists() {
        let chunk_index_bytes = std::fs::read(&repo_paths.chunk_index_path)?;
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
                target_lookup_path: None,
                linked: None,
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

        RepoCommands::DiscoverTargets {
            repo,
            lookup_root,
            json_out,
        } => {
            let candidates = g::discover_source_target_installs(&repo, &lookup_root)?;
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
            println!("lookup_root: {}", lookup_root.display());
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

        RepoCommands::Link {
            repo,
            endpoint,
            session_id,
            product_id,
            content_repo_id,
            remote_repo_slug,
        } => {
            if let (Some(product_id), Some(content_repo_id), Some(remote_repo_slug)) =
                (product_id, content_repo_id, remote_repo_slug)
            {
                run_repo_link_direct(
                    repo,
                    endpoint,
                    session_id,
                    product_id,
                    content_repo_id,
                    remote_repo_slug,
                )?;
            } else {
                run_repo_link_interactive(repo, endpoint, session_id)?;
            }
        }

        RepoCommands::Unlink { repo } => {
            let mut config = g::config::load_config()?;
            let repo_path = repo.display().to_string();
            if config.find_repo(&repo_path).is_none() {
                return Err(g::Error::manifest(format!(
                    "repo '{}' is not tracked in config.",
                    repo_path
                )));
            }
            config.upsert_repo(g::config::TrackedRepo {
                path: repo_path,
                target_lookup_path: None,
                linked: None,
            });
            g::config::save_config(&config)?;
            println!("unlinked repo '{}'", repo.display());
        }
    }

    Ok(())
}

fn run_repo_link_interactive(
    repo: PathBuf,
    endpoint: String,
    session_override: Option<String>,
) -> g::Result<()> {
    let session_id = session_id_for_cli(session_override)?;
    let client = reqwest::blocking::Client::builder()
        .user_agent("Gutch CLI")
        .build()
        .map_err(|err| g::Error::manifest(format!("failed to create link client: {}", err)))?;

    let repo_path = repo.display().to_string();
    let mut config = g::config::load_config()?;
    if let Some(existing) = config.find_repo(&repo_path) {
        if let Some(linked) = &existing.linked {
            eprintln!(
                "repo '{}' is already linked to product {} repo {}.",
                repo_path, linked.product_id, linked.remote_repo_slug
            );
            eprintln!("Use 'gutch repo unlink --repo {}' first.", shell_quote(&repo_path));
            return Err(g::Error::manifest("repo already linked"));
        }
    }

    let products: serde_json::Value = request_json_from_backend(
        client
            .get(format!(
                "{}/v1/users/me/creator/products",
                endpoint.trim_end_matches('/')
            ))
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", session_id)),
    )?;

    let product_list = products
        .as_array()
        .ok_or_else(|| g::Error::manifest("invalid products response"))?;
    if product_list.is_empty() {
        return Err(g::Error::manifest("no creator products found"));
    }

    println!();
    println!("Products:");
    for (i, product) in product_list.iter().enumerate() {
        let id = product["id"].as_i64().unwrap_or(0);
        let name = product["name"].as_str().unwrap_or("?");
        let slug = product["slug"].as_str().unwrap_or("?");
        println!("  [{}] {} (slug: {}, id: {})", i + 1, name, slug, id);
    }

    let choice = prompt_index("Select product", product_list.len())?;
    let product = &product_list[choice];
    let product_id = product["id"]
        .as_i64()
        .ok_or_else(|| g::Error::manifest("product missing id"))?;
    let product_id_str = product_id.to_string();

    let repos: serde_json::Value = request_json_from_backend(
        client
            .get(format!(
                "{}/v1/users/me/creator/products/{}/repos",
                endpoint.trim_end_matches('/'),
                product_id
            ))
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", session_id)),
    )?;

    let repo_list = repos
        .as_array()
        .ok_or_else(|| g::Error::manifest("invalid repos response"))?;
    if repo_list.is_empty() {
        return Err(g::Error::manifest(format!(
            "product {} has no repos",
            product_id
        )));
    }

    println!();
    println!("Repos:");
    for (i, r) in repo_list.iter().enumerate() {
        let id = r["id"].as_i64().unwrap_or(0);
        let slug = r["slug"].as_str().unwrap_or("?");
        println!("  [{}] repo id: {}, slug: {}", i + 1, id, slug);
    }

    let choice = prompt_index("Select repo", repo_list.len())?;
    let selected = &repo_list[choice];
    let remote_repo_id = selected["id"]
        .as_i64()
        .ok_or_else(|| g::Error::manifest("repo missing id"))?;
    let remote_repo_slug = selected["slug"]
        .as_str()
        .ok_or_else(|| g::Error::manifest("repo missing slug"))?
        .to_string();

    config.upsert_repo(g::config::TrackedRepo {
        path: repo_path.clone(),
        target_lookup_path: None,
        linked: Some(g::config::LinkedRepo {
            product_id: product_id_str,
            creator_user_id: None,
            remote_repo_id,
            remote_repo_slug: remote_repo_slug.clone(),
        }),
    });
    g::config::save_config(&config)?;

    println!(
        "linked repo '{}' to product {} repo {} (id: {})",
        repo_path, product_id, remote_repo_slug, remote_repo_id
    );
    Ok(())
}

fn run_repo_link_direct(
    repo: PathBuf,
    _endpoint: String,
    _session_override: Option<String>,
    product_id: i64,
    content_repo_id: i64,
    remote_repo_slug: String,
) -> g::Result<()> {
    let repo_path = repo.display().to_string();
    let mut config = g::config::load_config()?;
    if let Some(existing) = config.find_repo(&repo_path) {
        if let Some(linked) = &existing.linked {
            return Err(g::Error::manifest(format!(
                "repo '{}' is already linked to product {} repo {}.",
                repo_path, linked.product_id, linked.remote_repo_slug
            )));
        }
    }

    config.upsert_repo(g::config::TrackedRepo {
        path: repo_path.clone(),
        target_lookup_path: None,
        linked: Some(g::config::LinkedRepo {
            product_id: product_id.to_string(),
            creator_user_id: None,
            remote_repo_id: content_repo_id,
            remote_repo_slug: remote_repo_slug.clone(),
        }),
    });
    g::config::save_config(&config)?;

    println!(
        "linked repo '{}' to product {} repo {} (id: {})",
        repo_path, product_id, remote_repo_slug, content_repo_id
    );
    Ok(())
}

fn prompt_index(prompt: &str, count: usize) -> g::Result<usize> {
    use std::io::{self, Write};
    loop {
        print!("{} [1-{}]: ", prompt, count);
        io::stdout().flush().map_err(|err| g::Error::manifest(format!("io error: {}", err)))?;
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .map_err(|err| g::Error::manifest(format!("io error: {}", err)))?;
        match line.trim().parse::<usize>() {
            Ok(n) if n >= 1 && n <= count => return Ok(n - 1),
            _ => eprintln!("enter a number from 1 to {}", count),
        }
    }
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '\\'))
    {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('"', "\\\""))
    }
}
