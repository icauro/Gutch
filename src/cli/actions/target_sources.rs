use std::path::Path;
use std::sync::Arc;

use crate::cli::common::prompt_yes_no;
use crate::cli::progress::CliProgressSink;
use gutch as g;

pub type TargetSourcesSyncOptions = CliTargetSourcesSyncOptions;
pub type TargetSourcesSyncResult = g::TargetSourcesSyncResult;

#[derive(Debug, Clone, Copy)]
pub struct CliTargetSourcesSyncOptions {
    pub no_prune: bool,
    pub yes: bool,
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
    options: CliTargetSourcesSyncOptions,
) -> g::Result<TargetSourcesSyncResult> {
    let plan = g::plan_target_sources_sync(repo, Some(sources_path))?;
    if !plan.sources_ready {
        return Err(g::Error::manifest(
            "target sources path is missing or invalid",
        ));
    }

    let prune = !options.no_prune;

    if !plan.has_changes() {
        println!("target sources sync: no changes");
        return Ok(g::TargetSourcesSyncResult {
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
        return Ok(g::TargetSourcesSyncResult {
            inserted_count: 0,
            tag_updated_count: 0,
            replaced_same_tags_count: 0,
            removed_count: 0,
            pruned: prune,
        });
    }

    let progress: Arc<dyn g::GutchProgressSink> = CliProgressSink::new();
    let result = g::sync_target_sources(
        repo,
        sources_path,
        g::TargetSourcesSyncOptions {
            prune_removed_targets: prune,
        },
        &Some(progress),
    )?;

    println!(
        "target sources sync complete: inserted {}, tag updates {}, same-tag replacements {}, pruned {}{}",
        result.inserted_count,
        result.tag_updated_count,
        result.replaced_same_tags_count,
        result.removed_count,
        if prune {
            String::new()
        } else {
            " (--no-prune)".to_string()
        }
    );

    Ok(result)
}
