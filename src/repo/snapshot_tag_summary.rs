use crate::manifest;

#[derive(Debug, Clone)]
pub(crate) struct SnapshotTagSummary {
    pub(crate) version_id: String,
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
        version_id: exclusive_tag_or_empty(snapshot_tags, "version"),
        os: exclusive_tag_or_empty(snapshot_tags, "os"),
        arch: exclusive_tag_or_empty(snapshot_tags, "arch"),
        canonical_tags,
    }
}

/// Formats canonical tags for pre-scan display. This is a tag summary, never
/// a snapshot ID: content-derived identity is only available after scanning.
pub(crate) fn format_snapshot_tag_summary(snapshot_tags: &manifest::SnapshotTagSet) -> String {
    snapshot_tags.to_canonical_strings().join(" ")
}

pub(crate) fn exclusive_tag_or_empty(
    snapshot_tags: &manifest::SnapshotTagSet,
    prefix: &str,
) -> String {
    snapshot_tags
        .get_exclusive(prefix)
        .unwrap_or_default()
        .to_string()
}
