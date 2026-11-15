use super::*;

#[test]
fn error_display_bundle() {
    let e = Error::bundle("corrupt data");
    assert_eq!(e.to_string(), "Bundle error: corrupt data");
}

#[test]
fn error_display_manifest() {
    let e = Error::manifest("missing field");
    assert_eq!(e.to_string(), "Manifest error: missing field");
}

#[test]
fn error_display_verify() {
    let e = Error::verify("hash mismatch");
    assert_eq!(e.to_string(), "Verification error: hash mismatch");
}

#[test]
fn error_display_source_snapshot_not_verified() {
    let e = Error::SourceSnapshotNotVerified("missing file".into());
    assert_eq!(
        e.to_string(),
        "Source snapshot destination did not verify: missing file"
    );
}

#[test]
fn error_display_reconstruct() {
    let e = Error::reconstruct("output dir missing");
    assert_eq!(e.to_string(), "Reconstruction error: output dir missing");
}

#[test]
fn error_display_io() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let e = Error::from(io_err);
    assert!(e.to_string().contains("I/O error:"));
}

#[test]
fn error_display_hash_mismatch() {
    let e = Error::HashMismatch {
        expected: "abc".into(),
        actual: "def".into(),
    };
    assert_eq!(e.to_string(), "Hash mismatch: expected abc, got def");
}

#[test]
fn error_from_str() {
    let e = Error::from("test error");
    assert_eq!(e.to_string(), "Bundle error: test error");
}

#[test]
fn error_from_string() {
    let e = Error::from(String::from("owned error"));
    assert_eq!(e.to_string(), "Bundle error: owned error");
}

#[test]
fn error_display_chunk_not_found() {
    let e = Error::ChunkNotFound("abc123".into());
    assert_eq!(e.to_string(), "Chunk not found in any bundle: abc123");
}

#[test]
fn error_display_path_error() {
    let e = Error::PathError {
        path: PathBuf::from("/bad/path"),
        reason: "permission denied".into(),
    };
    assert!(e.to_string().contains("/bad/path"));
    assert!(e.to_string().contains("permission denied"));
}

#[test]
fn error_is_debug() {
    let e = Error::bundle("debug test");
    let debug = format!("{e:?}");
    assert!(debug.contains("Bundle"));
    assert!(debug.contains("debug test"));
}

#[test]
fn contextual_io_preserves_native_error_and_typed_failures() {
    let native = std::io::Error::from_raw_os_error(5);
    let expected_kind = native.kind();
    let error = Error::io_context("opening output", std::path::Path::new("output.bin"), native);
    assert!(error.to_string().contains("opening output"));
    assert!(error.to_string().contains("output.bin"));
    assert!(error.to_string().contains("os error 5"));
    let source = std::error::Error::source(&error)
        .unwrap()
        .downcast_ref::<std::io::Error>()
        .unwrap();
    assert_eq!(source.raw_os_error(), Some(5));
    assert_eq!(source.kind(), expected_kind);
    assert!(error.is_retryable_download());
    assert!(matches!(
        Error::Cancelled.with_io_context("writing", std::path::Path::new("x")),
        Error::Cancelled
    ));
}
