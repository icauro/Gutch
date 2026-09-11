use crate::cli::common::ReconstructedCorruptStats;
use crate::cli::common::ReconstructedRepairStats;
use crate::cli::common::SnapshotFetchReconstructStats;
use crate::cli::common::parse_bundle_downloads;
use crate::cli::common::write_report;
use crate::cli::progress::CliProgressSink;
use crate::cli::reporting::print_transfer_timings;
use crate::cli::verbose_output_enabled;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

const MAX_BUNDLE_URL_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) struct FetchCommand {
    pub(crate) repo: std::path::PathBuf,
    pub(crate) snapshot_id: String,
    pub(crate) metadata_url: String,
    pub(crate) product_metadata_url: Option<String>,
    pub(crate) blueprint_url: String,
    pub(crate) repo_metadata_url: Option<String>,
    pub(crate) chunk_index_url: Option<String>,
    pub(crate) bundle_downloads_endpoint: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) bundles: Vec<String>,
    pub(crate) artifact_url_manifest: Option<std::path::PathBuf>,
    pub(crate) transfer_policy: crate::api::TransferConcurrencyPolicy,
}

impl FetchCommand {
    fn into_request(
        self,
        control: crate::api::GutchOperationControl,
    ) -> crate::api::Result<crate::api::SnapshotFetchRequest> {
        let FetchCommand {
            repo,
            snapshot_id,
            metadata_url,
            product_metadata_url,
            blueprint_url,
            repo_metadata_url,
            chunk_index_url,
            bundle_downloads_endpoint,
            session_id,
            bundles,
            artifact_url_manifest,
            transfer_policy,
        } = self;
        let bundle_url_provider = bundle_downloads_endpoint
            .map(|endpoint| {
                let client = reqwest::blocking::Client::builder()
                    .user_agent("Gutch CLI")
                    .connect_timeout(Duration::from_secs(10))
                    .timeout(Duration::from_secs(30 * 60))
                    .build()
                    .map_err(|err| {
                        crate::api::Error::manifest(format!(
                            "failed to create download client: {err}"
                        ))
                    })?;
                let session_id = session_id.clone();
                Ok::<Arc<dyn crate::api::BundleUrlProvider>, crate::api::Error>(Arc::new(
                    move |bundle_ids: &[String], control: &crate::api::GutchOperationControl| {
                        resolve_bundle_urls(
                            &client,
                            &endpoint,
                            session_id.as_deref(),
                            bundle_ids,
                            control,
                        )
                    },
                ))
            })
            .transpose()?;
        let mut direct_downloads = parse_bundle_downloads(&bundles)?;
        if let Some(manifest_path) = artifact_url_manifest {
            direct_downloads.extend(parse_artifact_url_manifest(&manifest_path)?);
        }
        Ok(crate::api::SnapshotFetchRequest {
            repo_path: repo,
            snapshot_id,
            repo_metadata_url,
            metadata_url,
            product_metadata_url,
            blueprint_url,
            chunk_index_url,
            bundles: direct_downloads,
            bundle_url_provider,
            control,
            transfer_policy,
        })
    }
}

pub(crate) fn fetch(
    command: FetchCommand,
    profile: &crate::cli::Profile,
) -> crate::api::Result<()> {
    let progress = CliProgressSink::new();
    let result = crate::api::fetch_snapshot(
        &command.into_request(crate::cli::control_with_profile(profile).with_progress(progress))?,
    )?;

    if verbose_output_enabled() {
        println!(
            "fetched snapshot data for {} into {}",
            result.snapshot_id,
            result.repo_path.display()
        );
        println!(
            "bundles: required={}, downloaded={}, existing={}",
            result.required_bundle_count,
            result.downloaded_bundle_count,
            result.skipped_existing_bundle_count
        );
        println!(
            "artifacts: required={}, downloaded={}, existing={}",
            result.required_artifact_count,
            result.downloaded_artifact_count,
            result.skipped_existing_artifact_count
        );
        print_transfer_timings(&result.transfer_stats);
    }
    if profile.is_some() {
        crate::cli::reporting::print_profile_summary(profile.as_deref());
    }
    Ok(())
}

pub(crate) struct FetchAndReconstructCommand {
    pub(crate) fetch: FetchCommand,
    pub(crate) destination: std::path::PathBuf,
    pub(crate) staging_dir: Option<std::path::PathBuf>,
    pub(crate) source_snapshot_id: Option<String>,
    pub(crate) source_dir: Option<std::path::PathBuf>,
    pub(crate) runtime_hints: crate::api::RuntimeResourceHints,
    pub(crate) stats_json_out: Option<std::path::PathBuf>,
}

