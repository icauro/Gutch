use crate::cli::SnapshotSourcesCommands;
use crate::cli::reporting::ensure_parent_directory;

pub(crate) fn run(
    command: SnapshotSourcesCommands,
    profile: &crate::cli::Profile,
) -> crate::api::Result<()> {
    match command {
        SnapshotSourcesCommands::Capture {
            repo,
            source_root,
            yes,
            json_out,
        } => {
            let result = capture_snapshot_sources(
                &repo,
                &source_root,
                CliSnapshotSourcesCaptureOptions { yes },
                profile,
            )?;

            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                crate::filesystem::write(json_out, serde_json::to_vec_pretty(&result)?)?;
            }

            Ok(())
        }
    }
}

use std::path::Path;
use std::sync::Arc;

use crate::cli::command_support::prompt_yes_no;
use crate::cli::progress::CliProgressSink;

#[derive(Debug, Clone, Copy)]
pub(crate) struct CliSnapshotSourcesCaptureOptions {
    pub(crate) yes: bool,
}

fn print_snapshot_sources_capture_plan(plan: &crate::api::SnapshotSourcesCapturePlan) {
    println!("snapshot sources capture plan:");
    println!("candidates: {}", plan.candidate_count);
    println!("new snapshots: {}", plan.capturable_count);
    println!("up to date: {}", plan.up_to_date_count());
    for line in crate::api::describe_snapshot_sources_capture_plan(plan) {
        println!("  {line}");
    }
}

pub(crate) fn capture_snapshot_sources(
    repo: &Path,
    sources_path: &Path,
    options: CliSnapshotSourcesCaptureOptions,
    profile: &crate::cli::Profile,
) -> crate::api::Result<crate::api::SnapshotSourcesCaptureResult> {
    let plan = crate::api::plan_snapshot_sources_capture(repo, Some(sources_path))?;
    if !plan.sources_ready {
        return Err(crate::api::Error::manifest(
            "snapshot sources path is missing or invalid",
        ));
    }

    if !plan.has_changes() {
        println!("snapshot sources capture: no changes");
        return Ok(crate::api::SnapshotSourcesCaptureResult { captured_count: 0 });
    }

    print_snapshot_sources_capture_plan(&plan);

    if !options.yes && !prompt_yes_no("Proceed?", false)? {
        println!("snapshot sources capture cancelled");
        return Ok(crate::api::SnapshotSourcesCaptureResult { captured_count: 0 });
    }

    let progress: Arc<dyn crate::api::GutchProgressSink> = CliProgressSink::new();
    let control = crate::cli::control_with_profile(profile).with_progress(progress);
    let result = crate::api::capture_snapshot_sources(repo, sources_path, &control)?;

    println!(
        "snapshot sources capture complete: captured {}",
        result.captured_count,
    );

    Ok(result)
}
