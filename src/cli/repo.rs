use crate::cli::RepoCommands;
use crate::cli::SECTION_SEPARATOR;
use crate::cli::actions;
use crate::cli::common::load_optional_rules;
use crate::cli::reporting::ensure_parent_directory;
use gutch as g;

pub fn run(command: RepoCommands) -> g::Result<()> {
    match command {
        RepoCommands::Init { path, schema, force } => {
            actions::repo::init_repo(&path, schema.as_ref(), force)?;
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
                crate::helpers::write(json_out, serde_json::to_vec_pretty(&status)?)?;
            }

            if status.is_repo {
                println!("{SECTION_SEPARATOR}");
                println!("Targets");
                for target in &status.targets {
                    println!(
                        "{} | version={} | os={} | arch={} | files={} | missing_bundles={}",
                        target.target_id,
                        target.version_id,
                        target.os,
                        target.arch,
                        target.file_count,
                        target.missing_bundle_count
                    );
                }
                println!("{SECTION_SEPARATOR}");
                println!("Total");
                println!("name: {}", status.name);
                println!(
                    "remote_repo_id: {}",
                    if status.remote_repo_id.is_empty() {
                        "(not linked)"
                    } else {
                        &status.remote_repo_id
                    }
                );
                println!(
                    "product_id: {}",
                    if status.product_id.is_empty() {
                        "(not linked)"
                    } else {
                        &status.product_id
                    }
                );
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

        RepoCommands::TargetSources { command } => {
            crate::cli::target_sources::run(command)?;
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
                crate::helpers::write(json_out, serde_json::to_vec_pretty(&status.targets)?)?;
            }

            println!("{SECTION_SEPARATOR}");
            println!("Targets");
            for target in &status.targets {
                println!("{}", target.target_id);
            }
            println!("{SECTION_SEPARATOR}");
            println!("Total");
            println!("name: {}", status.name);
            println!(
                "remote_repo_id: {}",
                if status.remote_repo_id.is_empty() {
                    "(not linked)"
                } else {
                    &status.remote_repo_id
                }
            );
            println!("targets: {}", status.targets.len());
            println!("bundles: {}", status.bundle_count);
        }

        RepoCommands::SyncUp {
            repo,
            endpoint,
            session_id,
            yes,
        } => {
            let plan = actions::repo::plan_sync_up_repo(
                repo.clone(),
                endpoint.clone(),
                session_id.clone(),
            )?;
            print_sync_plan(&plan);
            if !yes && !prompt_yes_no("Proceed?", true)? {
                println!("sync-up cancelled");
                return Ok(());
            }
            let stats = actions::repo::sync_up_repo(repo, endpoint, session_id)?;
            println!(
                "synced repo {} to product {} repo {}",
                stats.repo_root, stats.product_id, stats.remote_repo_id
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
        } => {
            let product_id = product_id
                .ok_or_else(|| g::Error::manifest("--product-id is required for linking"))?;
            actions::repo::link_repo(&repo, product_id, &endpoint, session_id)?;
        }

        RepoCommands::Unlink {
            repo,
            delete_remote,
            endpoint,
            session_id,
        } => {
            actions::repo::unlink_repo(
                &repo,
                delete_remote,
                if delete_remote { Some(&endpoint) } else { None },
                session_id,
            )?;
        }
    }

    Ok(())
}

fn print_sync_plan(plan: &g::RepoSyncPlan) {
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

fn print_sync_section(label: &str, changes: &[g::RepoSyncTargetChange]) {
    if changes.is_empty() {
        return;
    }
    println!("{SECTION_SEPARATOR}");
    println!("{}", label);
    for change in changes {
        println!(
            "{} | version={} | os={} | arch={}{}",
            change.target_id,
            change.version,
            change.os,
            change.arch,
            change
                .artifact_state
                .as_deref()
                .map(|value| format!(" | remote_state={}", value))
                .unwrap_or_default()
        );
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
