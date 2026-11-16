//! Shared CLI helpers: report statistics, artifact URL parsing and confirmation prompts.

use std::io;
use std::io::Write;
use std::path::PathBuf;

use serde::Serialize;

use crate::cli::output;
use crate::cli::reporting::BuildTimingsSummary;
use crate::cli::reporting::ReconstructTimingsSummary;
use crate::cli::reporting::VerifyTimingsSummary;
use crate::cli::reporting::print_profile_summary;
use crate::cli::reporting::print_transfer_timings;
use crate::cli::reporting::profile_mode;
use crate::cli::reporting::ratio_pct;
use crate::cli::reporting::reconstruct_timings_summary;
use crate::cli::reporting::verify_timings_summary;
use crate::cli::reporting::write_stats_json_output;
use crate::cli::verbose_output_enabled;
use crate::installation::InstallationReconstructResult;
use crate::manifest::SnapshotManifest;
use crate::manifest::repo_manifest::RepoManifest;
use crate::profiling::Profiler;
use crate::repo::paths::RepoPaths;
use crate::repo::read::load_repo_manifest;
use crate::repo::read::load_snapshot_manifest;
use crate::repo::snapshot_manifests::snapshot_metadata_path;
use crate::sync::fetch_types::SnapshotFetchResult;
use crate::transfer::types::ArtifactDownload;
use crate::transfer::types::TransferStats;
use std::path::Path;

#[derive(Debug, Serialize)]
pub(crate) struct SnapshotCaptureStats {
    pub(crate) command: &'static str,
    pub(crate) repo_root: String,
    pub(crate) input_path: String,
    pub(crate) snapshot_id: String,
    pub(crate) manifest_path: String,
    pub(crate) repo_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) remote_repo_id: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) version_tag: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) os: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) arch: String,
    pub(crate) storage_mode: String,
    pub(crate) file_count: usize,
    pub(crate) total_logical_size: u64,
    pub(crate) total_compressed_size_estimate: u64,
    pub(crate) required_chunk_count: usize,
    pub(crate) required_bundle_count: usize,
    pub(crate) generated_bundle_count: usize,
    pub(crate) generated_loose_chunk_count: usize,
    pub(crate) reused_chunk_count: usize,
    pub(crate) new_chunk_count: usize,
    pub(crate) reused_chunk_percent: f64,
    pub(crate) reused_bundle_count: usize,
    pub(crate) reused_bundle_percent: f64,
    pub(crate) timings: BuildTimingsSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct SnapshotCaptureManyStats {
    pub(crate) command: &'static str,
    pub(crate) repo_root: String,
    pub(crate) snapshot_count: usize,
    pub(crate) total_logical_size: u64,
    pub(crate) total_compressed_size_estimate: u64,
    pub(crate) generated_bundle_count: usize,
    pub(crate) generated_loose_chunk_count: usize,
    pub(crate) total_capture_seconds: Option<f64>,
    pub(crate) snapshots: Vec<SnapshotCaptureStats>,
}

