use super::*;
use std::path::Path;

#[test]
fn only_downloads_marked_retryable_are_retried() {
    assert!(Error::download("connection reset", true).is_retryable_download());
    assert!(!Error::download("response exceeded the allowed size", false).is_retryable_download());
    assert!(!Error::from(std::io::Error::other("disk full")).is_retryable_download());
}

#[test]
fn diagnostic_report_classifies_structured_failures() {
    let path_failure = Error::PathValidation(path_validation::PathValidationFailure {
        operation: "Capture".into(),
        snapshot_id: None,
        violations: Vec::new(),
    });
    assert_eq!(
        path_failure.diagnostic_report()["code"],
        "filename_constraints"
    );
    assert_eq!(
        Error::artifact("truncated").diagnostic_report(),
        serde_json::json!({"code": "operation_failed", "message": "Invalid artifact: truncated"})
    );
}

#[test]
fn contextual_io_preserves_native_error_and_typed_failures() {
    let native = std::io::Error::from_raw_os_error(5);
    let expected_kind = native.kind();
    let error = Error::io_context("opening output", Path::new("output.bin"), native);
    assert!(error.to_string().contains("opening output"));
    assert!(error.to_string().contains("output.bin"));
    assert!(error.to_string().contains("os error 5"));
    let source = std::error::Error::source(&error)
        .unwrap()
        .downcast_ref::<std::io::Error>()
        .unwrap();
    assert_eq!(source.raw_os_error(), Some(5));
    assert_eq!(source.kind(), expected_kind);
    assert!(!error.is_retryable_download());
    assert!(matches!(
        Error::Cancelled.with_io_context("writing", Path::new("x")),
        Error::Cancelled
    ));
}
