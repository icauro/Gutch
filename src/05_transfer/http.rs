//! HTTP client construction and bounded streaming downloads.

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use crate::Error;
use crate::Result;
use crate::progress::OperationControl;

/// Error responses contribute at most this much body text to a one-line message.
const MAX_ERROR_DETAIL_BYTES: usize = 512;
/// Body bytes are handed on in slices this large so cancellation stays responsive.
const STREAM_SLICE_BYTES: usize = 64 * 1024;
/// How often a pending request polls the host's synchronous cancellation hook.
const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One operation-owned network runtime, shared by its synchronous download workers.
/// Dropping a cancelled request drops its I/O future; no detached blocking reader remains.
#[derive(Clone)]
pub(crate) struct DownloadClient {
    client: reqwest::Client,
    runtime: Arc<tokio::runtime::Runtime>,
}

pub(crate) fn build_download_client() -> Result<DownloadClient> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("gutch-http")
        .enable_all()
        .build()?;
    let client = reqwest::Client::builder()
        .user_agent("Gutch")
        .connect_timeout(Duration::from_secs(10))
        // A stalled body fails well before the whole-request limit.
        .read_timeout(Duration::from_secs(60))
        .timeout(Duration::from_secs(30 * 60))
        .build()
        .map_err(|error| {
            Error::download(format!("failed to create download client: {error}"), false)
        })?;
    Ok(DownloadClient {
        client,
        runtime: Arc::new(runtime),
    })
}

/// Source URLs may carry access tokens, so errors never echo them. A malformed
/// request or redirect policy failure repeats identically and is not retried.
pub(crate) fn request_error(context: &str, error: reqwest::Error) -> Error {
    let retryable = !(error.is_builder() || error.is_redirect());
    Error::download(format!("{context}: {}", error.without_url()), retryable)
}

/// Server faults, throttling and timeouts may clear on a later attempt.
pub(crate) fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    status.is_server_error()
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
}

fn size_limit_error() -> Error {
    Error::download("response exceeded the allowed size", false)
}

pub(crate) fn download_url_streaming<F, C>(
    client: &DownloadClient,
    url: &str,
    control: &OperationControl,
    maximum_bytes: u64,
    mut on_content_length: C,
    mut on_chunk: F,
) -> Result<()>
where
    F: FnMut(&[u8]) -> Result<()>,
    C: FnMut(Option<u64>) -> Result<()>,
{
    control.checkpoint()?;
    client.runtime.block_on(async {
        let transfer = async {
            let mut response = client
                .client
                .get(url)
                .send()
                .await
                .map_err(|error| request_error("download request failed", error))?;
            if !response.status().is_success() {
                let status = response.status();
                let mut body = Vec::new();
                while body.len() < MAX_ERROR_DETAIL_BYTES {
                    control.checkpoint()?;
                    let Ok(Some(chunk)) = response.chunk().await else {
                        break;
                    };
                    let length = chunk.len().min(MAX_ERROR_DETAIL_BYTES - body.len());
                    body.extend_from_slice(&chunk[..length]);
                }
                return Err(Error::download(
                    format_http_failure(status, &String::from_utf8_lossy(&body)),
                    is_retryable_status(status),
                ));
            }
            let total_bytes = response.content_length();
            if total_bytes.is_some_and(|total| total > maximum_bytes) {
                return Err(size_limit_error());
            }
            on_content_length(total_bytes)?;
            let mut transferred_bytes = 0_u64;
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| request_error("download body failed", error))?
            {
                transferred_bytes = transferred_bytes.saturating_add(chunk.len() as u64);
                if transferred_bytes > maximum_bytes {
                    return Err(size_limit_error());
                }
                for bytes in chunk.chunks(STREAM_SLICE_BYTES) {
                    control.checkpoint()?;
                    on_chunk(bytes)?;
                }
            }
            control.checkpoint()?;
            Ok(())
        };
        // The host supplies a synchronous hook, so poll it even while headers or
        // either a successful or error response body is stalled.
        tokio::select! {
            biased;
            error = async {
                loop {
                    if let Err(error) = control.checkpoint() {
                        return error;
                    }
                    tokio::time::sleep(CANCELLATION_POLL_INTERVAL).await;
                }
            } => Err(error),
            result = transfer => result,
        }
    })
}

fn format_http_failure(status: reqwest::StatusCode, body: &str) -> String {
    let body = body.trim();
    let detail = if body.is_empty() {
        String::new()
    } else {
        format!(": {}", body.replace(['\r', '\n'], " "))
    };
    format!("download request failed with status {status}{detail}")
}

/// Reads a blocking response body, failing once it passes `maximum_bytes`.
pub(crate) fn read_bounded_response<C>(
    mut response: reqwest::blocking::Response,
    maximum_bytes: u64,
    checkpoint: C,
) -> Result<Vec<u8>>
where
    C: FnMut() -> Result<()>,
{
    if response
        .content_length()
        .is_some_and(|length| length > maximum_bytes)
    {
        return Err(size_limit_error());
    }
    read_bounded(&mut response, maximum_bytes, checkpoint)
}

fn read_bounded<R, C>(mut reader: R, maximum_bytes: u64, mut checkpoint: C) -> Result<Vec<u8>>
where
    R: Read,
    C: FnMut() -> Result<()>,
{
    let mut buffer = vec![0_u8; STREAM_SLICE_BYTES];
    let mut bytes = Vec::new();
    loop {
        checkpoint()?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if (bytes.len() + read) as u64 > maximum_bytes {
            return Err(size_limit_error());
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    checkpoint()?;
    Ok(bytes)
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
