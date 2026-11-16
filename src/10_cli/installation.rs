//! `installation` command handlers.

use crate::cli::InstallationCommands;
use crate::cli::Profile;
use crate::cli::arguments::FetchArgs;
use crate::cli::arguments::PreviewArgs;
use crate::cli::arguments::PreviewPublication;
use crate::cli::arguments::ReconstructArgs;
use crate::cli::arguments::RuntimeArgs;
use crate::cli::command_support::InstallationVerifyStats;
use crate::cli::command_support::SnapshotReconstructStats;
use crate::cli::command_support::prompt_yes_no;
use crate::cli::command_support::{InstallationCorruptStats, InstallationRepairStats};
use crate::cli::control_with_profile;
use crate::cli::output;
use crate::cli::output::dry_run;
use crate::cli::progress::CliProgressSink;
use crate::cli::reporting::diagnostic_seconds;
use crate::cli::reporting::print_profile_summary;
use crate::cli::reporting::profile_mode;
use crate::cli::reporting::ratio_pct;
use crate::cli::reporting::reconstruct_timings_summary;
use crate::cli::reporting::verify_timings_summary;
use crate::cli::reporting::write_stats_json_output;
use crate::cli::snapshot_transfer::FetchAndReconstructCommand;
use crate::cli::snapshot_transfer::fetch_and_reconstruct;
use crate::cli::verbose_output_enabled;
use crate::installation::InstallationCorruptionPlan;
use crate::installation::InstallationCorruptionRequest;
use crate::installation::InstallationRepairRequest;
use crate::installation::InstallationRequest;
use crate::installation::InstallationVerifyRequest;
use crate::installation::corrupt_installation;
use crate::installation::inspect::VerifyMode;
use crate::installation::install::ExtraFilePolicy;
use crate::installation::install::InstallationPublication;
use crate::installation::preview::InstallationInspection;
use crate::installation::preview::visit_installation_plan;
use crate::installation::reconstruct_snapshot;
use crate::installation::repair_installation;
use crate::installation::verify_installation;
use crate::profiling::ProfileInstant;
use crate::runtime::RuntimeResourceHints;
use std::fs;
use std::path::PathBuf;

pub(crate) struct ReconstructCommand {
    pub(crate) repo: PathBuf,
    pub(crate) snapshot_id: String,
    pub(crate) destination: PathBuf,
    pub(crate) staging_dir: Option<PathBuf>,
    pub(crate) source_snapshot_id: Option<String>,
    pub(crate) source_dir: Option<PathBuf>,
    pub(crate) runtime: RuntimeArgs,
}

pub(crate) fn verify(
    repo: PathBuf,
    snapshot_id: String,
    destination: PathBuf,
    strict: bool,
    runtime: RuntimeArgs,
    profile: &Profile,
) -> crate::Result<()> {
    crate::cli::installation::verify_snapshot(crate::cli::installation::VerifyCommand {
        repo,
        snapshot_id,
        destination,
        verify_mode: if strict {
            VerifyMode::Strict
        } else {
            VerifyMode::Fast
        },
        runtime_hints: runtime.hints(),
        profile: profile.clone(),
    })
}

pub(crate) struct RepairCommand {
    pub(crate) repo: PathBuf,
    pub(crate) snapshot_id: String,
    pub(crate) destination: PathBuf,
    pub(crate) staging_dir: Option<PathBuf>,
    pub(crate) remove_extra_files: bool,
    pub(crate) runtime: RuntimeArgs,
    pub(crate) yes: bool,
}

