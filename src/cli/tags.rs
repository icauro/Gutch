use std::path::Path;

use crate::manifest::SnapshotManifest;
use crate::manifest::SnapshotTagSet;

#[derive(Debug, Clone)]
pub(crate) struct ResolvedSnapshotTags {
    pub snapshot_tags: SnapshotTagSet,
}

pub(crate) fn parse_cli_tags(tag_values: &[String]) -> crate::Result<SnapshotTagSet> {
    SnapshotTagSet::from_compact_strings(tag_values)
}

/// Resolves explicitly assigned snapshot tags.
///
/// Gutch deliberately does not infer tags from source names or apply repository
/// tag-schema defaults. The backend owns the remote version-tag rule. Tags are
/// descriptive metadata, not snapshot identity: the content-derived snapshot ID
/// is only available after the source scan.
pub(crate) fn resolve_snapshot_tags(
    _input_path: &Path,
    explicit_tags: &SnapshotTagSet,
    _context: &str,
) -> crate::Result<ResolvedSnapshotTags> {
    Ok(ResolvedSnapshotTags {
        snapshot_tags: SnapshotTagSet::from_compact_strings(&explicit_tags.to_canonical_strings())?,
    })
}

pub(crate) fn apply_resolved_snapshot_tags(
    snapshot_manifest: &mut SnapshotManifest,
    resolved_tags: &ResolvedSnapshotTags,
) {
    snapshot_manifest.snapshot_tags = resolved_tags.snapshot_tags.clone();
}

#[cfg(test)]
#[path = "tags_tests.rs"]
mod tests;
