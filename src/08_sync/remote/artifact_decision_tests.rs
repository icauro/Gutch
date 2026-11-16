use super::*;
use crate::sync::remote::PullConflictPolicy;

#[test]
fn remote_artifact_hashes_are_verified_before_persistence() {
    let bytes = b"remote artifact";
    let expected = remote_artifact_hash(bytes);
    assert!(verify_remote_artifact(bytes, &expected, "metadata").is_ok());
    assert!(verify_remote_artifact(b"tampered", &expected, "metadata").is_err());
    assert!(verify_remote_artifact(bytes, "", "metadata").is_err());
}

#[test]
fn empty_remote_hashes_do_not_force_a_change() {
    assert!(!hashes_differ("", ""));
    assert!(hashes_differ("abc", ""));
    assert!(hashes_differ("", "abc"));
    assert!(!hashes_differ("abc", "abc"));
}

fn mtime_ago(seconds: u64) -> std::time::SystemTime {
    std::time::SystemTime::now() - std::time::Duration::from_secs(seconds)
}

#[test]
fn identical_hashes_are_never_fetched() {
    for policy in [
        PullConflictPolicy::KeepLocal,
        PullConflictPolicy::KeepRemote,
        PullConflictPolicy::KeepNewer,
    ] {
        let decision = decide_artifact(
            true,
            "same".to_string(),
            "same",
            "2026-08-08T10:00:00Z",
            Some(mtime_ago(100)),
            policy,
        );
        assert_eq!(decision, ArtifactDecision::Identical);
    }
}

#[test]
fn keep_local_preserves_existing_content_but_fetches_missing_content() {
    let existing = decide_artifact(
        true,
        "local".to_string(),
        "remote",
        "2026-08-08T10:00:00Z",
        Some(mtime_ago(100)),
        PullConflictPolicy::KeepLocal,
    );
    assert_eq!(existing, ArtifactDecision::KeepLocal);

    let missing = decide_artifact(
        false,
        String::new(),
        "remote",
        "2026-08-08T10:00:00Z",
        None,
        PullConflictPolicy::KeepLocal,
    );
    assert_eq!(missing, ArtifactDecision::Fetch);
}

#[test]
fn keep_remote_fetches_even_when_local_is_newer() {
    let decision = decide_artifact(
        true,
        "local".to_string(),
        "remote",
        "2026-08-08T10:00:00Z",
        Some(mtime_ago(100)),
        PullConflictPolicy::KeepRemote,
    );
    assert_eq!(decision, ArtifactDecision::Fetch);
}

#[test]
fn keep_newer_uses_remote_updated_at() {
    let local_newer = decide_artifact(
        true,
        "local".to_string(),
        "remote",
        "2026-08-01T10:00:00Z",
        Some(mtime_ago(100)),
        PullConflictPolicy::KeepNewer,
    );
    assert_eq!(local_newer, ArtifactDecision::KeepLocal);

    let remote_newer = decide_artifact(
        true,
        "local".to_string(),
        "remote",
        "2030-08-01T10:00:00Z",
        Some(mtime_ago(100)),
        PullConflictPolicy::KeepNewer,
    );
    assert_eq!(remote_newer, ArtifactDecision::Fetch);
}

#[test]
fn unparseable_remote_timestamp_fetches() {
    let decision = decide_artifact(
        true,
        "local".to_string(),
        "remote",
        "not-a-timestamp",
        Some(mtime_ago(100)),
        PullConflictPolicy::KeepNewer,
    );
    assert_eq!(decision, ArtifactDecision::Fetch);
}