pub(crate) fn repair(command: RepairCommand, profile: &Profile) -> crate::Result<()> {
    if dry_run() {
        return preview(
            PreviewArgs {
                repo: command.repo,
                snapshot_id: command.snapshot_id,
                destination: command.destination,
                staging_dir: command.staging_dir,
                publication: PreviewPublication::Selective,
                remove_extra_files: command.remove_extra_files,
                metadata_only: false,
                source_snapshot: None,
                source_dir: None,
                runtime: command.runtime,
            },
            profile,
        );
    }
    if !command.yes && !prompt_yes_no("Apply these changes to the installation?", false)? {
        return Err(crate::Error::Cancelled);
    }
    let started_at = ProfileInstant::start(profile.as_deref());
    let execution = repair_installation(InstallationRepairRequest {
        repo_path: command.repo.clone(),
        snapshot_id: command.snapshot_id.clone(),
        destination_dir: command.destination.clone(),
        staging_dir: command.staging_dir,
        remove_extra_files: command.remove_extra_files,
        runtime_hints: command.runtime.hints(),
        control: control_with_profile(profile).with_progress(CliProgressSink::new()),
    })?;
    let result = execution.repair_result;
    let stats = InstallationRepairStats {
        command: "installation repair",
        repo_root: execution.repo_root.display().to_string(),
        snapshot_id: result.snapshot_id.clone(),
        destination_dir: result.destination_dir.display().to_string(),
        remove_extra_files: command.remove_extra_files,
        missing_file_count: result.missing_file_count,
        corrupt_file_count: result.corrupt_file_count,
        extra_file_count: result.extra_file_count,
        removed_extra_file_count: result.removed_extra_file_count,
        resolved_bundle_file_count: execution.resolved_bundle_file_count,
        elapsed_seconds: diagnostic_seconds(started_at.elapsed(), profile.as_deref()),
    };
    write_stats_json_output(&stats, profile.as_deref())?;

    output::line(format_args!(
        "repaired installation of snapshot {} at {}",
        stats.snapshot_id, stats.destination_dir
    ));
    if verbose_output_enabled() {
        output::line(format_args!(
            "classified missing={}, corrupt={}, extra={}; removed extra={}",
            stats.missing_file_count,
            stats.corrupt_file_count,
            stats.extra_file_count,
            stats.removed_extra_file_count
        ));
    }
    if profile.is_some() {
        print_profile_summary(profile.as_deref());
    }
    Ok(())
}

pub(crate) struct CorruptCommand {
    pub(crate) destination: PathBuf,
    pub(crate) plan: PathBuf,
    pub(crate) yes: bool,
}

pub(crate) fn corrupt(command: CorruptCommand, profile: &Profile) -> crate::Result<()> {
    let plan_bytes = fs::read(&command.plan).map_err(|error| {
        crate::Error::invalid_input(format!(
            "failed to read corruption plan '{}': {error}",
            command.plan.display()
        ))
    })?;
    let plan: InstallationCorruptionPlan =
        serde_json::from_slice(&plan_bytes).map_err(|error| {
            crate::Error::invalid_input(format!(
                "invalid corruption plan '{}': {error}",
                command.plan.display()
            ))
        })?;
    // Parse the plan first so the user never confirms a plan that cannot run.
    if !command.yes && !prompt_yes_no("Apply these changes to the installation?", false)? {
        return Err(crate::Error::Cancelled);
    }
    let started_at = ProfileInstant::start(profile.as_deref());
    let result = corrupt_installation(InstallationCorruptionRequest {
        destination_dir: command.destination.clone(),
        plan,
        control: control_with_profile(profile).with_progress(CliProgressSink::new()),
    })?;
    let stats = InstallationCorruptStats {
        command: "debug corrupt-installation",
        destination_dir: result.destination_dir.display().to_string(),
        plan_path: command.plan.display().to_string(),
        applied_operation_count: result.applied_operation_count,
        elapsed_seconds: diagnostic_seconds(started_at.elapsed(), profile.as_deref()),
    };
    write_stats_json_output(&stats, profile.as_deref())?;

    if verbose_output_enabled() {
        output::line(format_args!(
            "applied {} corruption operations to {}",
            stats.applied_operation_count, stats.destination_dir
        ));
    }
    if profile.is_some() {
        print_profile_summary(profile.as_deref());
    }
    Ok(())
}

pub(crate) fn run_installation(
    command: InstallationCommands,
    profile: &Profile,
) -> crate::Result<()> {
    match command {
        InstallationCommands::Reconstruct(arguments) => reconstruct_from_args(*arguments, profile),
        InstallationCommands::Preview(arguments) => preview(arguments, profile),
        InstallationCommands::Verify {
            repo,
            snapshot_id,
            destination,
            strict,
            runtime,
        } => verify(repo, snapshot_id, destination, strict, runtime, profile),
        InstallationCommands::Repair {
            repo,
            snapshot_id,
            destination,
            staging_dir,
            remove_extra_files,
            runtime,
            yes,
        } => repair(
            RepairCommand {
                repo,
                snapshot_id,
                destination,
                staging_dir,
                remove_extra_files,
                runtime,
                yes,
            },
            profile,
        ),
    }
}

