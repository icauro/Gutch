//! Summaries of the tags across stored snapshots.

use crate::manifest;

#[derive(Debug, Clone)]
pub(crate) struct SnapshotTagSummary {
    pub(crate) version_tag: String,
    pub(crate) os: String,
    pub(crate) arch: String,
    pub(crate) canonical_tags: Vec<String>,
}

pub(crate) fn summarize_snapshot_tags(
    snapshot_tags: &manifest::SnapshotTagSet,
) -> SnapshotTagSummary {
    let mut canonical_tags = snapshot_tags.to_canonical_strings();
    canonical_tags.sort();
    SnapshotTagSummary {
        version_tag: exclusive_tag_or_empty(snapshot_tags, "version"),
        os: exclusive_tag_or_empty(snapshot_tags, "os"),
        arch: exclusive_tag_or_empty(snapshot_tags, "arch"),
        canonical_tags,
    }
}

pub(crate) fn exclusive_tag_or_empty(
    snapshot_tags: &manifest::SnapshotTagSet,
    prefix: &str,
) -> String {
    snapshot_tags.get(prefix).unwrap_or_default().to_string()
}
