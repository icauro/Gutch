use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::cli::TargetCommands;
use crate::cli::common::prompt_yes_no;
use gutch as g;

#[derive(Debug, Clone, Copy)]
pub struct TargetSourcesSyncOptions {
    pub no_prune: bool,
    pub yes: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetSourcesSyncResult {
    pub inserted_count: usize,
    pub tag_updated_count: usize,
    pub replaced_same_tags_count: usize,
    pub removed_count: usize,
    pub pruned: bool,
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
        yes: true,
    })
}

fn print_target_sources_sync_plan(plan: &g::TargetSourcesSyncPlan, prune: bool) {
    println!("target sources sync plan:");
    println!("candidates: {}", plan.candidate_count);
    println!("new targets: {}", plan.insertable_count);
    println!("tag updates: {}", plan.tag_update_count);
    println!("same-tag replacements: {}", plan.replace_same_tags_count);
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
    for line in g::describe_target_sources_sync_plan(plan) {
        println!("  {line}");
    }
    if plan.replace_same_tags_count > 0 {
        println!("  Patron installs pinned to replaced content ids become invalid until updated.");
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

    if !plan.has_changes() {
        println!("target sources sync: no changes");
        return Ok(TargetSourcesSyncResult {
            inserted_count: 0,
            tag_updated_count: 0,
            replaced_same_tags_count: 0,
            removed_count: 0,
            pruned: prune,
        });
    }

    print_target_sources_sync_plan(&plan, prune);

    if !options.yes && !prompt_yes_no("Proceed?", false)? {
        println!("target sources sync cancelled");
        return Ok(TargetSourcesSyncResult {
            inserted_count: 0,
            tag_updated_count: 0,
            replaced_same_tags_count: 0,
            removed_count: 0,
            pruned: prune,
        });
    }

    let mut removed_count = 0;
    if prune {
        for target_id in &plan.to_remove {
            g::remove_target_from_repo(repo, target_id)?;
            removed_count += 1;
        }
    }

    let tag_update_ids: HashSet<String> = plan
        .to_update_tags
        .iter()
        .map(|update| update.content_target_id.clone())
        .collect();
    let replace_ids: HashSet<String> = plan
        .to_replace_same_tags
        .iter()
        .map(|replace| replace.content_target_id.clone())
        .collect();

    let mut inserted_count = 0;
    let mut tag_updated_count = 0;
    let mut replaced_same_tags_count = 0;
    for candidate in &candidates {
        removed_count +=
            g::delete_repo_targets_with_matching_tags(repo, &candidate.target_tags)?.len();
        insert_candidate(repo, candidate)?;
        if tag_update_ids.contains(&candidate.content_target_id) {
            tag_updated_count += 1;
        } else if replace_ids.contains(&candidate.content_target_id) {
            replaced_same_tags_count += 1;
        } else {
            inserted_count += 1;
        }
    }

    let _ = g::cleanup_orphan_bundles_in_repo(repo)?;

    println!(
        "target sources sync complete: inserted {}, tag updates {}, same-tag replacements {}, pruned {}{}",
        inserted_count,
        tag_updated_count,
        replaced_same_tags_count,
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
        replaced_same_tags_count,
        removed_count,
        pruned: prune,
    })
}
