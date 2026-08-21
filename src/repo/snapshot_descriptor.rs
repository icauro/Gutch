use crate::manifest;

#[derive(Debug, Clone)]
pub(crate) struct TargetTagSummary {
    pub version_id: String,
    pub os: String,
    pub arch: String,
    pub canonical_tags: Vec<String>,
}

pub(crate) fn summarize_target_tags(
    target_tags: &manifest::TargetTagSet,
    _target_schema: Option<&manifest::TargetTagSchema>,
) -> TargetTagSummary {
    let mut canonical_tags = target_tags.to_canonical_strings();
    canonical_tags.sort();
    TargetTagSummary {
        version_id: exclusive_tag_or_empty(target_tags, "version"),
        os: exclusive_tag_or_empty(target_tags, "os"),
        arch: exclusive_tag_or_empty(target_tags, "arch"),
        canonical_tags,
    }
}

pub(crate) fn preview_target_identity(
    target_tags: &manifest::TargetTagSet,
    target_schema: Option<&manifest::TargetTagSchema>,
) -> String {
    target_tags.to_ordered_strings(target_schema).join(" ")
}

pub(crate) fn exclusive_tag_or_empty(target_tags: &manifest::TargetTagSet, prefix: &str) -> String {
    target_tags
        .get_exclusive(prefix)
        .unwrap_or_default()
        .to_string()
}
