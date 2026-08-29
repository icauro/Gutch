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
    let result = g::fetch_snapshot(&g::SnapshotFetchRequest {
        repo_path: repo,
        snapshot_id,
        repo_metadata_url,
        metadata_url,
        product_metadata_url,
        blueprint_url,
        chunk_index_url,
        bundles: parse_bundle_downloads(&bundles)?,
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
    let bytes = read_response_bounded(response, MAX_BUNDLE_URL_RESPONSE_BYTES, || {
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

fn read_response_bounded<F>(
    mut response: reqwest::blocking::Response,
    maximum_bytes: u64,
    mut checkpoint: F,
) -> g::Result<Vec<u8>>
where
    F: FnMut() -> g::Result<()>,
{
    if response
        .content_length()
        .is_some_and(|length| length > maximum_bytes)
    {
        return Err(g::Error::manifest("response exceeded the allowed size"));
    }
    let mut bytes = Vec::new();
    crate::transport::read_bounded_chunks(
        &mut response,
        Some(maximum_bytes),
        &mut checkpoint,
        |chunk| {
            bytes.extend_from_slice(chunk);
            Ok(())
        },
    )?;
    Ok(bytes)
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
