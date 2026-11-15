use super::*;
use std::io::Write;
use std::net::TcpListener;
use std::sync::mpsc;

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
    let cancel = Arc::new(crate::test_util::TestCancellation::default());
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
    cancel.cancel();
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
    let streamed = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nlarge\r\n0\r\n\r\n";
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
