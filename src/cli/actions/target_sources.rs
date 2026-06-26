use std::collections::HashSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::cli::TargetCommands;
use gutch as g;

#[derive(Debug, Clone, Copy)]
pub struct TargetSourcesSyncOptions {
    pub dry_run: bool,
    pub no_prune: bool,
    pub yes: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetSourcesSyncResult {
    pub inserted_count: usize,
    pub tag_updated_count: usize,
    pub removed_count: usize,
    pub pruned: bool,
    pub dry_run: bool,
}


fn delete_repo_targets_with_matching_tags(
    repo: &Path,
    candidate: &g::SourceTargetInstallCandidate,
) -> g::Result<usize> {
    let status = g::inspect_gutch_repo(repo)?;
    let mut removed_count = 0;
    for target in status.targets {
        if target.target_tags != candidate.target_tags {
            continue;
        }
        crate::cli::target::run(TargetCommands::Delete {
            repo: repo.to_path_buf(),
            target: target.target_id,
            yes: true,
        })?;
        removed_count += 1;
    }
    Ok(removed_count)
}

fn insert_candidate(repo: &Path, candidate: &g::SourceTargetInstallCandidate) -> g::Result<()> {
    let (_, repo_manifest) = g::load_gutch_repo_manifest(repo)?;
    let compression_profile = repo_manifest.compression_profile.preset;
    crate::cli::target::run(TargetCommands::Insert {
        dir: PathBuf::from(&candidate.path),
        repo: repo.to_path_buf(),
        tags: Vec::new(),
        chunk_profile: g::manifest::ArtifactSizingPreset::Chunk128KiB,
        bundle_profile: g::manifest::BundleSizingPreset::Bundle16MiB,
        compression_profile,
        storage_mode: g::manifest::ArtifactStorageMode::Bundled,
        memory_mode: g::MemoryMode::Auto,
        max_memory_mib: None,
        jobs: None,
        summary_out: None,
        stats_json_out: None,
    })
}

fn prompt_yes_no(prompt: &str, default_yes: bool) -> g::Result<bool> {
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

pub fn sync_target_sources(
    repo: &Path,
    sources_path: &Path,
    options: TargetSourcesSyncOptions,
) -> g::Result<TargetSourcesSyncResult> {
    let plan = g::plan_target_sources_sync(repo, Some(sources_path))?;
    if !plan.sources_ready {
        return Err(g::Error::manifest(
            "target sources path is missing or invalid",
        ));
    }

    let candidates = g::discover_source_target_installs(repo, sources_path)?;
    let prune = !options.no_prune;
    let removable_count = if prune { plan.removable_count } else { 0 };

    if options.dry_run {
        println!("target sources sync (dry run)");
        println!("candidates: {}", plan.candidate_count);
        println!("new targets: {}", plan.insertable_count);
        println!("tag updates: {}", plan.tag_update_count);
        println!("to refresh: {}", plan.refresh_count());
        println!(
            "to prune: {}{}",
            plan.removable_count,
            if prune {
                String::new()
            } else {
                " (skipped with --no-prune)".to_string()
            }
        );
        for update in &plan.to_update_tags {
            println!(
                "  tag update: {} ({})",
                update.content_target_id, update.folder_name
            );
        }
        return Ok(TargetSourcesSyncResult {
            inserted_count: plan.insertable_count,
            tag_updated_count: plan.tag_update_count,
            removed_count: removable_count,
            pruned: prune,
            dry_run: true,
        });
    }

    if plan.has_changes() && !options.yes && !prompt_yes_no("Proceed?", true)? {
        println!("target sources sync cancelled");
        return Ok(TargetSourcesSyncResult {
            inserted_count: 0,
            tag_updated_count: 0,
            removed_count: 0,
            pruned: prune,
            dry_run: false,
        });
    }

    let mut removed_count = 0;
    if prune {
        for target_id in &plan.to_remove {
            crate::cli::target::run(TargetCommands::Delete {
                repo: repo.to_path_buf(),
                target: target_id.clone(),
                yes: true,
            })?;
            removed_count += 1;
        }
    }

    let tag_update_ids: HashSet<String> = plan
        .to_update_tags
        .iter()
        .map(|update| update.content_target_id.clone())
        .collect();

    let mut inserted_count = 0;
    let mut tag_updated_count = 0;
    for candidate in &candidates {
        removed_count += delete_repo_targets_with_matching_tags(repo, candidate)?;
        insert_candidate(repo, candidate)?;
        if tag_update_ids.contains(&candidate.content_target_id) {
            tag_updated_count += 1;
        } else {
            inserted_count += 1;
        }
    }

    println!(
        "target sources sync complete: inserted {}, tag updates {}, pruned {}{}",
        inserted_count,
        tag_updated_count,
        removed_count,
        if prune {
            String::new()
        } else {
            " (--no-prune)".to_string()
        }
    );

    Ok(TargetSourcesSyncResult {
        inserted_count,
        tag_updated_count,
        removed_count,
        pruned: prune,
        dry_run: false,
    })
}