pub(crate) fn fetch_and_reconstruct(
    command: FetchAndReconstructCommand,
    profile: &crate::cli::Profile,
) -> crate::api::Result<()> {
    let FetchAndReconstructCommand {
        fetch,
        destination,
        staging_dir,
        source_snapshot_id,
        source_dir,
        runtime_hints,
        stats_json_out,
    } = command;
    let progress = CliProgressSink::new();
    let fetch_request =
        fetch.into_request(crate::cli::control_with_profile(profile).with_progress(progress))?;
    let result = crate::api::download_and_reconstruct_snapshot(
        crate::api::SnapshotDownloadAndReconstructRequest {
            fetch_request,
            destination_dir: destination,
            staging_dir,
            source_snapshot_id: source_snapshot_id.clone(),
            source_dir: source_dir.clone(),
            runtime_hints,
        },
    )?;
    let crate::api::SnapshotDownloadAndReconstructResult {
        fetch_result,
        reconstruct_result: reconstruct_execution,
    } = result;
    let snapshot_manifest = reconstruct_execution.snapshot_manifest;
    let reconstruct = reconstruct_execution.reconstruct_result;
    let reused_source_chunk_percent = crate::cli::reporting::ratio_pct(
        reconstruct.reused_source_chunk_count as u64,
        reconstruct.required_chunk_count as u64,
    );
    let stats = SnapshotFetchReconstructStats {
        command: "snapshot fetch-reconstruct",
        repo_root: fetch_result.repo_path.display().to_string(),
        snapshot_id: fetch_result.snapshot_id.clone(),
        destination_dir: reconstruct.destination_dir.display().to_string(),
        staging_dir: reconstruct.staging_dir.display().to_string(),
        source_snapshot_id,
        source_destination_dir: reconstruct_execution
            .source_destination_dir
            .map(|path| path.display().to_string()),
        file_count: snapshot_manifest.file_count(),
        required_chunk_count: reconstruct.required_chunk_count,
        required_bundle_count: fetch_result.required_bundle_count,
        downloaded_bundle_count: fetch_result.downloaded_bundle_count,
        skipped_existing_bundle_count: fetch_result.skipped_existing_bundle_count,
        required_artifact_count: fetch_result.required_artifact_count,
        downloaded_artifact_count: fetch_result.downloaded_artifact_count,
        skipped_existing_artifact_count: fetch_result.skipped_existing_artifact_count,
        transfer_policy: fetch_result.transfer_policy.name(),
        initial_transfer_concurrency: fetch_result.transfer_stats.initial_concurrency,
        maximum_transfer_concurrency: fetch_result.transfer_stats.maximum_concurrency,
        peak_transfer_concurrency: fetch_result.transfer_stats.peak_concurrency,
        transfer_adjustment_count: fetch_result.transfer_stats.adjustment_count,
        transfer_retry_count: fetch_result.transfer_stats.retry_count,
        transfer_bytes: fetch_result.transfer_stats.transferred_bytes,
        transfer_total_bytes: fetch_result.transfer_stats.total_bytes,
        transfer_seconds: fetch_result.transfer_stats.elapsed_seconds,
        transfer_mib_per_second: fetch_result.transfer_stats.mib_per_second,
        reused_source_chunk_count: reconstruct.reused_source_chunk_count,
        reused_source_chunk_percent,
        total_logical_size: snapshot_manifest.total_logical_size,
        total_compressed_size_estimate: snapshot_manifest.total_compressed_size_estimate,
        reconstruct_timings: crate::cli::reporting::reconstruct_timings_summary(
            &reconstruct.reconstruct_timings,
        ),
        verify_timings: crate::cli::reporting::verify_timings_summary(&reconstruct.verify_timings),
        outputs: crate::cli::reporting::optional_output_paths(&stats_json_out),
    };
    write_report(&stats_json_out, &stats, profile.as_deref())?;

    if verbose_output_enabled() {
        println!(
            "downloaded and reconstructed snapshot {} to {}",
            stats.snapshot_id, stats.destination_dir
        );
        println!(
            "artifacts: required={}, downloaded={}, existing={}",
            stats.required_artifact_count,
            stats.downloaded_artifact_count,
            stats.skipped_existing_artifact_count
        );
        print_transfer_timings(&fetch_result.transfer_stats);
    }
    if profile.is_some() {
        crate::cli::reporting::print_profile_summary(profile.as_deref());
    }
    Ok(())
}

