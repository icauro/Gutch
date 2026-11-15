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
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;

    #[derive(Debug, Default)]
    struct Cancellation(AtomicBool);
    impl crate::progress::CancellationHook for Cancellation {
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Acquire)
        }
    }

    fn assert_stalled_download_cancels(response: &'static [u8], wait_for_chunk: bool) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/artifact", listener.local_addr().unwrap());
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server_ready = ready_tx.clone();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            socket.write_all(response).unwrap();
            if !wait_for_chunk {
                server_ready.send(()).unwrap();
            }
            // Keep the peer stalled until the caller has observed cancellation.
            let _ = release_rx.recv_timeout(Duration::from_secs(10));
        });
        let cancel = Arc::new(Cancellation::default());
        let control = OperationControl::default().with_cancellation(cancel.clone());
        let (result_tx, result_rx) = mpsc::channel();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("artifact");
        std::fs::write(&path, b"previous artifact").unwrap();
        let output = path.clone();
        let worker = std::thread::spawn(move || {
            let client = build_download_client().unwrap();
            let result = crate::filesystem::write_atomic_with(&output, |file| {
                download_url_streaming(
                    &client,
                    &url,
                    &control,
                    1024,
                    |_| Ok(()),
                    |chunk| {
                        file.write_all(chunk)?;
                        if wait_for_chunk {
                            let _ = ready_tx.send(());
                        }
                        Ok(())
                    },
                )
            });
            // Include network-runtime teardown in the completion deadline.
            drop(client);
            result_tx.send(result).unwrap();
        });
        let ready = ready_rx.recv_timeout(Duration::from_secs(5));
        cancel.0.store(true, Ordering::Release);
        let result = result_rx.recv_timeout(Duration::from_secs(2));
        let _ = release_tx.send(());
        server.join().unwrap();
        worker.join().unwrap();
        ready.unwrap();
        assert!(matches!(result.unwrap(), Err(Error::Cancelled)));
        assert_eq!(std::fs::read(path).unwrap(), b"previous artifact");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn cancellation_interrupts_stalled_response_headers() {
        assert_stalled_download_cancels(b"", false);
    }

    #[test]
    fn cancellation_interrupts_stalled_success_body_and_preserves_destination() {
        assert_stalled_download_cancels(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nx", true);
    }

    #[test]
    fn cancellation_interrupts_stalled_error_body() {
        assert_stalled_download_cancels(
            b"HTTP/1.1 503 Unavailable\r\nContent-Length: 100\r\n\r\nx",
            false,
        );
    }

    #[test]
    fn http_error_detail_is_normalized_to_one_line() {
        let message = super::format_http_failure(
            reqwest::StatusCode::FORBIDDEN,
            "  <Error>\n    <Code>AccessDenied</Code>\n  </Error>  ",
        );
        assert_eq!(
            message,
            "download request failed with status 403 Forbidden: <Error>     <Code>AccessDenied</Code>   </Error>"
        );
    }

    #[test]
    fn request_errors_never_echo_the_url() {
        // Bind then drop a listener so the port refuses connections.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let client = super::build_download_client().unwrap();
        let error = super::download_url_streaming(
            &client,
            &format!("http://127.0.0.1:{port}/chunk?token=secret-value"),
            &OperationControl::default(),
            1024,
            |_| Ok(()),
            |_| Ok(()),
        )
        .unwrap_err();
        assert!(!error.to_string().contains("secret-value"), "{error}");
        assert!(error.is_retryable_download());
    }

    #[test]
    fn only_transient_statuses_are_retryable() {
        for code in [408, 429, 500, 502, 503] {
            let status = reqwest::StatusCode::from_u16(code).unwrap();
            assert!(is_retryable_status(status), "{code}");
        }
        for code in [400, 401, 403, 404, 410] {
            let status = reqwest::StatusCode::from_u16(code).unwrap();
            assert!(!is_retryable_status(status), "{code}");
        }
    }

    /// Serves one canned response and returns the downloaded body.
    fn download_from(response: Vec<u8>, maximum_bytes: u64) -> Result<Vec<u8>> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/artifact", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            let _ = socket.write_all(&response);
        });
        let client = build_download_client().unwrap();
        let mut bytes = Vec::new();
        let result = download_url_streaming(
            &client,
            &url,
            &OperationControl::default(),
            maximum_bytes,
            |_| Ok(()),
            |chunk| {
                bytes.extend_from_slice(chunk);
                Ok(())
            },
        );
        server.join().unwrap();
        result.map(|()| bytes)
    }

    #[test]
    fn size_limit_rejects_declared_and_streamed_overflow() {
        let ok = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ndata";
        assert_eq!(download_from(ok.to_vec(), 4).unwrap(), b"data");
        let declared = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nlarge";
        let streamed =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nlarge\r\n0\r\n\r\n";
        for response in [&declared[..], &streamed[..]] {
            let error = download_from(response.to_vec(), 4).unwrap_err();
            assert!(error.to_string().contains("allowed size"), "{error}");
            assert!(!error.is_retryable_download());
        }
    }

    #[test]
    fn error_detail_is_truncated() {
        let mut response = b"HTTP/1.1 503 Unavailable\r\nContent-Length: 2000\r\n\r\n".to_vec();
        response.extend(std::iter::repeat_n(b'x', 2000));
        let error = download_from(response, 4096).unwrap_err();
        assert!(error.is_retryable_download());
        assert!(
            error.to_string().len() < MAX_ERROR_DETAIL_BYTES + 100,
            "{error}"
        );
    }

    #[test]
    fn bounded_reader_stops_past_the_limit() {
        assert_eq!(read_bounded(&b"data"[..], 4, || Ok(())).unwrap(), b"data");
        assert!(read_bounded(&b"large"[..], 4, || Ok(())).is_err());
    }
}
