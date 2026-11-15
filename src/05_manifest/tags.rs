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
mod tests {
    use super::*;

    #[test]
    fn text_and_json_preserve_unicode_and_separators() {
        let text = SnapshotTagSet::from_compact_strings(&[" Label = 日本語:one=two,three ".into()])
            .unwrap();
        let json: SnapshotTagSet =
            serde_json::from_str(r#"{"key_value":{"Label":" 日本語:one=two,three "}}"#).unwrap();
        assert_eq!(text, json);
        assert_eq!(text.get("label"), Some("日本語:one=two,three"));
        assert_eq!(
            serde_json::to_string(&SnapshotTagSet::default()).unwrap(),
            r#"{"key_value":{}}"#
        );
    }

    #[test]
    fn rejects_conflicts_and_old_shapes_without_silent_data_loss() {
        for input in [
            "key:value",
            "key::value",
            "key+=value",
            "key=",
            "1key=value",
            "key=early-access",
        ] {
            assert!(
                SnapshotTagSet::from_compact_strings(&[input.into()]).is_err(),
                "{input}"
            );
        }
        for input in [
            r#"{"scoped":{}}"#,
            r#"{"exclusive":{}}"#,
            r#"{"key_value":{"a":"one","a":"two"}}"#,
            r#"{"key_value":{"A":"one","a":"two"}}"#,
        ] {
            assert!(
                serde_json::from_str::<SnapshotTagSet>(input).is_err(),
                "{input}"
            );
        }
        assert!(
            SnapshotTagSet::from_compact_strings(&["key=one".into(), "key=one".into()]).is_ok()
        );
        assert!(
            SnapshotTagSet::from_compact_strings(&["key=one".into(), "key=two".into()]).is_err()
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub revision: u32,
}

impl StructuredVersion {
    pub fn parse(raw: &str) -> crate::Result<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(crate::Error::manifest("version cannot be empty"));
        }

        let candidate = trimmed
            .strip_prefix('v')
            .or_else(|| trimmed.strip_prefix('V'))
            .unwrap_or(trimmed);
        let parts: Vec<&str> = candidate.split('.').collect();
        if parts.len() < 3
            || parts.len() > 4
            || parts.iter().any(|part| {
                part.is_empty() || !part.chars().all(|character| character.is_ascii_digit())
            })
        {
            return Err(crate::Error::manifest(format!("invalid version '{}'", raw)));
        }

        let parsed_parts = parts
            .iter()
            .map(|part| {
                part.parse::<u32>()
                    .map_err(|_| crate::Error::manifest(format!("invalid version '{}'", raw)))
            })
            .collect::<crate::Result<Vec<_>>>()?;

        Ok(Self {
            major: parsed_parts[0],
            minor: parsed_parts[1],
            patch: parsed_parts[2],
            revision: parsed_parts.get(3).copied().unwrap_or(0),
        })
    }

    pub fn canonical_string(&self) -> String {
        if self.revision == 0 {
            format!("{}.{}.{}", self.major, self.minor, self.patch)
        } else {
            format!(
                "{}.{}.{}.{}",
                self.major, self.minor, self.patch, self.revision
            )
        }
    }
}

#[cfg(test)]
mod conformance_tests {
    use super::*;
    #[test]
    fn shared_tag_contract() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/datasets/tag_contract.json")).unwrap();
        for case in cases["assignments"].as_array().unwrap() {
            let input: Vec<String> = serde_json::from_value(case["input"].clone()).unwrap();
            let result = SnapshotTagSet::from_compact_strings(&input);
            if case["expected"].is_null() {
                assert!(result.is_err(), "{case}");
            } else {
                assert_eq!(
                    serde_json::to_value(result.unwrap().key_value).unwrap(),
                    case["expected"],
                    "{case}"
                );
            }
        }
        for case in cases["formats"].as_array().unwrap() {
            let format: crate::manifest::constraints::SnapshotTagValueFormat =
                serde_json::from_value(case["format"].clone()).unwrap();
            assert_eq!(
                format.accepts(case["value"].as_str().unwrap()),
                case["valid"].as_bool().unwrap(),
                "{case}"
            );
        }
    }
}
