use std::path::PathBuf;

use crate::cli::RepoCommands;
use crate::cli::SECTION_SEPARATOR;
use crate::cli::actions;
use crate::cli::common::{load_optional_rules, session_id_for_cli};
use crate::cli::reporting::ensure_parent_directory;
use g::remote::backend::request_json_from_backend;
use gutch as g;

pub fn run(command: RepoCommands) -> g::Result<()> {
    match command {
        RepoCommands::Init { path, schema } => {
            actions::repo::init_repo(&path, schema.as_ref())?;
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

            let (repo_paths, mut repo_manifest) = g::load_gutch_repo_manifest(&repo)?;
            repo_manifest.target_schema = if clear {
                None
            } else {
                load_optional_rules(&schema)?
            };
            g::write_gutch_repo_manifest(&repo_paths, &repo_manifest)?;
            println!(
                "updated repo schema at {}",
                repo_paths.repo_manifest_path.display()
            );
        }

        RepoCommands::Inspect { repo, json_out } => {
            let status = g::inspect_gutch_repo(&repo)?;
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
                println!("gutch_repo_id: {}", status.gutch_repo_id);
                println!("repo_path: {}", status.repo_path);
                println!("targets: {}", status.targets.len());
                println!("bundles: {}", status.bundle_count);
                println!("bundle_bytes: {}", status.bundle_bytes);
            } else if let Some(error) = status.error.as_deref() {
                println!("inspect status: {}", error);
            } else {
                println!(
                    "inspect status: no gutch repo found at {}",
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
            let status = g::inspect_gutch_repo(&repo)?;
            if !status.is_repo {
                return Err(g::Error::manifest(format!(
                    "no gutch repo found at {}",
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
            println!("gutch_repo_id: {}", status.gutch_repo_id);
            println!("targets: {}", status.targets.len());
            println!("bundles: {}", status.bundle_count);
        }

        RepoCommands::SyncUp {
            repo,
            endpoint,
            product_id,
            gutch_repo_id,
            session_id,
            yes,
        } => {
            let plan = actions::repo::plan_sync_up_repo(
                repo.clone(),
                endpoint.clone(),
                product_id,
                gutch_repo_id,
                session_id.clone(),
            )?;
            print_sync_plan(&plan);
            if !yes && !prompt_yes_no("Proceed?", true)? {
                println!("sync-up cancelled");
                return Ok(());
            }
            let stats = actions::repo::sync_up_repo(repo, endpoint, product_id, gutch_repo_id, session_id)?;
            println!(
                "synced repo {} to product {} repo {}",
                stats.repo_root, stats.product_id, stats.gutch_repo_id
            );
            println!(
                "targets added: {}, removed: {}, updated: {}, unchanged: {}",
                stats.target_added_count,
                stats.target_removed_count,
                stats.target_updated_count,
                stats.target_unchanged_count
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
            gutch_repo_id,
            remote_repo_slug,
        } => {
            if let (Some(product_id), Some(gutch_repo_id), Some(remote_repo_slug)) =
                (product_id, gutch_repo_id, remote_repo_slug)
            {
                run_repo_link_direct(
                    repo,
                    endpoint,
                    session_id,
                    product_id,
                    gutch_repo_id,
                    remote_repo_slug,
                )?;
            } else {
                run_repo_link_interactive(repo, endpoint, session_id)?;
            }
        }

        RepoCommands::Unlink { repo } => {
            actions::repo::unlink_repo(&repo)?;
        }
    }

    Ok(())
}

fn print_sync_plan(plan: &actions::repo::RepoSyncPlan) {
    println!("{SECTION_SEPARATOR}");
    println!("Sync plan");
    println!("repo: {}", plan.repo_root);
    println!(
        "targets: local={} remote={}",
        plan.target_count, plan.remote_target_count
    );
    println!(
        "changes: add={} remove={} update={} unchanged={}",
        plan.to_add.len(),
        plan.to_remove.len(),
        plan.to_update.len(),
        plan.unchanged.len()
    );
    print_sync_section("Add", &plan.to_add);
    print_sync_section("Remove", &plan.to_remove);
    print_sync_section("Update", &plan.to_update);
}

fn print_sync_section(label: &str, changes: &[actions::repo::RepoSyncTargetChange]) {
    if changes.is_empty() {
        return;
    }
    println!("{SECTION_SEPARATOR}");
    println!("{}", label);
    for change in changes {
        println!(
            "{} | version={} | os={} | arch={} | tier={}{}",
            change.target_id,
            change.version,
            change.platform,
            change.architecture,
            change.tier,
            change
                .artifact_state
                .as_deref()
                .map(|value| format!(" | remote_state={}", value))
                .unwrap_or_default()
        );
    }
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
            eprintln!(
                "Use 'plctl gutch repo unlink --repo {}' first.",
                shell_quote(&repo_path)
            );
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

    loop {
        println!();
        println!("Products:");
        for (i, product) in product_list.iter().enumerate() {
            let id = product["id"].as_i64().unwrap_or(0);
            let name = product["name"].as_str().unwrap_or("?");
            let slug = product["slug"].as_str().unwrap_or("?");
            println!("  [{}] {} (slug: {}, id: {})", i + 1, name, slug, id);
        }
        println!("  [0] Back");

        let Some(choice) = prompt_index_or_back("Select product", product_list.len())? else {
            return Err(g::Error::manifest("interactive command cancelled"));
        };
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
            println!("product {} has no repos.", product_id);
            continue;
        }

        println!();
        println!("Repos:");
        for (i, r) in repo_list.iter().enumerate() {
            let id = r["id"].as_i64().unwrap_or(0);
            let slug = r["slug"].as_str().unwrap_or("?");
            println!("  [{}] repo id: {}, slug: {}", i + 1, id, slug);
        }
        println!("  [0] Back");

        let Some(choice) = prompt_index_or_back("Select repo", repo_list.len())? else {
            continue;
        };
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
        return Ok(());
    }
}

fn run_repo_link_direct(
    repo: PathBuf,
    _endpoint: String,
    _session_override: Option<String>,
    product_id: i64,
    gutch_repo_id: i64,
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
            remote_repo_id: gutch_repo_id,
            remote_repo_slug: remote_repo_slug.clone(),
        }),
    });
    g::config::save_config(&config)?;

    println!(
        "linked repo '{}' to product {} repo {} (id: {})",
        repo_path, product_id, remote_repo_slug, gutch_repo_id
    );
    Ok(())
}

fn prompt_index_or_back(prompt: &str, count: usize) -> g::Result<Option<usize>> {
    use std::io::{self, Write};
    loop {
        print!("{} [0-{}]: ", prompt, count);
        io::stdout()
            .flush()
            .map_err(|err| g::Error::manifest(format!("io error: {}", err)))?;
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .map_err(|err| g::Error::manifest(format!("io error: {}", err)))?;
        match line.trim().parse::<usize>() {
            Ok(0) => return Ok(None),
            Ok(n) if n >= 1 && n <= count => return Ok(Some(n - 1)),
            _ => eprintln!("enter 0 to go back, or a number from 1 to {}", count),
        }
    }
}

fn prompt_yes_no(prompt: &str, default_yes: bool) -> g::Result<bool> {
    use std::io::{self, Write};

    loop {
        let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
        print!("{} {}: ", prompt, suffix);
        io::stdout()
            .flush()
            .map_err(|err| g::Error::manifest(format!("io error: {}", err)))?;
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .map_err(|err| g::Error::manifest(format!("io error: {}", err)))?;
        let trimmed = line.trim().to_ascii_lowercase();
        if trimmed.is_empty() {
            return Ok(default_yes);
        }
        if trimmed == "y" || trimmed == "yes" {
            return Ok(true);
        }
        if trimmed == "n" || trimmed == "no" {
            return Ok(false);
        }
        eprintln!("enter y or n");
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
