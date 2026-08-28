use crate::manifest;

#[derive(Debug, Clone)]
pub struct SnapshotTagSummary {
    pub version_id: String,
    pub os: String,
    pub arch: String,
    pub canonical_tags: Vec<String>,
}

pub fn summarize_snapshot_tags(
    snapshot_tags: &manifest::SnapshotTagSet,
    _snapshot_tag_schema: Option<&manifest::SnapshotTagSchema>,
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

pub(crate) fn preview_snapshot_identity(
    snapshot_tags: &manifest::SnapshotTagSet,
    snapshot_tag_schema: Option<&manifest::SnapshotTagSchema>,
) -> String {
    snapshot_tags
        .to_ordered_strings(snapshot_tag_schema)
        .join(" ")
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
