use crate::api::SnapshotTagSet;

pub(crate) fn parse_cli_tags(tag_values: &[String]) -> crate::Result<SnapshotTagSet> {
    SnapshotTagSet::from_compact_strings(tag_values)
}

#[cfg(test)]
#[path = "tags_tests.rs"]
mod tests;
