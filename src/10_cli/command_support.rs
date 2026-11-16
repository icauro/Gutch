//! Shared CLI helpers: report statistics, artifact URL parsing and confirmation prompts.

use std::io;
use std::io::Write;
use std::path::PathBuf;

use serde::Serialize;

use crate::cli::reporting::BuildTimingsSummary;
use crate::cli::reporting::ReconstructTimingsSummary;
use crate::cli::reporting::VerifyTimingsSummary;
use crate::manifest::SnapshotManifest;
use crate::manifest::repo_manifest::RepoManifest;
use crate::repo::paths::RepoPaths;
use crate::repo::read::load_gutch_repo_manifest;
use crate::repo::read::load_snapshot_manifest;
use crate::repo::snapshot_manifests::snapshot_metadata_path;
use crate::transfer::types::ArtifactDownload;
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
    pub(crate) resolved_bundle_file_count: usize,
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

#[derive(Debug, Serialize)]
pub(crate) struct SnapshotReconstructStats {
    pub(crate) command: &'static str,
    pub(crate) repo_root: String,
    pub(crate) snapshot_id: String,
    pub(crate) destination_dir: String,
    pub(crate) staging_dir: String,
    pub(crate) source_snapshot_id: Option<String>,
    pub(crate) source_destination_dir: Option<String>,
    pub(crate) file_count: usize,
    pub(crate) required_chunk_count: usize,
    pub(crate) required_bundle_count: usize,
    pub(crate) required_artifact_count: usize,
    pub(crate) resolved_bundle_file_count: usize,
    pub(crate) resolved_artifact_file_count: usize,
    pub(crate) reused_source_chunk_count: usize,
    pub(crate) reused_source_chunk_percent: f64,
    pub(crate) total_logical_size: u64,
    pub(crate) total_compressed_size_estimate: u64,
    pub(crate) reconstruct_timings: ReconstructTimingsSummary,
    pub(crate) verify_timings: VerifyTimingsSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct SnapshotFetchReconstructStats {
    pub(crate) command: &'static str,
    pub(crate) repo_root: String,
    pub(crate) snapshot_id: String,
    pub(crate) destination_dir: String,
    pub(crate) staging_dir: String,
    pub(crate) source_snapshot_id: Option<String>,
    pub(crate) source_destination_dir: Option<String>,
    pub(crate) file_count: usize,
    pub(crate) required_chunk_count: usize,
    pub(crate) required_bundle_count: usize,
    pub(crate) downloaded_bundle_count: usize,
    pub(crate) skipped_existing_bundle_count: usize,
    pub(crate) required_artifact_count: usize,
    pub(crate) downloaded_artifact_count: usize,
    pub(crate) skipped_existing_artifact_count: usize,
    pub(crate) transfer_policy: &'static str,
    pub(crate) initial_transfer_concurrency: usize,
    pub(crate) maximum_transfer_concurrency: usize,
    pub(crate) peak_transfer_concurrency: usize,
    pub(crate) transfer_adjustment_count: usize,
    pub(crate) transfer_retry_count: usize,
    pub(crate) transfer_bytes: u64,
    pub(crate) transfer_total_bytes: u64,
    pub(crate) transfer_seconds: f64,
    pub(crate) transfer_mib_per_second: f64,
    pub(crate) reused_source_chunk_count: usize,
    pub(crate) reused_source_chunk_percent: f64,
    pub(crate) total_logical_size: u64,
    pub(crate) total_compressed_size_estimate: u64,
    pub(crate) reconstruct_timings: ReconstructTimingsSummary,
    pub(crate) verify_timings: VerifyTimingsSummary,
}

pub(crate) fn load_snapshot_manifest_from_repo(
    repo_root: &Path,
    snapshot_id: &str,
) -> crate::Result<(RepoPaths, RepoManifest, PathBuf, SnapshotManifest)> {
    let (repo_paths, repo_manifest) = load_gutch_repo_manifest(repo_root)?;
    let manifest_path = snapshot_metadata_path(&repo_paths, snapshot_id)?;
    if !manifest_path.exists() {
        return Err(crate::Error::invalid_input(format!(
            "snapshot metadata not found at {}",
            manifest_path.display()
        )));
    }

    let snapshot_manifest = load_snapshot_manifest(&repo_paths, snapshot_id)?;
    Ok((repo_paths, repo_manifest, manifest_path, snapshot_manifest))
}

pub(crate) fn parse_artifact_downloads(
    artifact_args: &[String],
) -> crate::Result<Vec<ArtifactDownload>> {
    artifact_args
        .iter()
        .map(|artifact_arg| {
            let Some((artifact_id, url)) = artifact_arg.split_once('=') else {
                return Err(crate::Error::invalid_input(format!(
                    "invalid --artifact value '{}'; use <artifact-id>=<url>",
                    artifact_arg
                )));
            };
            let artifact_id = artifact_id.trim();
            let url = url.trim();
            if artifact_id.is_empty() || url.is_empty() {
                return Err(crate::Error::invalid_input(format!(
                    "invalid --artifact value '{}'; use <artifact-id>=<url>",
                    artifact_arg
                )));
            }

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
