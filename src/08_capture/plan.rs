//! Plans a single capture and describes it before execution.

use std::path::Path;

use crate::manifest::SnapshotTagSet;
use crate::repo::Result;

use crate::capture::contracts::SnapshotCapturePlan;
use crate::progress::OperationControl;
use crate::repo::paths::load_repo_manifest;
use crate::repo::summarize_snapshot_tags;

/// Plans a capture of `source_dir` into the repository at `repo_root`.
///
/// The CLI plans before it asks for confirmation, so the planning work is
/// recorded in the same profile as the capture that follows.
pub(crate) fn plan_capture_source(
    repo_root: &Path,
    source_dir: &Path,
    explicit_tags: &SnapshotTagSet,
    control: &OperationControl,
) -> Result<SnapshotCapturePlan> {
    let _plan_span = control.profile_span("capture.prepare_source.plan_snapshot");
    let (_, repository) = load_repo_manifest(repo_root)?;
    repository
        .constraints
        .unwrap_or_default()
        .validate_snapshot_tags(explicit_tags, &source_dir.display().to_string())?;

    let snapshot_tags = explicit_tags.normalized()?;
    let folder_name = source_dir
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            crate::Error::invalid_input(format!(
                "invalid source snapshot path: {}",
                source_dir.display()
            ))
        })?
        .to_string();
    let summary = summarize_snapshot_tags(&snapshot_tags);
    // Capture is additive: equal tags never select replacement targets. A new
    // content-derived snapshot ID always creates a new snapshot.
    Ok(SnapshotCapturePlan {
        folder_name,
        path: source_dir.display().to_string(),
        version_tag: summary.version_tag,
        os: summary.os,
        arch: summary.arch,
        snapshot_tags,
    })
}
