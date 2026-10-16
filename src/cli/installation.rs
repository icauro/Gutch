use crate::cli::command_support::InstallationVerifyStats;
use crate::cli::command_support::SnapshotReconstructStats;
use crate::cli::command_support::{
    InstallationCorruptStats, InstallationRepairStats, write_report,
};
use crate::cli::progress::CliProgressSink;
use crate::cli::reporting::optional_output_paths;
use crate::cli::reporting::print_profile_summary;
use crate::cli::reporting::ratio_pct;
use crate::cli::reporting::reconstruct_timings_summary;
use crate::cli::reporting::verify_timings_summary;
use crate::cli::verbose_output_enabled;
use std::path::PathBuf;

pub(crate) struct ReconstructCommand {
    pub(crate) repo: std::path::PathBuf,
    pub(crate) snapshot_id: String,
    pub(crate) destination: std::path::PathBuf,
    pub(crate) staging_dir: Option<std::path::PathBuf>,
    pub(crate) source_snapshot_id: Option<String>,
    pub(crate) source_dir: Option<std::path::PathBuf>,
    pub(crate) runtime: crate::cli::arguments::RuntimeArgs,
    pub(crate) stats_json_out: Option<std::path::PathBuf>,
}

pub(crate) fn verify(
    repo: std::path::PathBuf,
    snapshot_id: String,
    destination: std::path::PathBuf,
    strict: bool,
    runtime: crate::cli::arguments::RuntimeArgs,
    stats_json_out: Option<std::path::PathBuf>,
    profile: &crate::cli::Profile,
) -> crate::api::Result<()> {
    crate::cli::installation::verify_snapshot(crate::cli::installation::VerifyCommand {
        repo,
        snapshot_id,
        destination,
        verify_mode: if strict {
            crate::api::VerifyMode::Strict
        } else {
            crate::api::VerifyMode::Fast
        },
        runtime_hints: runtime.hints(),
        stats_json_out,
        profile: profile.clone(),
    })
}

pub(crate) struct RepairCommand {
    pub(crate) repo: std::path::PathBuf,
    pub(crate) snapshot_id: String,
    pub(crate) destination: std::path::PathBuf,
    pub(crate) staging_dir: Option<std::path::PathBuf>,
    pub(crate) remove_added_files: bool,
    pub(crate) runtime: crate::cli::arguments::RuntimeArgs,
    pub(crate) stats_json_out: Option<std::path::PathBuf>,
    pub(crate) yes: bool,
}

pub(crate) fn repair(
    command: RepairCommand,
    profile: &crate::cli::Profile,
) -> crate::api::Result<()> {
    if crate::cli::output::dry_run() {
        return run_installation(
            crate::cli::InstallationCommands::Preview {
                repo: command.repo,
                snapshot_id: command.snapshot_id,
                destination: command.destination,
                staging_dir: command.staging_dir,
                publication: "selective".into(),
                remove_added_files: command.remove_added_files,
                metadata_only: false,
                source_snapshot: None,
                source_dir: None,
                runtime: command.runtime,
            },
            profile,
        );
    }
    if !command.yes
        && !crate::cli::command_support::prompt_yes_no(
            "Apply these changes to the installation?",
            false,
        )?
    {
        return Err(crate::api::Error::manifest("installation repair cancelled"));
    }
    let started_at = crate::profile::ProfileInstant::start(profile.as_deref());
    let execution = crate::api::repair_installation(crate::api::InstallationRepairRequest {
        repo_path: command.repo.clone(),
        snapshot_id: command.snapshot_id.clone(),
        destination_dir: command.destination.clone(),
        staging_dir: command.staging_dir,
        remove_added_files: command.remove_added_files,
        runtime_hints: command.runtime.hints(),
        control: crate::cli::control_with_profile(profile).with_progress(CliProgressSink::new()),
    })?;
    let result = execution.repair_result;
    let stats = InstallationRepairStats {
        command: "installation repair",
        repo_root: execution.repo_root.display().to_string(),
        snapshot_id: result.snapshot_id.clone(),
        destination_dir: result.destination_dir.display().to_string(),
        remove_added_files: command.remove_added_files,
        missing_file_count: result.missing_file_count,
        corrupt_file_count: result.corrupt_file_count,
        extra_file_count: result.extra_file_count,
        removed_extra_file_count: result.removed_extra_file_count,
        reconstructed_extra_file_count: result.reconstructed_extra_file_count,
        resolved_bundle_file_count: execution.resolved_bundle_file_count,
        elapsed_seconds: crate::cli::reporting::diagnostic_seconds(
            started_at.elapsed(),
            profile.as_deref(),
        ),
        outputs: crate::cli::reporting::optional_output_paths(&command.stats_json_out),
    };
    write_report(&command.stats_json_out, &stats, profile.as_deref())?;

    crate::cli::output::line(format_args!(
        "repaired installation of snapshot {} at {}",
        stats.snapshot_id, stats.destination_dir
    ));
    if verbose_output_enabled() {
        crate::cli::output::line(format_args!(
            "classified missing={}, corrupt={}, extra={}; removed extra={}, preserved extra={}",
            stats.missing_file_count,
            stats.corrupt_file_count,
            stats.extra_file_count,
            stats.removed_extra_file_count,
            stats.reconstructed_extra_file_count
        ));
    }
    if profile.is_some() {
        crate::cli::reporting::print_profile_summary(profile.as_deref());
    }
    Ok(())
}