pub(crate) struct ReconstructCommand {
    pub(crate) repo: std::path::PathBuf,
    pub(crate) snapshot_id: String,
    pub(crate) destination: std::path::PathBuf,
    pub(crate) staging_dir: Option<std::path::PathBuf>,
    pub(crate) source_snapshot_id: Option<String>,
    pub(crate) source_dir: Option<std::path::PathBuf>,
    pub(crate) runtime: crate::cli::args::RuntimeArgs,
    pub(crate) stats_json_out: Option<std::path::PathBuf>,
}

fn resolve_bundle_urls(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    session_id: Option<&str>,
    bundle_ids: &[String],
    control: &crate::api::GutchOperationControl,
) -> crate::api::Result<Vec<crate::api::BundleDownload>> {
    control.checkpoint()?;
    let separator = if endpoint.contains('?') { "&" } else { "?" };
    let request_url = format!(
        "{endpoint}{separator}ids={}",
        urlencoding::encode(&bundle_ids.join(","))
    );
    let mut request = client.get(request_url);
    if let Some(session_id) = session_id {
        request = request.bearer_auth(session_id);
    }
    let response = request
        .send()
        .map_err(|err| crate::api::Error::manifest(format!("bundle URL request failed: {err}")))?;
    if !response.status().is_success() {
        return Err(crate::api::Error::manifest(format!(
            "bundle URL request failed with status {}",
            response.status()
        )));
    }
    let bytes = crate::api::read_bounded_response(response, MAX_BUNDLE_URL_RESPONSE_BYTES, || {
        control.checkpoint()
    })?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct BundleUrlsResponse {
        #[serde(default)]
        bundles: Vec<crate::api::BundleDownload>,
    }
    Ok(serde_json::from_slice::<BundleUrlsResponse>(&bytes)?.bundles)
}

fn parse_artifact_url_manifest(
    path: &std::path::Path,
) -> crate::api::Result<Vec<crate::api::BundleDownload>> {
    let bytes = std::fs::read(path)?;
    let urls = serde_json::from_slice::<std::collections::BTreeMap<String, String>>(&bytes)
        .map_err(|error| {
            crate::api::Error::manifest(format!(
                "invalid artifact URL manifest '{}': {error}",
                path.display()
            ))
        })?;
    urls.into_iter()
        .map(|(bundle_id, url)| {
            if bundle_id.trim().is_empty() || url.trim().is_empty() {
                return Err(crate::api::Error::manifest(format!(
                    "artifact URL manifest '{}' contains an empty artifact ID or URL",
                    path.display()
                )));
            }
            Ok(crate::api::BundleDownload { bundle_id, url })
        })
        .collect()
}

pub(crate) fn verify(
    repo: std::path::PathBuf,
    snapshot_id: String,
    destination: std::path::PathBuf,
    strict: bool,
    runtime: crate::cli::args::RuntimeArgs,
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
    pub(crate) runtime: crate::cli::args::RuntimeArgs,
    pub(crate) stats_json_out: Option<std::path::PathBuf>,
    pub(crate) yes: bool,
}

