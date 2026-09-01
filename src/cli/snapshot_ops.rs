use crate::cli::actions;
use crate::cli::common::parse_bundle_downloads;
use crate::cli::progress::CliProgressSink;
use crate::cli::verbose_output_enabled;
use gutch as g;
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
}

pub(crate) fn fetch(command: FetchCommand) -> g::Result<()> {
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
    } = command;
    let progress = CliProgressSink::new();
    let bundle_url_provider = bundle_downloads_endpoint
        .map(|endpoint| {
            let client = reqwest::blocking::Client::builder()
                .user_agent("Gutch CLI")
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30 * 60))
                .build()
                .map_err(|err| {
                    g::Error::manifest(format!("failed to create download client: {err}"))
                })?;
            let session_id = session_id.clone();
            Ok::<Arc<dyn g::BundleUrlProvider>, g::Error>(Arc::new(
                move |bundle_ids: &[String], control: &g::GutchOperationControl| {
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
    let result = g::fetch_snapshot(&g::SnapshotFetchRequest {
        repo_path: repo,
        snapshot_id,
        repo_metadata_url,
        metadata_url,
        product_metadata_url,
        blueprint_url,
        chunk_index_url,
        bundles: direct_downloads,
        bundle_url_provider,
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
        println!(
            "artifacts: required={}, downloaded={}, existing={}",
            result.required_artifact_count,
            result.downloaded_artifact_count,
            result.skipped_existing_artifact_count
        );
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

pub(crate) fn reconstruct(command: ReconstructCommand) -> g::Result<()> {
    actions::snapshot_ops::reconstruct_snapshot(actions::snapshot_ops::ReconstructCommand {
        repo: command.repo,
        snapshot_id: command.snapshot_id,
        destination: command.destination,
        staging_dir: command.staging_dir,
        source_snapshot: command.source_snapshot_id,
        source_dir: command.source_dir,
        runtime_hints: command.runtime.hints(),
        stats_json_out: command.stats_json_out,
    })
}

fn resolve_bundle_urls(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    session_id: Option<&str>,
    bundle_ids: &[String],
    control: &g::GutchOperationControl,
) -> g::Result<Vec<g::BundleDownload>> {
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
        .map_err(|err| g::Error::manifest(format!("bundle URL request failed: {err}")))?;
    if !response.status().is_success() {
        return Err(g::Error::manifest(format!(
            "bundle URL request failed with status {}",
            response.status()
        )));
    }
    let bytes = g::read_bounded_response(response, MAX_BUNDLE_URL_RESPONSE_BYTES, || {
        control.checkpoint()
    })?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct BundleUrlsResponse {
        #[serde(default)]
        bundles: Vec<g::BundleDownload>,
    }
    Ok(serde_json::from_slice::<BundleUrlsResponse>(&bytes)?.bundles)
}

fn parse_artifact_url_manifest(path: &std::path::Path) -> g::Result<Vec<g::BundleDownload>> {
    let bytes = std::fs::read(path)?;
    let urls = serde_json::from_slice::<std::collections::BTreeMap<String, String>>(&bytes)
        .map_err(|error| {
            g::Error::manifest(format!(
                "invalid artifact URL manifest '{}': {error}",
                path.display()
            ))
        })?;
    urls.into_iter()
        .map(|(bundle_id, url)| {
            if bundle_id.trim().is_empty() || url.trim().is_empty() {
                return Err(g::Error::manifest(format!(
                    "artifact URL manifest '{}' contains an empty artifact ID or URL",
                    path.display()
                )));
            }
            Ok(g::BundleDownload { bundle_id, url })
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