pub(crate) struct CorruptCommand {
    pub(crate) destination: std::path::PathBuf,
    pub(crate) plan: std::path::PathBuf,
    pub(crate) stats_json_out: Option<std::path::PathBuf>,
    pub(crate) yes: bool,
}

pub(crate) fn corrupt(
    command: CorruptCommand,
    profile: &crate::cli::Profile,
) -> crate::api::Result<()> {
    let plan_bytes = std::fs::read(&command.plan).map_err(|error| {
        crate::api::Error::manifest(format!(
            "failed to read corruption plan '{}': {error}",
            command.plan.display()
        ))
    })?;
    let plan: crate::api::InstallationCorruptionPlan = serde_json::from_slice(&plan_bytes)
        .map_err(|error| {
            crate::api::Error::manifest(format!(
                "invalid corruption plan '{}': {error}",
                command.plan.display()
            ))
        })?;
    // Parse the plan first so the user never confirms a plan that cannot run.
    if !command.yes
        && !crate::cli::command_support::prompt_yes_no(
            "Apply these changes to the installation?",
            false,
        )?
    {
        return Err(crate::api::Error::manifest(
            "installation corruption cancelled",
        ));
    }
    let started_at = crate::profile::ProfileInstant::start(profile.as_deref());
    let result = crate::api::corrupt_installation(crate::api::InstallationCorruptionRequest {
        destination_dir: command.destination.clone(),
        plan,
        control: crate::cli::control_with_profile(profile).with_progress(CliProgressSink::new()),
    })?;
    let stats = InstallationCorruptStats {
        command: "debug corrupt-installation",
        destination_dir: result.destination_dir.display().to_string(),
        plan_path: command.plan.display().to_string(),
        applied_operation_count: result.applied_operation_count,
        elapsed_seconds: crate::cli::reporting::diagnostic_seconds(
            started_at.elapsed(),
            profile.as_deref(),
        ),
        outputs: crate::cli::reporting::optional_output_paths(&command.stats_json_out),
    };
    write_report(&command.stats_json_out, &stats, profile.as_deref())?;

    if verbose_output_enabled() {
        crate::cli::output::line(format_args!(
            "applied {} corruption operations to {}",
            stats.applied_operation_count, stats.destination_dir
        ));
    }
    if profile.is_some() {
        crate::cli::reporting::print_profile_summary(profile.as_deref());
    }
    Ok(())
}

pub(crate) fn run_installation(
    command: crate::cli::InstallationCommands,
    profile: &crate::cli::Profile,
) -> crate::api::Result<()> {
    match command {
        crate::cli::InstallationCommands::Reconstruct(arguments) => {
            let crate::cli::arguments::ReconstructionArgs {
                repo,
                snapshot_id,
                destination,
                staging_dir,
                source_snapshot_id,
                source_dir,
                runtime,
                fetch,
                metadata_url,
                product_metadata_url,
                blueprint_url,
                repo_metadata_url,
                chunk_index_url,
                bundle_downloads_endpoint,
                session_id,
                artifacts,
                artifact_url_manifest,
                transfer,
                stats_json_out,
            } = *arguments;
            if fetch {
                crate::cli::snapshot_transfer::fetch_and_reconstruct(
                    crate::cli::snapshot_transfer::FetchAndReconstructCommand {
                        fetch: crate::cli::snapshot_transfer::FetchCommand {
                            repo,
                            snapshot_id,
                            metadata_url: metadata_url
                                .expect("Clap requires snapshot metadata with --fetch"),
                            blueprint_url: blueprint_url
                                .expect("Clap requires a blueprint with --fetch"),
                            product_metadata_url,
                            repo_metadata_url,
                            chunk_index_url,
                            bundle_downloads_endpoint,
                            session_id,
                            artifacts,
                            artifact_url_manifest,
                            transfer_policy: transfer.policy(),
                        },
                        destination,
                        staging_dir,
                        source_snapshot_id,
                        source_dir,
                        runtime_hints: runtime.hints(),
                        stats_json_out,
                    },
                    profile,
                )
            } else {
                reconstruct(
                    ReconstructCommand {
                        repo,
                        snapshot_id,
                        destination,
                        staging_dir,
                        source_snapshot_id,
                        source_dir,
                        runtime,
                        stats_json_out,
                    },
                    profile,
                )
            }
        }
        crate::cli::InstallationCommands::Preview {
            repo,
            snapshot_id,
            destination,
            staging_dir,
            publication,
            remove_added_files,
            metadata_only,
            source_snapshot,
            source_dir,
            runtime,
        } => {
            let publication = if publication == "selective" {
                crate::api::InstallationPublication::Selective {
                    extra_files: if remove_added_files {
                        crate::api::ExtraFilePolicy::Remove
                    } else {
                        crate::api::ExtraFilePolicy::Preserve
                    },
                }
            } else {
                crate::api::InstallationPublication::ReplaceDirectory
            };
            let summary = crate::api::visit_installation_plan(
                &crate::api::InstallationRequest {
                    repo_path: repo,
                    snapshot_id,
                    destination_dir: destination,
                    staging_dir,
                    source_snapshot_id: source_snapshot,
                    source_dir,
                    runtime_hints: runtime.hints(),
                    control: crate::cli::control_with_profile(profile)
                        .with_progress(CliProgressSink::new()),
                },
                publication,
                if metadata_only {
                    crate::api::InstallationInspection::MetadataOnly
                } else {
                    crate::api::InstallationInspection::Content
                },
                &mut |event| crate::cli::output::event(&event),
            )?;
            crate::cli::output::record(&summary)?;
            crate::cli::output::line(format_args!(
                "{} entries; {} artifacts; {} bytes staging",
                summary.entry_count, summary.required_artifact_count, summary.staging_file_bytes
            ));
            Ok(())
        }

        crate::cli::InstallationCommands::Verify {
            repo,
            snapshot_id,
            destination,
            strict,
            runtime,
            stats_json_out,
        } => verify(
            repo,
            snapshot_id,
            destination,
            strict,
            runtime,
            stats_json_out,
            profile,
        ),
        crate::cli::InstallationCommands::Repair {
            repo,
            snapshot_id,
            destination,
            staging_dir,
            remove_added_files,
            runtime,
            stats_json_out,
            yes,
        } => repair(
            RepairCommand {
                repo,
                snapshot_id,
                destination,
                staging_dir,
                remove_added_files,
                runtime,
                stats_json_out,
                yes,
            },
            profile,
        ),
    }
}

