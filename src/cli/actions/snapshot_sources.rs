use std::path::Path;
use std::sync::Arc;

use crate::cli::common::prompt_yes_no;
use crate::cli::progress::CliProgressSink;
use gutch as g;

#[derive(Debug, Clone, Copy)]
pub struct CliSnapshotSourcesSyncOptions {
    pub no_prune: bool,
    pub yes: bool,
}

fn print_snapshot_sources_sync_plan(plan: &g::SnapshotSourcesSyncPlan, prune: bool) {
    println!("snapshot sources sync plan:");
    println!("candidates: {}", plan.candidate_count);
    println!("new snapshots: {}", plan.creatable_count);
    println!("refreshes: {}", plan.refresh_count);
    println!("up to date: {}", plan.up_to_date_count());
    println!(
        "to prune: {}{}",
        plan.removable_count,
        if prune {
            String::new()
        } else {
            " (skipped with --no-prune)".to_string()
        }
    );
    for line in g::describe_snapshot_sources_sync_plan(plan) {
        println!("  {line}");
    }
    if plan.refresh_count > 0 {
        println!("  Existing snapshot references to replaced snapshots must be updated.");
    }
}

pub fn sync_snapshot_sources(
    repo: &Path,
    sources_path: &Path,
    options: CliSnapshotSourcesSyncOptions,
) -> g::Result<g::SnapshotSourcesSyncResult> {
    let plan = g::plan_snapshot_sources_sync(repo, Some(sources_path))?;
    if !plan.sources_ready {
        return Err(g::Error::manifest(
            "snapshot sources path is missing or invalid",
        ));
    }

    let prune = !options.no_prune;

    if !plan.has_changes() {
        println!("snapshot sources sync: no changes");
        return Ok(g::SnapshotSourcesSyncResult {
            created_count: 0,
            refreshed_count: 0,
            removed_count: 0,
            pruned: prune,
        });
    }

    print_snapshot_sources_sync_plan(&plan, prune);

    if !options.yes && !prompt_yes_no("Proceed?", false)? {
        println!("snapshot sources sync cancelled");
        return Ok(g::SnapshotSourcesSyncResult {
            created_count: 0,
            refreshed_count: 0,
            removed_count: 0,
            pruned: prune,
        });
    }

    let progress: Arc<dyn g::GutchProgressSink> = CliProgressSink::new();
    let control = g::GutchOperationControl::default().with_progress(progress);
    let result = g::sync_snapshot_sources(
        repo,
        sources_path,
        g::SnapshotSourcesSyncOptions {
            prune_removed_snapshots: prune,
        },
        &control,
    )?;

    println!(
        "snapshot sources sync complete: created {}, refreshed {}, pruned {}{}",
        result.created_count,
        result.refreshed_count,
        result.removed_count,
        if prune {
            String::new()
        } else {
            " (--no-prune)".to_string()
        }
    );

    Ok(result)
}