/// Reconstructs from the local repository, or fetches first with `--fetch`.
fn reconstruct_from_args(arguments: ReconstructArgs, profile: &Profile) -> crate::Result<()> {
    let ReconstructArgs {
        repo,
        snapshot_id,
        destination,
        staging_dir,
        source_snapshot_id,
        source_dir,
        runtime,
        fetch,
        metadata_url,
        blueprint_url,
        sources,
        transfer,
    } = arguments;
    if !fetch {
        return reconstruct(
            ReconstructCommand {
                repo,
                snapshot_id,
                destination,
                staging_dir,
                source_snapshot_id,
                source_dir,
                runtime,
            },
            profile,
        );
    }
    fetch_and_reconstruct(
        FetchAndReconstructCommand {
            fetch: FetchArgs {
                repo,
                snapshot_id,
                metadata_url: metadata_url.expect("Clap requires snapshot metadata with --fetch"),
                blueprint_url: blueprint_url.expect("Clap requires a blueprint with --fetch"),
                sources,
                transfer,
            },
            destination,
            staging_dir,
            source_snapshot_id,
            source_dir,
            runtime_hints: runtime.hints(),
        },
        profile,
    )
}

/// Prints each planned installation change and a summary line.
fn preview(arguments: PreviewArgs, profile: &Profile) -> crate::Result<()> {
    let publication = if arguments.publication == PreviewPublication::Selective {
        InstallationPublication::Selective {
            extra_files: if arguments.remove_extra_files {
                ExtraFilePolicy::Remove
            } else {
                ExtraFilePolicy::Preserve
            },
        }
    } else {
        InstallationPublication::ReplaceDirectory
    };
    let inspection = if arguments.metadata_only {
        InstallationInspection::MetadataOnly
    } else {
        InstallationInspection::Content
    };
    let request = InstallationRequest {
        repo_path: arguments.repo,
        snapshot_id: arguments.snapshot_id,
        destination_dir: arguments.destination,
        staging_dir: arguments.staging_dir,
        source_snapshot_id: arguments.source_snapshot,
        source_dir: arguments.source_dir,
        runtime_hints: arguments.runtime.hints(),
        control: control_with_profile(profile).with_progress(CliProgressSink::new()),
    };
    let summary = visit_installation_plan(&request, publication, inspection, &mut |event| {
        output::event(&event)
    })?;
    output::record(&summary)?;
    output::line(format_args!(
        "{} entries; {} artifacts; {} bytes staging",
        summary.entry_count, summary.required_artifact_count, summary.staging_file_bytes
    ));
    Ok(())
}

pub(crate) struct VerifyCommand {
    pub(crate) repo: PathBuf,
    pub(crate) snapshot_id: String,
    pub(crate) destination: PathBuf,
    pub(crate) verify_mode: VerifyMode,
    pub(crate) runtime_hints: RuntimeResourceHints,
    pub(crate) profile: Profile,
}

