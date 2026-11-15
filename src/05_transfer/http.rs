//! HTTP client construction and bounded streaming downloads.

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use crate::Error;
use crate::Result;
use crate::progress::OperationControl;

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

pub(crate) fn download_url_streaming<F, C>(
    client: &DownloadClient,
    url: &str,
    control: &OperationControl,
    maximum_bytes: Option<u64>,
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
            let mut response = client.client.get(url).send().await.map_err(|error| {
                Error::download(format!("download request failed: {error}"), true)
            })?;
            if !response.status().is_success() {
                let status = response.status();
                let mut body = Vec::new();
                while body.len() < 16 * 1024 {
                    control.checkpoint()?;
                    let Ok(Some(chunk)) = response.chunk().await else {
                        break;
                    };
                    let length = chunk.len().min(16 * 1024 - body.len());
                    body.extend_from_slice(&chunk[..length]);
                }
                return Err(Error::download(
                    format_http_failure(status, &String::from_utf8_lossy(&body)),
                    status.is_server_error()
                        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
                        || status == reqwest::StatusCode::REQUEST_TIMEOUT,
                ));
            }
            let total_bytes = response.content_length();
            if maximum_bytes.is_some_and(|maximum| total_bytes.is_some_and(|total| total > maximum))
            {
                return Err(Error::download("download exceeded the allowed size", false));
            }
            on_content_length(total_bytes)?;
            let mut transferred_bytes = 0_u64;
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| Error::download(format!("download body failed: {error}"), true))?
            {
                transferred_bytes = transferred_bytes.saturating_add(chunk.len() as u64);
                if maximum_bytes.is_some_and(|maximum| transferred_bytes > maximum) {
                    return Err(Error::download("response exceeded the allowed size", false));
                }
                for bytes in chunk.chunks(64 * 1024) {
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
                    tokio::time::sleep(Duration::from_millis(25)).await;
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

pub(crate) fn read_bounded_chunks<R, F, C>(
    mut reader: R,
    maximum_bytes: Option<u64>,
    mut checkpoint: C,
    mut on_chunk: F,
) -> Result<u64>
where
    R: Read,
    F: FnMut(&[u8]) -> Result<()>,
    C: FnMut() -> Result<()>,
{
    checkpoint()?;
    let mut buffer = [0_u8; 64 * 1024];
    let mut transferred_bytes = 0_u64;
    loop {
        checkpoint()?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        transferred_bytes = transferred_bytes.saturating_add(read as u64);
        if maximum_bytes.is_some_and(|maximum| transferred_bytes > maximum) {
            return Err(Error::download("response exceeded the allowed size", false));
        }
        on_chunk(&buffer[..read])?;
    }
    checkpoint()?;
    Ok(transferred_bytes)
}

pub fn read_bounded_response<C>(
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
        return Err(Error::download("response exceeded the allowed size", false));
    }
    let mut bytes = Vec::new();
    read_bounded_chunks(&mut response, Some(maximum_bytes), checkpoint, |chunk| {
        bytes.extend_from_slice(chunk);
        Ok(())
    })?;
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
                    Some(1024),
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
}
