//! Whether a pull keeps, fetches or skips one remote artifact.

use super::provider::PullConflictPolicy;
use crate::Error;
use crate::Result;
use crate::artifact::hash::hash_hex;
use std::fs;
use std::path::Path;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArtifactDecision {
    Identical,
    Fetch,
    /// The local copy differs and the conflict policy keeps it.
    KeepLocal,
}

/// The hash a provider reports for a stored remote file (`RemoteRepo` and
/// `RemoteSnapshot` `*_hash_b3` fields): lowercase BLAKE3 hex of its bytes.
pub fn remote_artifact_hash(bytes: &[u8]) -> String {
    hash_hex(&blake3::hash(bytes))
}

pub(super) fn verify_remote_artifact(bytes: &[u8], expected_hash: &str, label: &str) -> Result<()> {
    let expected_hash = expected_hash.trim();
    // Without a recorded hash the download cannot be verified.
    if expected_hash.is_empty() {
        return Err(Error::repo(format!(
            "remote did not record a hash for the {label}"
        )));
    }
    let actual_hash = remote_artifact_hash(bytes);
    if actual_hash != expected_hash {
        return Err(Error::HashMismatch {
            expected: format!("{label} {expected_hash}"),
            actual: format!("{label} {actual_hash}"),
        });
    }
    Ok(())
}

pub(super) fn hashes_differ(local_hash: &str, remote_hash: &str) -> bool {
    let local_hash = local_hash.trim();
    let remote_hash = remote_hash.trim();
    if local_hash.is_empty() && remote_hash.is_empty() {
        return false;
    }
    remote_hash.is_empty() || local_hash != remote_hash
}

pub(super) fn local_file_mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

fn remote_updated_at_is_newer_or_equal(
    remote_updated_at: &str,
    local_modified: Option<SystemTime>,
) -> bool {
    let Ok(remote) = chrono::DateTime::parse_from_rfc3339(remote_updated_at) else {
        return true;
    };
    let remote_utc = remote.with_timezone(&chrono::Utc);
    let Some(local_modified) = local_modified else {
        return true;
    };
    SystemTime::from(remote_utc) >= local_modified
}

pub(super) fn decide_artifact(
    local_exists: bool,
    local_hash: String,
    remote_hash: &str,
    remote_updated_at: &str,
    local_modified: Option<SystemTime>,
    policy: PullConflictPolicy,
) -> ArtifactDecision {
    if !local_exists || hashes_differ(&local_hash, remote_hash) {
        match policy {
            PullConflictPolicy::KeepLocal => {
                if local_exists {
                    ArtifactDecision::KeepLocal
                } else {
                    ArtifactDecision::Fetch
                }
            }
            PullConflictPolicy::KeepRemote => ArtifactDecision::Fetch,
            PullConflictPolicy::KeepNewer => {
                if remote_updated_at_is_newer_or_equal(remote_updated_at, local_modified) {
                    ArtifactDecision::Fetch
                } else {
                    ArtifactDecision::KeepLocal
                }
            }
        }
    } else {
        ArtifactDecision::Identical
    }
}

#[cfg(test)]
#[path = "artifact_decision_tests.rs"]
mod tests;
