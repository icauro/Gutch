use crate::cli::RepoCommands;
use crate::cli::SECTION_SEPARATOR;
use crate::cli::actions;
use crate::cli::common::load_optional_rules;
use crate::cli::reporting::{
    ensure_parent_directory, print_repo_check_timings, print_tracked_repo_check_timings,
};
use gutch as g;

pub fn run(command: RepoCommands) -> g::Result<()> {
    match command {
        RepoCommands::Init {
            path,
            schema,
            force,
        } => {
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
                repo_paths.repo_metadata_path.display()
            );
        }

        RepoCommands::LaunchSet {
            repo,
            launch,
            clear,
        } => {
            if clear == launch.is_some() {
                return Err(g::Error::manifest(
                    "provide either --launch <path> or --clear",
                ));
            }
            let configuration = if clear {
                None
            } else {
                let path = launch.as_ref().expect("launch path checked");
                let json = crate::helpers::read_to_string(path)?;
                Some(serde_json::from_str::<g::RepoLaunchConfiguration>(&json)?)
            };
            g::update_gutch_repo_launch(&repo, configuration)?;
            println!("updated repo launch configuration");
        }

        RepoCommands::Check { repo, json_out } => {
            let tracked = g::check_tracked_gutch_repo(&repo)?;
            let status = tracked.repo_status.clone();
            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                crate::helpers::write(json_out, serde_json::to_vec_pretty(&tracked)?)?;
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
                if let (Some(repo_timings), Some(tracked_timings)) =
                    (status.timings.as_ref(), tracked.timings.as_ref())
                {
                    println!();
                    print_tracked_repo_check_timings(repo_timings, tracked_timings);
                } else if let Some(repo_timings) = status.timings.as_ref() {
                    println!();
                    print_repo_check_timings(repo_timings);
                }
            } else if let Some(error) = status.error.as_deref() {
                println!("check status: {}", error);
            } else {
                println!("check status: no gutch repo found at {}", status.repo_path);
            }
        }

        RepoCommands::Verify {
            repo,
            target,
            strict,
            json_out,
        } => {
            let mode = if strict {
                g::VerifyMode::Strict
            } else {
                g::VerifyMode::Fast
            };
            let result = g::verify_gutch_repo(&repo, target.as_deref(), mode)?;
            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                crate::helpers::write(json_out, serde_json::to_vec_pretty(&result)?)?;
            }

            println!("{SECTION_SEPARATOR}");
            println!("Verify ({})", result.mode);
            println!("ok: {}", result.ok);
            println!("targets: {} ok={}", result.target_count, result.targets_ok);
            for target_result in &result.targets {
                println!(
                    "{} | ok={} | missing={} | corrupt={}",
                    target_result.target_id,
                    target_result.ok,
                    target_result.missing_bundle_count,
                    target_result.corrupt_bundle_count
                );
                for missing in &target_result.missing_bundles {
                    println!("  missing bundle: {missing}");
                }
                for corrupt in &target_result.corrupt_bundles {
                    println!("  corrupt bundle: {corrupt}");
                }
            }
        }

        RepoCommands::TargetSources { command } => {
            crate::cli::target_sources::run(command)?;
        }

        RepoCommands::List { repo, json_out } => {
            let status = g::check_gutch_repo(&repo)?;
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

        RepoCommands::Push {
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
                println!("push cancelled");
                return Ok(());
            }
            let progress = crate::cli::progress::CliProgressSink::new();
            let stats = actions::repo::sync_up_repo(repo, endpoint, session_id, Some(progress))?;
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

        RepoCommands::Pull {
            repo,
            remote,
            endpoint,
            session_id,
            on_conflict,
            prune,
            yes,
        } => {
            let policy = g::PullConflictPolicy::parse(&on_conflict)?;
            let plan = actions::repo::plan_repo_pull(
                repo.clone(),
                remote.clone(),
                endpoint.clone(),
                session_id.clone(),
                policy,
            )?;
            print_pull_plan(&plan);
            if !yes && !prompt_yes_no("Proceed?", true)? {
                println!("pull cancelled");
                return Ok(());
            }
            let progress = crate::cli::progress::CliProgressSink::new();
            let stats = actions::repo::pull_repo(
                repo,
                remote,
                endpoint,
                session_id,
                policy,
                prune,
                Some(progress),
            )?;
            println!(
                "pulled repo {} from product {} repo {} ({})",
                stats.repo_root,
                stats.product_id,
                stats.remote_repo_id,
                if prune { "with prune" } else { "no prune" }
            );
            println!(
                "targets downloaded: {}, skipped: {}, bundles downloaded: {}, bundles skipped: {}",
                stats.target_downloaded_count,
                stats.target_skipped_count,
                stats.bundle_downloaded_count,
                stats.bundle_skipped_count
            );
            if stats.repo_manifest_downloaded {
                println!("repo manifest downloaded");
            }
            if stats.chunk_map_downloaded {
                println!("chunk map downloaded");
            }
            if stats.kept_newer_local_count > 0 {
                println!("kept newer local: {}", stats.kept_newer_local_count);
            }
            if stats.local_only_targets_kept > 0 {
                println!("kept local-only targets: {}", stats.local_only_targets_kept);
            }
            if stats.pruned_target_count > 0 || stats.pruned_bundle_count > 0 {
                println!(
                    "pruned targets: {}, bundles: {}",
                    stats.pruned_target_count, stats.pruned_bundle_count
                );
            }
        }

        RepoCommands::Remote {
            command:
                crate::cli::RemoteCommands::List {
                    endpoint,
                    session_id,
                },
        } => {
            let entries = actions::repo::list_remote_repos(endpoint, session_id)?;
            for entry in &entries {
                println!("{}/{}", entry.product_name, entry.repo_name);
            }
            if entries.is_empty() {
                println!("no remote repos found");
            }
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

fn print_pull_plan(plan: &g::RepoPullPlan) {
    println!("{SECTION_SEPARATOR}");
    println!("Pull plan");
    println!("repo: {}", plan.repo_root);
    println!(
        "remote: product {} repo {} ({})",
        plan.product_id, plan.remote_repo_id, plan.remote_repo_name
    );
    println!(
        "changes: repo_manifest={} chunk_map={} targets_download={} targets_skip={} local_only={} kept_newer_local={}",
        plan.repo_manifest_needed,
        plan.chunk_map_needed,
        plan.targets_to_download.len(),
        plan.targets_to_skip.len(),
        plan.local_only_targets.len(),
        plan.kept_newer_local_targets.len()
    );
    print_pull_section("Download", &plan.targets_to_download);
    print_pull_section("Local only", &plan.local_only_targets);
    if !plan.kept_newer_local_targets.is_empty() {
        println!("kept newer local:");
        for target_id in &plan.kept_newer_local_targets {
            println!("  {}", target_id);
        }
    }
}

fn print_pull_section(label: &str, target_ids: &[String]) {
    if target_ids.is_empty() {
        return;
    }
    println!("{SECTION_SEPARATOR}");
    println!("{}", label);
    for target_id in target_ids {
        println!("{}", target_id);
    }
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
