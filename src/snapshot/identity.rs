use std::path::Path;

use crate::manifest::{SnapshotManifest, SnapshotTagSet};

#[derive(Debug, Clone)]
pub(crate) struct ResolvedSnapshotIdentity {
    pub snapshot_tags: SnapshotTagSet,
}

pub(crate) fn parse_cli_tags(tag_values: &[String]) -> crate::Result<SnapshotTagSet> {
    SnapshotTagSet::from_compact_strings(tag_values)
}

/// Resolves a snapshot identity from explicitly assigned tags.
///
/// Gutch deliberately does not infer tags from source names or apply repository
/// tag-schema defaults and requirements.
pub(crate) fn resolve_snapshot_identity(
    _input_path: &Path,
    explicit_tags: &SnapshotTagSet,
    _context: &str,
) -> crate::Result<ResolvedSnapshotIdentity> {
    Ok(ResolvedSnapshotIdentity {
        snapshot_tags: explicit_tags.clone(),
    })
}

pub(crate) fn apply_resolved_snapshot_identity(
    snapshot_manifest: &mut SnapshotManifest,
    resolved_identity: &ResolvedSnapshotIdentity,
) {
    snapshot_manifest.snapshot_tags = resolved_identity.snapshot_tags.clone();
}

#[cfg(test)]
#[path = "../core/identity_tests.rs"]
mod tests;
