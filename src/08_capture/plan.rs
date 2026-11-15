use std::path::Path;

use crate::capture::source_operations::SourceSnapshotOperationCandidate;
use crate::capture::source_operations::build_source_snapshot_ops_candidate_with_modified_at;
use crate::manifest::SnapshotTagSet;
use crate::repo::Result;

use super::contracts::SnapshotCapturePlan;
impl SnapshotCapturePlan {
    pub fn from_candidate(candidate: SourceSnapshotOperationCandidate) -> Self {
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

pub fn plan_snapshot_capture(
    repo_root: &Path,
    source_dir: &Path,
    explicit_tags: &SnapshotTagSet,
) -> Result<SnapshotCapturePlan> {
    plan_snapshot_capture_with_source_modified_at(
        repo_root,
        source_dir,
        explicit_tags,
        String::new(),
    )
}

/// Plans a capture while attributing the planning work to the capture profile.
///
/// The CLI needs to plan before it can ask for confirmation, so this wrapper
/// keeps that real preflight work in the same trace as the subsequent capture.
pub fn plan_snapshot_capture_with_control(
    repo_root: &Path,
    source_dir: &Path,
    explicit_tags: &SnapshotTagSet,
    control: &crate::progress::OperationControl,
) -> Result<SnapshotCapturePlan> {
    let _plan_span = control.profile_span("capture.prepare_source.plan_snapshot");
    plan_snapshot_capture(repo_root, source_dir, explicit_tags)
}

pub(crate) fn plan_snapshot_capture_with_source_modified_at(
    repo_root: &Path,
    source_dir: &Path,
    explicit_tags: &SnapshotTagSet,
    source_modified_at: String,
) -> Result<SnapshotCapturePlan> {
    let (_, repository) = crate::repo::paths::load_gutch_repo_manifest(repo_root)?;
    repository
        .constraints
        .unwrap_or_default()
        .validate_snapshot_tags(explicit_tags, &source_dir.display().to_string())?;

    let candidate = build_source_snapshot_ops_candidate_with_modified_at(
        source_dir,
        explicit_tags,
        source_modified_at,
    )?;
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
