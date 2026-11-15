//! Snapshot tag sets and tag-key normalization.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Formatter;

/// A snapshot has one value per key. The named collection is always serialized,
/// including for tagless snapshots, so the wire shape stays explicit.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SnapshotTagSet {
    #[serde(default, deserialize_with = "deserialize_key_value_tags")]
    pub key_value: BTreeMap<String, String>,
}

fn deserialize_key_value_tags<'de, D>(deserializer: D) -> Result<BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct TagMapVisitor;
    impl<'de> serde::de::Visitor<'de> for TagMapVisitor {
        type Value = BTreeMap<String, String>;
        fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
            formatter.write_str("an object mapping tag keys to string values")
        }
        fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
        where
            M: serde::de::MapAccess<'de>,
        {
            let mut tags = SnapshotTagSet::default();
            while let Some((key, value)) = map.next_entry::<String, String>()? {
                tags.insert(&key, &value)
                    .map_err(serde::de::Error::custom)?;
            }
            Ok(tags.key_value)
        }
    }
    // Validate during traversal: collecting into a map first would silently
    // discard conflicting duplicate JSON keys before we could reject them.
    deserializer.deserialize_map(TagMapVisitor)
}

pub(crate) fn normalize_snapshot_tag_key(raw: &str) -> crate::Result<String> {
    let key = raw.trim().to_ascii_lowercase();
    let mut characters = key.bytes();
    if !characters.next().is_some_and(|c| c.is_ascii_lowercase())
        || !characters.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
    {
        return Err(crate::Error::manifest(format!(
            "tag key '{raw}' must use lower snake case"
        )));
    }
    Ok(key)
}

impl SnapshotTagSet {
    pub fn is_empty(&self) -> bool {
        self.key_value.is_empty()
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.key_value.get(key).map(String::as_str)
    }
    pub fn contains_key(&self, key: &str) -> bool {
        self.key_value.contains_key(key)
    }

    /// Adds an assignment, rejecting conflicting duplicates instead of choosing
    /// an arbitrary value. Explicit edits replace the selected snapshot's set.
    pub fn insert(&mut self, key: &str, value: &str) -> crate::Result<()> {
        let key = normalize_snapshot_tag_key(key)?;
        let value = value.trim();
        if value.is_empty() {
            return Err(crate::Error::manifest(format!(
                "tag '{key}' cannot have an empty value"
            )));
        }
        if value.contains('-') {
            return Err(crate::Error::manifest("tag values cannot contain hyphens"));
        }
        if self.get(&key).is_some_and(|existing| existing != value) {
            return Err(crate::Error::manifest(format!(
                "conflicting values for tag key '{key}'"
            )));
        }
        self.key_value.insert(key, value.to_owned());
        Ok(())
    }

    /// Explicit input overrides source-configuration defaults by key.
    pub fn extend_missing_from(&mut self, other: &Self) {
        for (key, value) in &other.key_value {
            self.key_value
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
    }

    pub fn normalized(&self) -> crate::Result<Self> {
        let mut normalized = Self::default();
        for (key, value) in &self.key_value {
            normalized.insert(key, value)?;
        }
        Ok(normalized)
    }

    pub fn to_canonical_strings(&self) -> Vec<String> {
        self.key_value
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect()
    }

    pub fn from_compact_strings(values: &[String]) -> crate::Result<Self> {
        let mut tags = Self::default();
        for raw in values {
            let (key, value) = raw.split_once('=').ok_or_else(|| {
                crate::Error::manifest(format!("invalid snapshot tag '{raw}'; use key=value"))
            })?;
            tags.insert(key, value)?;
        }
        Ok(tags)
    }
}

#[cfg(test)]
#[path = "tags_tests.rs"]
mod tests;
