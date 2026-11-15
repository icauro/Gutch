//! Reuse of an existing destination: keeps only files that still match the snapshot.

use std::collections::HashSet;
use std::path::Path;

use crate::Result;
use crate::installation::contracts::SnapshotReconstructSource;
use crate::installation::inspect::{
    InspectionSpans, VerifyMode, VerifyRequest, classify_directory,
};
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::OperationControl;
use crate::runtime::RuntimeResourceHints;

pub(crate) struct VerifiedSource<'a> {
    manifest: SnapshotManifest,
    directory: &'a Path,
}
impl VerifiedSource<'_> {
    pub(crate) fn source(&self) -> SnapshotReconstructSource<'_> {
        SnapshotReconstructSource {
            snapshot_manifest: &self.manifest,
            destination_dir: self.directory,
        }
    }
}

pub(crate) fn resolve_verified_source<'a>(
    source: Option<SnapshotReconstructSource<'a>>,
    runtime_hints: &RuntimeResourceHints,
    control: &OperationControl,
) -> Result<Option<VerifiedSource<'a>>> {
    control.checkpoint()?;
    let Some(source) = source else {
        return Ok(None);
    };
    let (classification, _) = classify_directory(VerifyRequest {
        snapshot_manifest: source.snapshot_manifest,
        dir: source.destination_dir,
        mode: VerifyMode::Fast,
        runtime_hints,
        control: control.clone(),
        spans: &InspectionSpans::VERIFY,
        pool: None,
    })?;
    let invalid: HashSet<_> = classification
        .missing_files
        .iter()
        .chain(classification.corrupt_files.iter())
        .map(|issue| issue.path.as_str())
        .collect();
    let mut manifest = source.snapshot_manifest.clone();
    manifest
        .entries
        .retain(|entry| !invalid.contains(entry.path()));
    manifest.total_logical_size = manifest.files().map(|file| file.size).sum();
    Ok(Some(VerifiedSource {
        manifest,
        directory: source.destination_dir,
    }))
}
