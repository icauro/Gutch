use crate::cli::actions;
use crate::cli::common::parse_bundle_downloads;
use crate::cli::progress::CliProgressSink;
use crate::cli::verbose_output_enabled;
use gutch as g;

pub(crate) fn fetch(
    repo: std::path::PathBuf,
    snapshot_id: String,
    metadata_url: String,
    product_metadata_url: Option<String>,
    blueprint_url: String,
    repo_metadata_url: Option<String>,
    chunk_index_url: Option<String>,
    bundle_downloads_endpoint: Option<String>,
    session_id: Option<String>,
    bundles: Vec<String>,
) -> g::Result<()> {
    let progress = CliProgressSink::new();
    let result = g::fetch_snapshot(&g::SnapshotFetchRequest {
        repo_path: repo,
        snapshot_id,
        repo_metadata_url,
        metadata_url,
        product_metadata_url,
        blueprint_url,
        chunk_index_url,
        bundle_downloads_endpoint,
        session_id,
        bundles: parse_bundle_downloads(&bundles)?,
        control: g::GutchOperationControl::default().with_progress(progress),
    })?;

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
    }
    Ok(())
}

pub(crate) fn reconstruct(
    repo: std::path::PathBuf,
    snapshot_id: String,
    destination: std::path::PathBuf,
    staging_dir: Option<std::path::PathBuf>,
    source_snapshot_id: Option<String>,
    source_dir: Option<std::path::PathBuf>,
    runtime: crate::cli::args::RuntimeArgs,
    stats_json_out: Option<std::path::PathBuf>,
) -> g::Result<()> {
    actions::snapshot_ops::reconstruct_snapshot(actions::snapshot_ops::ReconstructCommand {
        repo,
        snapshot_id,
        destination,
        staging_dir,
        source_snapshot: source_snapshot_id,
        source_dir,
        runtime_hints: runtime.hints(),
        stats_json_out,
    })
}

pub(crate) fn verify(
    repo: std::path::PathBuf,
    snapshot_id: String,
    destination: std::path::PathBuf,
    strict: bool,
    runtime: crate::cli::args::RuntimeArgs,
    stats_json_out: Option<std::path::PathBuf>,
) -> g::Result<()> {
    actions::snapshot_ops::verify_snapshot(actions::snapshot_ops::VerifyCommand {
        repo,
        snapshot_id,
        destination,
        verify_mode: if strict {
            g::VerifyMode::Strict
        } else {
            g::VerifyMode::Fast
        },
        runtime_hints: runtime.hints(),
        stats_json_out,
    })
}
