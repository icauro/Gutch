use serde::{Deserialize, Serialize};

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetManifest {
    pub manifest_version: String,
    #[serde(default, skip_serializing)]
    pub target_id: String,
    pub inserted_at: String,
    /// Max source file mtime (UTC) recorded when this target was last synced from a source folder.
    #[serde(default)]
    pub source_modified_at: String,
    pub total_logical_size: u64,
    pub total_compressed_size_estimate: u64,
    #[serde(
        default,
        serialize_with = "serialize_target_tags_as_compact_strings",
        deserialize_with = "deserialize_target_tags_from_compact_strings"
    )]
    pub target_tags: TargetTagSet,
    pub files: Vec<FileManifest>,
}

impl TargetManifest {
    pub fn new(target_id: String, target_tags: TargetTagSet) -> Self {
        TargetManifest {
            manifest_version: FORMAT_VERSION.to_string(),
            target_id,
            inserted_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            source_modified_at: String::new(),
            total_logical_size: 0,
            total_compressed_size_estimate: 0,
            target_tags,
            files: Vec::new(),
        }
    }
}

pub(crate) fn serialize_target_tags_as_compact_strings<S>(
    target_tags: &TargetTagSet,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    target_tags.to_canonical_strings().serialize(serializer)
}

pub(crate) fn deserialize_target_tags_from_compact_strings<'de, D>(
    deserializer: D,
) -> std::result::Result<TargetTagSet, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<String>::deserialize(deserializer)?;
    TargetTagSet::from_compact_strings(&values).map_err(serde::de::Error::custom)
}

#[cfg(test)]
#[path = "target_tests.rs"]
mod tests;
