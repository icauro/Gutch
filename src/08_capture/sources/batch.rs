//! Plans and captures a batch of sources.

use crate::Result;
use crate::artifact::profiles::BundleSizingProfile;
use crate::artifact::profiles::ChunkSizingProfile;
use crate::artifact::profiles::validate_packaging_sizes;
use crate::capture::contracts::SnapshotCapturePlan;
use crate::capture::contracts::SnapshotCaptureRequest;
use crate::capture::contracts::SnapshotCaptureResult;
use crate::capture::execute::capture_planned_snapshots;
use crate::capture::plan::plan_capture_source;

pub fn plan_capture_snapshots(
    requests: &[SnapshotCaptureRequest],
) -> Result<Vec<SnapshotCapturePlan>> {
    requests
        .iter()
        .map(|request| {
            validate_packaging_sizes(
                &ChunkSizingProfile::for_preset(request.chunk_profile),
                &BundleSizingProfile::for_preset(request.bundle_profile),
                request.storage_mode,
            )?;
            plan_capture_source(
                &request.repo_root,
                &request.source,
                &request.tags,
                &request.control,
            )
        })
        .collect()
}

pub fn plan_capture_snapshot(request: &SnapshotCaptureRequest) -> Result<SnapshotCapturePlan> {
    let mut plans = plan_capture_snapshots(std::slice::from_ref(request))?;
    Ok(plans.remove(0))
}

pub fn capture_snapshot(request: &SnapshotCaptureRequest) -> Result<SnapshotCaptureResult> {
    let mut results = capture_snapshots(std::slice::from_ref(request))?;
    Ok(results.remove(0))
}

/// Validate every source first. Successful captures remain committed if a later capture fails.
pub fn capture_snapshots(
    requests: &[SnapshotCaptureRequest],
) -> Result<Vec<SnapshotCaptureResult>> {
    plan_capture_snapshots(requests)?;
    capture_planned_snapshots(requests)
}
