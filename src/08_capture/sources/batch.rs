//! Plans and captures a batch of sources.

use crate::Result;
use crate::artifact::profiles::BundleSizingProfile;
use crate::artifact::profiles::ChunkSizingProfile;
use crate::artifact::profiles::validate_packaging_sizes;
use crate::capture::contracts::SnapshotCapturePlan;
use crate::capture::contracts::SnapshotCaptureRequest;
use crate::capture::contracts::SnapshotCaptureResult;
use crate::capture::execute::capture_planned_snapshots;
use crate::capture::plan::plan_snapshot_capture;

pub fn plan_snapshot_captures(
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
            plan_snapshot_capture(
                &request.repo_root,
                &request.source_dir,
                &request.explicit_tags,
                &request.control,
            )
        })
        .collect()
}

/// Validate every source first. Successful captures remain committed if a later capture fails.
pub fn capture_snapshots(
    requests: Vec<SnapshotCaptureRequest>,
) -> Result<Vec<SnapshotCaptureResult>> {
    plan_snapshot_captures(&requests)?;
    capture_planned_snapshots(requests)
}