#[derive(Debug, Serialize)]
pub(crate) struct InstallationVerifyStats {
    pub(crate) command: &'static str,
    pub(crate) repo_root: String,
    pub(crate) snapshot_id: String,
    pub(crate) destination_dir: String,
    pub(crate) verify_mode: String,
    pub(crate) expected_file_count: usize,
    pub(crate) missing_file_count: usize,
    pub(crate) corrupt_file_count: usize,
    pub(crate) extra_file_count: usize,
    pub(crate) verification_passed: bool,
    pub(crate) errors: Vec<String>,
    pub(crate) extra_files: Vec<String>,
    pub(crate) timings: VerifyTimingsSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct InstallationRepairStats {
    pub(crate) command: &'static str,
    pub(crate) repo_root: String,
    pub(crate) snapshot_id: String,
    pub(crate) destination_dir: String,
    pub(crate) remove_extra_files: bool,
    pub(crate) missing_file_count: usize,
    pub(crate) corrupt_file_count: usize,
    pub(crate) extra_file_count: usize,
    pub(crate) removed_extra_file_count: usize,
    pub(crate) required_bundle_count: usize,
    pub(crate) elapsed_seconds: Option<f64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct InstallationCorruptStats {
    pub(crate) command: &'static str,
    pub(crate) destination_dir: String,
    pub(crate) plan_path: String,
    pub(crate) applied_operation_count: usize,
    pub(crate) elapsed_seconds: Option<f64>,
}

/// Report of `installation reconstruct`, with or without `--fetch`.
#[derive(Debug, Serialize)]
pub(crate) struct SnapshotReconstructStats {
    pub(crate) command: &'static str,
    pub(crate) repo_root: String,
    pub(crate) snapshot_id: String,
    pub(crate) destination_dir: String,
    pub(crate) staging_dir: String,
    pub(crate) reuse_snapshot_id: Option<String>,
    pub(crate) reuse_dir: Option<String>,
    pub(crate) file_count: usize,
    pub(crate) required_chunk_count: usize,
    pub(crate) required_bundle_count: usize,
    pub(crate) required_artifact_count: usize,
    pub(crate) reused_chunk_count: usize,
    pub(crate) reused_chunk_percent: f64,
    pub(crate) total_logical_size: u64,
    pub(crate) total_compressed_size_estimate: u64,
    pub(crate) reconstruct_timings: ReconstructTimingsSummary,
    pub(crate) verify_timings: VerifyTimingsSummary,
    /// Present only when the reconstruction fetched its data first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) fetch: Option<SnapshotFetchStats>,
}

impl SnapshotReconstructStats {
    pub(crate) fn new(
        execution: &InstallationReconstructResult,
        reuse_snapshot_id: Option<String>,
        fetch: Option<SnapshotFetchStats>,
        profile: Option<&Profiler>,
    ) -> Self {
        let manifest = &execution.snapshot_manifest;
        let result = execution;
        Self {
            command: "installation reconstruct",
            repo_root: execution.repo_root.display().to_string(),
            snapshot_id: result.snapshot_id.clone(),
            destination_dir: result.destination_dir.display().to_string(),
            staging_dir: result.staging_dir.display().to_string(),
            reuse_snapshot_id,
            reuse_dir: execution
                .reuse_dir
                .as_ref()
                .map(|path| path.display().to_string()),
            file_count: manifest.file_count(),
            required_chunk_count: result.required_chunk_count,
            required_bundle_count: result.required_bundle_count,
            required_artifact_count: result.required_artifact_count,
            reused_chunk_count: result.reused_chunk_count,
            reused_chunk_percent: ratio_pct(
                result.reused_chunk_count as u64,
                result.required_chunk_count as u64,
            ),
            total_logical_size: manifest.total_logical_size,
            total_compressed_size_estimate: manifest.total_compressed_size_estimate,
            reconstruct_timings: reconstruct_timings_summary(
                &result.reconstruct_timings,
                profile_mode(profile),
            ),
            verify_timings: verify_timings_summary(&result.verify_timings, profile_mode(profile)),
            fetch,
        }
    }

    /// Writes the JSON report and the console summary.
    pub(crate) fn report(&self, profile: Option<&Profiler>) -> crate::Result<()> {
        write_stats_json_output(self, profile)?;
        let action = if self.fetch.is_some() {
            "downloaded and reconstructed"
        } else {
            "reconstructed"
        };
        output::line(format_args!(
            "{action} snapshot {} to {}",
            self.snapshot_id, self.destination_dir
        ));
        if verbose_output_enabled() {
            output::line(format_args!(
                "reused {} of {} chunks from existing content ({:.2}%)",
                self.reused_chunk_count, self.required_chunk_count, self.reused_chunk_percent
            ));
            if let Some(fetch) = &self.fetch {
                fetch.print_details();
            }
        }
        if profile.is_some() {
            print_profile_summary(profile);
        }
        Ok(())
    }
}

/// Downloads made by `snapshot fetch` or `installation reconstruct --fetch`.
#[derive(Debug, Serialize)]
pub(crate) struct SnapshotFetchStats {
    pub(crate) required_bundle_count: usize,
    pub(crate) downloaded_bundle_count: usize,
    pub(crate) skipped_existing_bundle_count: usize,
    pub(crate) required_artifact_count: usize,
    pub(crate) downloaded_artifact_count: usize,
    pub(crate) skipped_existing_artifact_count: usize,
    pub(crate) transfer_policy: &'static str,
    pub(crate) transfer: TransferStats,
}

impl SnapshotFetchStats {
    pub(crate) fn new(result: &SnapshotFetchResult) -> Self {
        Self {
            required_bundle_count: result.required_bundle_count,
            downloaded_bundle_count: result.downloaded_bundle_count,
            skipped_existing_bundle_count: result.skipped_existing_bundle_count,
            required_artifact_count: result.required_artifact_count,
            downloaded_artifact_count: result.downloaded_artifact_count,
            skipped_existing_artifact_count: result.skipped_existing_artifact_count,
            transfer_policy: result.transfer_policy.name(),
            transfer: result.transfer_stats,
        }
    }

    pub(crate) fn print_details(&self) {
        output::line(format_args!(
            "bundles: required={}, downloaded={}, existing={}",
            self.required_bundle_count,
            self.downloaded_bundle_count,
            self.skipped_existing_bundle_count
        ));
        output::line(format_args!(
            "artifacts: required={}, downloaded={}, existing={}",
            self.required_artifact_count,
            self.downloaded_artifact_count,
            self.skipped_existing_artifact_count
        ));
        print_transfer_timings(&self.transfer);
    }
}

pub(crate) fn load_snapshot_manifest_from_repo(
    repo_root: &Path,
    snapshot_id: &str,
) -> crate::Result<(RepoPaths, RepoManifest, PathBuf, SnapshotManifest)> {
    let repo_manifest = load_repo_manifest(repo_root)?;
    let repo_paths = RepoPaths::from_repo_root(repo_root);
    let manifest_path = snapshot_metadata_path(&repo_paths, snapshot_id)?;
    if !manifest_path.exists() {
        return Err(crate::Error::invalid_input(format!(
            "snapshot metadata not found at {}",
            manifest_path.display()
        )));
    }

    let snapshot_manifest = load_snapshot_manifest(repo_root, snapshot_id)?;
    Ok((repo_paths, repo_manifest, manifest_path, snapshot_manifest))
}

pub(crate) fn parse_artifact_downloads(
    artifact_args: &[String],
) -> crate::Result<Vec<ArtifactDownload>> {
    artifact_args
        .iter()
        .map(|artifact_arg| {
            let (artifact_id, url) = artifact_arg
                .split_once('=')
                .map(|(id, url)| (id.trim(), url.trim()))
                .filter(|(id, url)| !id.is_empty() && !url.is_empty())
                .ok_or_else(|| {
                    crate::Error::invalid_input(format!(
                        "invalid --artifact-url value '{artifact_arg}'; use ARTIFACT_ID=URL"
                    ))
                })?;

            Ok(ArtifactDownload {
                artifact_id: artifact_id.to_string(),
                url: url.to_string(),
            })
        })
        .collect()
}

pub(crate) fn prompt_yes_no(prompt: &str, default_yes: bool) -> crate::Result<bool> {
    // Structured output holds the plan back, so a prompt would ask blindly.
    if super::output::structured() {
        return Err(crate::Error::invalid_input(
            "confirmation prompts are unavailable with --json or --json-lines; review the plan with --dry-run and confirm with --yes",
        ));
    }
    loop {
        let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
        eprint!("{} {}: ", prompt, suffix);
        io::stderr()
            .flush()
            .map_err(|err| crate::Error::invalid_input(format!("io error: {}", err)))?;
        let mut line = String::new();
        let bytes_read = io::stdin()
            .read_line(&mut line)
            .map_err(|err| crate::Error::invalid_input(format!("io error: {}", err)))?;
        if bytes_read == 0 {
            return Err(crate::Error::invalid_input("confirmation requires input"));
        }
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
