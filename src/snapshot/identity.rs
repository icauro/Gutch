use std::path::Path;

use crate::manifest::{SnapshotManifest, SnapshotTagSchema, SnapshotTagSet};

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
/// tag-schema defaults and requirements. The schema and inference arguments are
/// retained temporarily for the shared capture call graph, but have no effect.
pub(crate) fn resolve_snapshot_identity(
    _input_path: &Path,
    explicit_tags: &SnapshotTagSet,
    _rules: &SnapshotTagSchema,
    _context: &str,
) -> crate::Result<ResolvedSnapshotIdentity> {
    Ok(ResolvedSnapshotIdentity {
        snapshot_tags: explicit_tags.clone(),
    })
}

/// Compatibility-shaped internal entry point for callers that are being
/// collapsed onto explicit tag assignment. The boolean no longer changes
/// behavior because name-based inference has been removed.
pub(crate) fn resolve_snapshot_identity_with_inference(
    input_path: &Path,
    explicit_tags: &SnapshotTagSet,
    rules: &SnapshotTagSchema,
    _allow_name_inference: bool,
    context: &str,
) -> crate::Result<ResolvedSnapshotIdentity> {
    resolve_snapshot_identity(input_path, explicit_tags, rules, context)
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
