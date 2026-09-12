use std::io;
use std::io::Write;
use std::path::PathBuf;

use serde::Serialize;

use crate::cli::reporting::BuildTimingsSummary;
use crate::cli::reporting::OptionalOutputPaths;
use crate::cli::reporting::ReconstructTimingsSummary;
use crate::cli::reporting::VerifyTimingsSummary;
use crate::cli::reporting::write_stats_json_output;

#[derive(Debug, Serialize)]
pub(crate) struct SnapshotCaptureStats {
    pub(crate) command: &'static str,
    pub(crate) repo_root: String,
    pub(crate) input_path: String,
    pub(crate) snapshot_id: String,
    pub(crate) manifest_path: String,
    pub(crate) product_id: String,
    pub(crate) remote_repo_id: String,
    pub(crate) version_id: String,
    pub(crate) os: String,
    pub(crate) arch: String,
    pub(crate) storage_mode: String,
    pub(crate) file_count: usize,
    pub(crate) total_logical_size: u64,
    pub(crate) total_compressed_size_estimate: u64,
    pub(crate) required_chunk_count: usize,
    pub(crate) required_bundle_count: usize,
    pub(crate) generated_bundle_count: usize,
    pub(crate) generated_bundle_bytes: u64,
    pub(crate) generated_chunk_count: usize,
    pub(crate) generated_chunk_bytes: u64,
    pub(crate) reused_chunk_count: usize,
    pub(crate) new_chunk_count: usize,
    pub(crate) reused_chunk_percent: f64,
    pub(crate) reused_bundle_count: usize,
    pub(crate) reused_bundle_percent: f64,
    pub(crate) timings: BuildTimingsSummary,
    pub(crate) outputs: OptionalOutputPaths,
}

#[derive(Debug, Serialize)]
pub(crate) struct SnapshotCaptureManyStats {
    pub(crate) command: &'static str,
    pub(crate) repo_root: String,
    pub(crate) snapshot_count: usize,
    pub(crate) total_logical_size: u64,
    pub(crate) total_compressed_size_estimate: u64,
    pub(crate) generated_bundle_count: usize,
    pub(crate) generated_bundle_bytes: u64,
    pub(crate) generated_chunk_count: usize,
    pub(crate) generated_chunk_bytes: u64,
    pub(crate) total_build_seconds: f64,
    pub(crate) snapshots: Vec<SnapshotCaptureStats>,
    pub(crate) outputs: OptionalOutputPaths,
}

#[derive(Debug, Serialize)]
pub(crate) struct SnapshotVerifyStats {
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
    pub(crate) outputs: OptionalOutputPaths,
}

#[derive(Debug, Serialize)]
pub(crate) struct InstallationRepairStats {
    pub(crate) command: &'static str,
    pub(crate) repo_root: String,
    pub(crate) snapshot_id: String,
    pub(crate) destination_dir: String,
    pub(crate) remove_added_files: bool,
    pub(crate) missing_file_count: usize,
    pub(crate) corrupt_file_count: usize,
    pub(crate) extra_file_count: usize,
    pub(crate) removed_extra_file_count: usize,
    pub(crate) reconstructed_extra_file_count: usize,
    pub(crate) resolved_bundle_file_count: usize,
    pub(crate) elapsed_seconds: f64,
    pub(crate) outputs: OptionalOutputPaths,
}

#[derive(Debug, Serialize)]
pub(crate) struct InstallationCorruptStats {
    pub(crate) command: &'static str,
    pub(crate) destination_dir: String,
    pub(crate) plan_path: String,
    pub(crate) applied_operation_count: usize,
    pub(crate) elapsed_seconds: f64,
    pub(crate) outputs: OptionalOutputPaths,
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
    pub(crate) outputs: OptionalOutputPaths,
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
    pub(crate) outputs: OptionalOutputPaths,
}

pub(crate) fn write_report<T: Serialize>(
    stats_json_out: &Option<PathBuf>,
    stats: &T,
    profile: Option<&crate::profile::GutchProfiler>,
) -> crate::api::Result<()> {
    write_stats_json_output(stats_json_out, stats, profile)
}

pub(crate) fn load_snapshot_manifest_from_repo(
    repo_root: &std::path::Path,
    snapshot_id: &str,
) -> crate::api::Result<(
    crate::api::GutchRepoPaths,
    crate::manifest::gutch_repo::GutchRepoManifest,
    PathBuf,
    crate::manifest::snapshot::SnapshotManifest,
)> {
    let (repo_paths, repo_manifest) = crate::api::load_gutch_repo_manifest(repo_root)?;
    let manifest_path = crate::api::snapshot_metadata_path(&repo_paths, snapshot_id)?;
    if !manifest_path.exists() {
        return Err(crate::api::Error::manifest(format!(
            "snapshot manifest not found at {}",
            manifest_path.display()
        )));
    }

    let snapshot_manifest = crate::api::load_snapshot_manifest(&repo_paths, snapshot_id)?;
    Ok((repo_paths, repo_manifest, manifest_path, snapshot_manifest))
}

pub(crate) fn parse_artifact_downloads(
    artifact_args: &[String],
) -> crate::api::Result<Vec<crate::api::ArtifactDownload>> {
    artifact_args
        .iter()
        .map(|artifact_arg| {
            let Some((artifact_id, url)) = artifact_arg.split_once('=') else {
                return Err(crate::api::Error::manifest(format!(
                    "invalid --artifact value '{}'; use <artifact-id>=<url>",
                    artifact_arg
                )));
            };
            let artifact_id = artifact_id.trim();
            let url = url.trim();
            if artifact_id.is_empty() || url.is_empty() {
                return Err(crate::api::Error::manifest(format!(
                    "invalid --artifact value '{}'; use <artifact-id>=<url>",
                    artifact_arg
                )));
            }

            Ok(crate::api::ArtifactDownload {
                artifact_id: artifact_id.to_string(),
                url: url.to_string(),
            })
        })
        .collect()
}

pub(crate) fn prompt_yes_no(prompt: &str, default_yes: bool) -> crate::api::Result<bool> {
    loop {
        let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
        print!("{} {}: ", prompt, suffix);
        io::stdout()
            .flush()
            .map_err(|err| crate::api::Error::manifest(format!("io error: {}", err)))?;
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .map_err(|err| crate::api::Error::manifest(format!("io error: {}", err)))?;
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
