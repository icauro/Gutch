use std::path::Path;
use std::sync::Arc;

use crate::command_api::common::prompt_yes_no;
use crate::command_api::progress::CliProgressSink;
use gutch as g;

#[derive(Debug, Clone, Copy)]
pub(crate) struct CliSnapshotSourcesCaptureOptions {
    pub(crate) yes: bool,
}

fn print_snapshot_sources_capture_plan(plan: &g::SnapshotSourcesCapturePlan) {
    println!("snapshot sources capture plan:");
    println!("candidates: {}", plan.candidate_count);
    println!("new snapshots: {}", plan.capturable_count);
    println!("up to date: {}", plan.up_to_date_count());
    for line in g::describe_snapshot_sources_capture_plan(plan) {
        println!("  {line}");
    }
}

pub(crate) fn capture_snapshot_sources(
    repo: &Path,
    sources_path: &Path,
    options: CliSnapshotSourcesCaptureOptions,
    profile: &crate::command_api::Profile,
) -> g::Result<g::SnapshotSourcesCaptureResult> {
    let plan = g::plan_snapshot_sources_capture(repo, Some(sources_path))?;
    if !plan.sources_ready {
        return Err(g::Error::manifest(
            "snapshot sources path is missing or invalid",
        ));
    }

    if !plan.has_changes() {
        println!("snapshot sources capture: no changes");
        return Ok(g::SnapshotSourcesCaptureResult { captured_count: 0 });
    }

    print_snapshot_sources_capture_plan(&plan);

    if !options.yes && !prompt_yes_no("Proceed?", false)? {
        println!("snapshot sources capture cancelled");
        return Ok(g::SnapshotSourcesCaptureResult { captured_count: 0 });
    }

    let progress: Arc<dyn g::GutchProgressSink> = CliProgressSink::new();
    let control = crate::command_api::control_with_profile(profile).with_progress(progress);
    let result = g::capture_snapshot_sources(repo, sources_path, &control)?;

    println!(
        "snapshot sources capture complete: captured {}",
        result.captured_count,
    );

    Ok(result)
}