pub(crate) fn repair(
    command: RepairCommand,
    profile: &crate::cli::Profile,
) -> crate::api::Result<()> {
    if !command.yes {
        return Err(crate::api::Error::manifest(
            "reconstructed repair requires --yes flag",
        ));
    }
    let started_at = crate::profile::ProfileInstant::start(profile.as_deref());
    let execution =
        crate::api::repair_snapshot_from_repo(crate::api::SnapshotRepairFromRepoRequest {
            repo_path: command.repo.clone(),
            snapshot_id: command.snapshot_id.clone(),
            destination_dir: command.destination.clone(),
            staging_dir: command.staging_dir,
            remove_added_files: command.remove_added_files,
            runtime_hints: command.runtime.hints(),
            control: crate::cli::control_with_profile(profile)
                .with_progress(CliProgressSink::new()),
        })?;
    let result = execution.repair_result;
    let stats = ReconstructedRepairStats {
        command: "reconstructed repair",
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
        elapsed_seconds: started_at.elapsed().as_secs_f64(),
        outputs: crate::cli::reporting::optional_output_paths(&command.stats_json_out),
    };
    write_report(&command.stats_json_out, &stats, profile.as_deref())?;

    if verbose_output_enabled() {
        println!(
            "repaired reconstructed snapshot {} at {}",
            stats.snapshot_id, stats.destination_dir
        );
        println!(
            "classified missing={}, corrupt={}, extra={}; removed extra={}, preserved extra={}",
            stats.missing_file_count,
            stats.corrupt_file_count,
            stats.extra_file_count,
            stats.removed_extra_file_count,
            stats.reconstructed_extra_file_count
        );
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
    if !command.yes {
        return Err(crate::api::Error::manifest(
            "reconstructed corrupt requires --yes flag",
        ));
    }
    let plan_bytes = std::fs::read(&command.plan).map_err(|error| {
        crate::api::Error::manifest(format!(
            "failed to read corruption plan '{}': {error}",
            command.plan.display()
        ))
    })?;
    let plan: crate::api::ReconstructedCorruptionPlan = serde_json::from_slice(&plan_bytes)
        .map_err(|error| {
            crate::api::Error::manifest(format!(
                "invalid corruption plan '{}': {error}",
                command.plan.display()
            ))
        })?;
    let started_at = crate::profile::ProfileInstant::start(profile.as_deref());
    let result =
        crate::api::corrupt_reconstructed_snapshot(crate::api::ReconstructedCorruptionRequest {
            destination_dir: command.destination.clone(),
            plan,
            control: crate::cli::control_with_profile(profile)
                .with_progress(CliProgressSink::new()),
        })?;
    let stats = ReconstructedCorruptStats {
        command: "reconstructed corrupt",
        destination_dir: result.destination_dir.display().to_string(),
        plan_path: command.plan.display().to_string(),
        applied_operation_count: result.applied_operation_count,
        elapsed_seconds: started_at.elapsed().as_secs_f64(),
        outputs: crate::cli::reporting::optional_output_paths(&command.stats_json_out),
    };
    write_report(&command.stats_json_out, &stats, profile.as_deref())?;

    if verbose_output_enabled() {
        println!(
            "applied {} corruption operations to {}",
            stats.applied_operation_count, stats.destination_dir
        );
    }
    if profile.is_some() {
        crate::cli::reporting::print_profile_summary(profile.as_deref());
    }
    Ok(())
}

pub(crate) fn run_reconstructed(
    command: crate::cli::ReconstructedCommands,
    profile: &crate::cli::Profile,
) -> crate::api::Result<()> {
    match command {
        crate::cli::ReconstructedCommands::Plan {
            repo,
            snapshot_id,
            destination,
            publication,
            remove_added_files,
            metadata_only,
            source_snapshot,
            source_dir,
            runtime,
        } => {
            use std::io::Write;
            let publication = if publication == "selective" {
                crate::api::InstallationPublication::Selective
            } else {
                crate::api::InstallationPublication::ReplaceDirectory
            };
            let extra_files = if remove_added_files
                || publication == crate::api::InstallationPublication::ReplaceDirectory
            {
                crate::api::ExtraFilePolicy::Remove
            } else {
                crate::api::ExtraFilePolicy::Preserve
            };
            let mut output = std::io::BufWriter::new(std::io::stdout().lock());
            let summary = crate::api::visit_installation_plan(
                &crate::api::SnapshotReconstructFromRepoRequest {
                    repo_path: repo,
                    snapshot_id,
                    destination_dir: destination,
                    staging_dir: None,
                    source_snapshot_id: source_snapshot,
                    source_dir,
                    runtime_hints: runtime.hints(),
                    control: crate::cli::control_with_profile(profile)
                        .with_progress(CliProgressSink::new()),
                },
                publication,
                extra_files,
                if metadata_only {
                    crate::api::InstallationInspection::MetadataOnly
                } else {
                    crate::api::InstallationInspection::Content
                },
                &mut |event| {
                    serde_json::to_writer(&mut output, &event)?;
                    writeln!(output)?;
                    Ok(())
                },
            )?;
            serde_json::to_writer(
                &mut output,
                &serde_json::json!({"kind":"summary", "summary":summary}),
            )?;
            writeln!(output)?;
            output.flush()?;
            Ok(())
        }

        crate::cli::ReconstructedCommands::Verify {
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
        crate::cli::ReconstructedCommands::Repair {
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
        crate::cli::ReconstructedCommands::Corrupt {
            destination,
            plan,
            stats_json_out,
            yes,
        } => corrupt(
            CorruptCommand {
                destination,
                plan,
                stats_json_out,
                yes,
            },
            profile,
        ),
    }
}

use std::path::PathBuf;

use crate::cli::common::SnapshotReconstructStats;
use crate::cli::common::SnapshotVerifyStats;
use crate::cli::reporting::optional_output_paths;
use crate::cli::reporting::print_profile_summary;
use crate::cli::reporting::ratio_pct;
use crate::cli::reporting::reconstruct_timings_summary;
use crate::cli::reporting::verify_timings_summary;

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
    let execution = crate::api::reconstruct_snapshot_from_repo(
        crate::api::SnapshotReconstructFromRepoRequest {
            repo_path: command.repo.clone(),
            snapshot_id: command.snapshot_id.clone(),
            destination_dir: command.destination.clone(),
            staging_dir: command.staging_dir.clone(),
            source_snapshot_id: command.source_snapshot_id.clone(),
            source_dir: command.source_dir.clone(),
            runtime_hints: command.runtime.hints(),
            control: crate::cli::control_with_profile(profile)
                .with_progress(CliProgressSink::new()),
        },
    )?;
    let tm = execution.snapshot_manifest;
    let result = execution.reconstruct_result;
    let reused_source_chunk_percent = ratio_pct(
        result.reused_source_chunk_count as u64,
        result.required_chunk_count as u64,
    );
    let stats = SnapshotReconstructStats {
        command: "snapshot reconstruct",
        repo_root: execution.repo_root.display().to_string(),
        snapshot_id: result.snapshot_id.clone(),
        destination_dir: result.destination_dir.display().to_string(),
        staging_dir: result.staging_dir.display().to_string(),
        source_snapshot_id: command.source_snapshot_id.clone(),
        source_destination_dir: execution
            .source_destination_dir
            .as_ref()
            .map(|path| path.display().to_string()),
        file_count: tm.file_count(),
        required_chunk_count: result.required_chunk_count,
        required_bundle_count: result.required_bundle_count,
        required_artifact_count: result.required_artifact_count,
        resolved_bundle_file_count: execution.resolved_bundle_file_count,
        resolved_artifact_file_count: execution.resolved_artifact_file_count,
        reused_source_chunk_count: result.reused_source_chunk_count,
        reused_source_chunk_percent,
        total_logical_size: tm.total_logical_size,
        total_compressed_size_estimate: tm.total_compressed_size_estimate,
        reconstruct_timings: reconstruct_timings_summary(&result.reconstruct_timings),
        verify_timings: verify_timings_summary(&result.verify_timings),
        outputs: optional_output_paths(&command.stats_json_out),
    };
    write_report(&command.stats_json_out, &stats, profile.as_deref())?;

    if verbose_output_enabled() {
        println!(
            "reconstructed snapshot {} to {}",
            stats.snapshot_id, stats.destination_dir
        );
        println!(
            "reused {} of {} chunks from source ({:.2}%)",
            stats.reused_source_chunk_count,
            stats.required_chunk_count,
            stats.reused_source_chunk_percent
        );
    }
    if profile.is_some() {
        print_profile_summary(profile.as_deref());
    }
    Ok(())
}

pub(crate) fn verify_snapshot(command: VerifyCommand) -> crate::api::Result<()> {
    let progress = CliProgressSink::new();
    let execution =
        crate::api::verify_installed_snapshot(crate::api::SnapshotVerifyFromRepoRequest {
            repo_path: command.repo.clone(),
            snapshot_id: command.snapshot_id.clone(),
            destination_dir: command.destination.clone(),
            verify_mode: command.verify_mode,
            runtime_hints: command.runtime_hints,
            control: crate::cli::control_with_profile(&command.profile).with_progress(progress),
        })?;
    let repo_root = execution.repo_root;
    let tm = execution.snapshot_manifest;
    let classification = execution.classification;
    let timings = execution.verify_timings;
    let missing_file_count = classification.missing_files.len();
    let corrupt_file_count = classification.corrupt_files.len();
    let extra_file_count = classification.extra_files.len();
    let errors = classification.into_messages();
    let stats = SnapshotVerifyStats {
        command: "reconstructed verify",
        repo_root: repo_root.display().to_string(),
        snapshot_id: tm.snapshot_id.clone(),
        destination_dir: command.destination.display().to_string(),
        verify_mode: command.verify_mode.as_str().to_string(),
        expected_file_count: tm.file_count(),
        missing_file_count,
        corrupt_file_count,
        extra_file_count,
        verification_passed: errors.is_empty(),
        errors: errors.clone(),
        timings: verify_timings_summary(&timings),
        outputs: optional_output_paths(&command.stats_json_out),
    };
    write_report(&command.stats_json_out, &stats, command.profile.as_deref())?;

    if errors.is_empty() {
        if verbose_output_enabled() {
            println!("verification passed for {}", command.destination.display());
        }
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
