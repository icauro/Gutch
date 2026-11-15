//! Plans a single capture and describes it before execution.

use std::path::Path;

use crate::capture::sources::discovery::SourceSnapshotCandidate;
use crate::capture::sources::discovery::source_snapshot_candidate;
use crate::manifest::SnapshotTagSet;
use crate::repo::Result;

use crate::capture::contracts::SnapshotCapturePlan;
use crate::progress::OperationControl;
use crate::repo::paths::load_gutch_repo_manifest;
impl SnapshotCapturePlan {
    pub fn from_candidate(candidate: SourceSnapshotCandidate) -> Self {
        Self {
            folder_name: candidate.folder_name,
            path: candidate.path,
            version_tag: candidate.version_tag,
            os: candidate.os,
            arch: candidate.arch,
            snapshot_tags: candidate.snapshot_tags,
        }
    }
}

/// Plans a capture of `source_dir` into the repository at `repo_root`.
///
/// The CLI plans before it asks for confirmation, so the planning work is
/// recorded in the same profile as the capture that follows.
pub fn plan_snapshot_capture(
    repo_root: &Path,
    source_dir: &Path,
    explicit_tags: &SnapshotTagSet,
    control: &OperationControl,
) -> Result<SnapshotCapturePlan> {
    let _plan_span = control.profile_span("capture.prepare_source.plan_snapshot");
    let (_, repository) = load_gutch_repo_manifest(repo_root)?;
    repository
        .constraints
        .unwrap_or_default()
        .validate_snapshot_tags(explicit_tags, &source_dir.display().to_string())?;

    // The plan does not report the source modification time, so skip reading it.
    let candidate = source_snapshot_candidate(source_dir, explicit_tags, String::new())?;
    // Capture is additive: equal tags never select replacement targets. A new
    // content-derived snapshot ID always creates a new snapshot.
    Ok(SnapshotCapturePlan::from_candidate(candidate))
}

pub fn describe_snapshot_capture_plan(plan: &SnapshotCapturePlan) -> String {
    format!(
        "Capture new snapshot from {} ({}) with tags {}",
        plan.folder_name,
        plan.version_tag,
        plan.snapshot_tags.to_canonical_strings().join(" ")
    )
}