pub(crate) struct VerifyCommand {
    pub(crate) repo: PathBuf,
    pub(crate) snapshot_id: String,
    pub(crate) destination: PathBuf,
    pub(crate) verify_mode: crate::api::VerifyMode,
    pub(crate) runtime_hints: crate::api::RuntimeResourceHints,
    pub(crate) stats_json_out: Option<PathBuf>,
    pub(crate) profile: crate::cli::Profile,
}

pub(crate) fn reconstruct(
    command: ReconstructCommand,
    profile: &crate::cli::Profile,
) -> crate::api::Result<()> {
    if crate::cli::output::dry_run() {
        return run_installation(
            crate::cli::InstallationCommands::Preview {
                repo: command.repo,
                snapshot_id: command.snapshot_id,
                destination: command.destination,
                staging_dir: command.staging_dir,
                publication: "replace-directory".into(),
                remove_added_files: true,
                metadata_only: false,
                source_snapshot: command.source_snapshot_id,
                source_dir: command.source_dir,
                runtime: command.runtime,
            },
            profile,
        );
    }
    let execution = crate::api::reconstruct_snapshot(crate::api::InstallationRequest {
        repo_path: command.repo.clone(),
        snapshot_id: command.snapshot_id.clone(),
        destination_dir: command.destination.clone(),
        staging_dir: command.staging_dir.clone(),
        source_snapshot_id: command.source_snapshot_id.clone(),
        source_dir: command.source_dir.clone(),
        runtime_hints: command.runtime.hints(),
        control: crate::cli::control_with_profile(profile).with_progress(CliProgressSink::new()),
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
            crate::cli::reporting::profile_mode(profile.as_deref()),
        ),
        verify_timings: verify_timings_summary(
            &result.verify_timings,
            crate::cli::reporting::profile_mode(profile.as_deref()),
        ),
        outputs: optional_output_paths(&command.stats_json_out),
    };
    write_report(&command.stats_json_out, &stats, profile.as_deref())?;

    crate::cli::output::line(format_args!(
        "reconstructed snapshot {} to {}",
        stats.snapshot_id, stats.destination_dir
    ));
    if verbose_output_enabled() {
        crate::cli::output::line(format_args!(
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

pub(crate) fn verify_snapshot(command: VerifyCommand) -> crate::api::Result<()> {
    let progress = CliProgressSink::new();
    let execution = crate::api::verify_installation(crate::api::InstallationVerifyRequest {
        repo_path: command.repo.clone(),
        snapshot_id: command.snapshot_id.clone(),
        destination_dir: command.destination.clone(),
        verify_mode: command.verify_mode,
        runtime_hints: command.runtime_hints,
        control: crate::cli::control_with_profile(&command.profile).with_progress(progress),
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
        timings: verify_timings_summary(
            &timings,
            crate::cli::reporting::profile_mode(command.profile.as_deref()),
        ),
        outputs: optional_output_paths(&command.stats_json_out),
    };
    write_report(&command.stats_json_out, &stats, command.profile.as_deref())?;

    if errors.is_empty() {
        crate::cli::output::line(format_args!(
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
        return Err(crate::api::Error::verify(errors.join("; ")));
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
            stats_json_out: None,
            yes: false,
        };

        // Without `--yes` a prompt would fail on test stdin, so a plan error proves ordering.
        let error = super::corrupt(command, &None).unwrap_err().to_string();

        assert!(error.contains("failed to read corruption plan"), "{error}");
    }
}
