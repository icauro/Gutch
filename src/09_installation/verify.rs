//! Read-only verification of a destination against a stored snapshot.

use crate::Result;
use crate::installation::contracts::{InstallationVerifyRequest, InstallationVerifyResult};
use crate::installation::inspect::{InspectionSpans, VerifyRequest, classify_directory};
use crate::installation::prepare::{LoadedRepoSnapshot, load_repo_snapshot_manifest};
use crate::repo::coordination::read_with_control;

pub fn verify_installation(request: InstallationVerifyRequest) -> Result<InstallationVerifyResult> {
    request.control.checkpoint()?;
    let root = request.repo_root.clone();
    read_with_control(&root, &request.control.clone(), || {
        request.control.checkpoint()?;
        let LoadedRepoSnapshot {
            repo_paths,
            snapshot_manifest,
        } = load_repo_snapshot_manifest(&request.repo_root, &request.snapshot_id)?;
        let (classification, verify_timings) = classify_directory(VerifyRequest {
            snapshot_manifest: &snapshot_manifest,
            dir: &request.destination_dir,
            mode: request.verify_mode,
            runtime_hints: &request.runtime_hints,
            control: request.control,
            spans: &InspectionSpans::VERIFY,
            pool: None,
        })?;

        Ok(InstallationVerifyResult {
            repo_root: repo_paths.repo_root,
            snapshot_manifest,
            classification,
            verify_timings,
        })
    })
}
