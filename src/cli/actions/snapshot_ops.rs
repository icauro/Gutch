use std::path::PathBuf;

use crate::cli::common::{SnapshotReconstructStats, SnapshotVerifyStats, write_report};
use crate::cli::progress::CliProgressSink;
use crate::cli::reporting::{
    optional_output_paths, print_reconstruct_timings, print_verify_timings, ratio_pct,
    reconstruct_timings_summary, verify_timings_summary,
};
use crate::cli::verbose_output_enabled;
use gutch as g;

pub(crate) struct ReconstructCommand {
    pub(crate) repo: PathBuf,
    pub(crate) snapshot_id: String,
    pub(crate) destination: PathBuf,
    pub(crate) staging_dir: Option<PathBuf>,
    pub(crate) source_snapshot: Option<String>,
    pub(crate) source_dir: Option<PathBuf>,
    pub(crate) runtime_hints: g::RuntimeResourceHints,
    pub(crate) stats_json_out: Option<PathBuf>,
}

pub(crate) struct VerifyCommand {
    pub(crate) repo: PathBuf,
    pub(crate) snapshot_id: String,
    pub(crate) destination: PathBuf,
    pub(crate) verify_mode: g::VerifyMode,
    pub(crate) runtime_hints: g::RuntimeResourceHints,
    pub(crate) stats_json_out: Option<PathBuf>,
}

pub(crate) fn reconstruct_snapshot(command: ReconstructCommand) -> g::Result<()> {
    let execution = g::reconstruct_snapshot_from_repo(g::SnapshotReconstructFromRepoRequest {
        repo_path: command.repo.clone(),
        snapshot_id: command.snapshot_id.clone(),
        destination_dir: command.destination.clone(),
        staging_dir: command.staging_dir.clone(),
        source_snapshot_id: command.source_snapshot.clone(),
        source_dir: command.source_dir.clone(),
        runtime_hints: command.runtime_hints,
        control: g::GutchOperationControl::default(),
    })?;
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
        source_snapshot_id: command.source_snapshot.clone(),
        source_destination_dir: execution
            .source_destination_dir
            .as_ref()
            .map(|path| path.display().to_string()),
        file_count: tm.files.len(),
        required_chunk_count: result.required_chunk_count,
        required_bundle_count: result.required_bundle_count,
        resolved_bundle_file_count: execution.resolved_bundle_file_count,
        reused_source_chunk_count: result.reused_source_chunk_count,
        reused_source_chunk_percent,
        total_logical_size: tm.total_logical_size,
        total_compressed_size_estimate: tm.total_compressed_size_estimate,
        reconstruct_timings: reconstruct_timings_summary(&result.reconstruct_timings),
        verify_timings: verify_timings_summary(&result.verify_timings),
        outputs: optional_output_paths(&command.stats_json_out),
    };
    write_report(&command.stats_json_out, &stats)?;

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
    print_reconstruct_timings(&result.reconstruct_timings);
    print_verify_timings(&result.verify_timings);
    Ok(())
}

pub(crate) fn verify_snapshot(command: VerifyCommand) -> g::Result<()> {
    let progress = CliProgressSink::new();
    let execution = g::verify_snapshot_from_repo(g::SnapshotVerifyFromRepoRequest {
        repo_path: command.repo.clone(),
        snapshot_id: command.snapshot_id.clone(),
        destination_dir: command.destination.clone(),
        verify_mode: command.verify_mode,
        runtime_hints: command.runtime_hints,
        control: g::GutchOperationControl::default().with_progress(progress),
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
        command: "snapshot verify",
        repo_root: repo_root.display().to_string(),
        snapshot_id: tm.snapshot_id.clone(),
        destination_dir: command.destination.display().to_string(),
        verify_mode: command.verify_mode.as_str().to_string(),
        expected_file_count: tm.files.len(),
        missing_file_count,
        corrupt_file_count,
        extra_file_count,
        verification_passed: errors.is_empty(),
        errors: errors.clone(),
        timings: verify_timings_summary(&timings),
        outputs: optional_output_paths(&command.stats_json_out),
    };
    write_report(&command.stats_json_out, &stats)?;

    if errors.is_empty() {
        if verbose_output_enabled() {
            println!("verification passed for {}", command.destination.display());
        }
        print_verify_timings(&timings);
    } else {
        eprintln!("{} verification errors:", errors.len());
        for err in &errors {
            eprintln!("  - {}", err);
        }
        std::process::exit(1);
    }

    Ok(())
}