pub(crate) fn reconstruct(command: ReconstructCommand, profile: &Profile) -> crate::Result<()> {
    if dry_run() {
        return preview(
            PreviewArgs {
                repo: command.repo,
                snapshot_id: command.snapshot_id,
                destination: command.destination,
                staging_dir: command.staging_dir,
                publication: PreviewPublication::ReplaceDirectory,
                remove_extra_files: true,
                metadata_only: false,
                source_snapshot: command.source_snapshot_id,
                source_dir: command.source_dir,
                runtime: command.runtime,
            },
            profile,
        );
    }
    let execution = reconstruct_snapshot(InstallationRequest {
        repo_path: command.repo.clone(),
        snapshot_id: command.snapshot_id.clone(),
        destination_dir: command.destination.clone(),
        staging_dir: command.staging_dir.clone(),
        source_snapshot_id: command.source_snapshot_id.clone(),
        source_dir: command.source_dir.clone(),
        runtime_hints: command.runtime.hints(),
        control: control_with_profile(profile).with_progress(CliProgressSink::new()),
    })?;
    let snapshot_manifest = execution.snapshot_manifest;
    let result = execution.reconstruct_result;
    let reused_source_chunk_percent = ratio_pct(
        result.reused_source_chunk_count as u64,
        result.required_chunk_count as u64,
    );
    let stats = SnapshotReconstructStats {
        command: "installation reconstruct",
        repo_root: execution.repo_root.display().to_string(),
        snapshot_id: result.snapshot_id.clone(),
        destination_dir: result.destination_dir.display().to_string(),
        staging_dir: result.staging_dir.display().to_string(),
        source_snapshot_id: command.source_snapshot_id.clone(),
        source_destination_dir: execution
            .source_destination_dir
            .as_ref()
            .map(|path| path.display().to_string()),
        file_count: snapshot_manifest.file_count(),
        required_chunk_count: result.required_chunk_count,
        required_bundle_count: result.required_bundle_count,
        required_artifact_count: result.required_artifact_count,
        resolved_bundle_file_count: execution.resolved_bundle_file_count,
        resolved_artifact_file_count: execution.resolved_artifact_file_count,
        reused_source_chunk_count: result.reused_source_chunk_count,
        reused_source_chunk_percent,
        total_logical_size: snapshot_manifest.total_logical_size,
        total_compressed_size_estimate: snapshot_manifest.total_compressed_size_estimate,
        reconstruct_timings: reconstruct_timings_summary(
            &result.reconstruct_timings,
            profile_mode(profile.as_deref()),
        ),
        verify_timings: verify_timings_summary(
            &result.verify_timings,
            profile_mode(profile.as_deref()),
        ),
    };
    write_stats_json_output(&stats, profile.as_deref())?;

    output::line(format_args!(
        "reconstructed snapshot {} to {}",
        stats.snapshot_id, stats.destination_dir
    ));
    if verbose_output_enabled() {
        output::line(format_args!(
            "reused {} of {} chunks from source ({:.2}%)",
            stats.reused_source_chunk_count,
            stats.required_chunk_count,
            stats.reused_source_chunk_percent
        ));
    }
    if profile.is_some() {
        print_profile_summary(profile.as_deref());
    }
    Ok(())
}

pub(crate) fn verify_snapshot(command: VerifyCommand) -> crate::Result<()> {
    let progress = CliProgressSink::new();
    let execution = verify_installation(InstallationVerifyRequest {
        repo_path: command.repo.clone(),
        snapshot_id: command.snapshot_id.clone(),
        destination_dir: command.destination.clone(),
        verify_mode: command.verify_mode,
        runtime_hints: command.runtime_hints,
        control: control_with_profile(&command.profile).with_progress(progress),
    })?;
    let repo_root = execution.repo_root;
    let snapshot_manifest = execution.snapshot_manifest;
    let classification = execution.classification;
    let timings = execution.verify_timings;
    let missing_file_count = classification.missing_files.len();
    let corrupt_file_count = classification.corrupt_files.len();
    let extra_file_count = classification.extra_files.len();
    let errors = classification.into_messages();
    let stats = InstallationVerifyStats {
        command: "installation verify",
        repo_root: repo_root.display().to_string(),
        snapshot_id: snapshot_manifest.snapshot_id.clone(),
        destination_dir: command.destination.display().to_string(),
        verify_mode: command.verify_mode.as_str().to_string(),
        expected_file_count: snapshot_manifest.file_count(),
        missing_file_count,
        corrupt_file_count,
        extra_file_count,
        verification_passed: errors.is_empty(),
        errors: errors.clone(),
        timings: verify_timings_summary(&timings, profile_mode(command.profile.as_deref())),
    };
    write_stats_json_output(&stats, command.profile.as_deref())?;

    if errors.is_empty() {
        output::line(format_args!(
            "verification passed for {}",
            command.destination.display()
        ));
        if command.profile.is_some() {
            print_profile_summary(command.profile.as_deref());
        }
    } else {
        eprintln!("{} verification errors:", errors.len());
        for err in &errors {
            eprintln!("  - {}", err);
        }
        // The list is printed above and recorded in the report; keep the error short.
        return Err(crate::Error::verify(format!(
            "{} verification errors in {}",
            errors.len(),
            command.destination.display()
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn corruption_rejects_an_unreadable_plan_before_asking_for_confirmation() {
        let temp = tempfile::tempdir().unwrap();
        let command = super::CorruptCommand {
            destination: temp.path().join("installation"),
            plan: temp.path().join("missing-plan.json"),
            yes: false,
        };

        // Without `--yes` a prompt would fail on test stdin, so a plan error proves ordering.
        let error = super::corrupt(command, &None).unwrap_err().to_string();

        assert!(error.contains("failed to read corruption plan"), "{error}");
    }
}
