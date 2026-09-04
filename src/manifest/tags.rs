use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct SnapshotTagSet {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub scoped: BTreeMap<String, BTreeSet<String>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub exclusive: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotTagSetFields {
    #[serde(default)]
    scoped: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    exclusive: BTreeMap<String, String>,
}

impl<'de> Deserialize<'de> for SnapshotTagSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let fields = SnapshotTagSetFields::deserialize(deserializer)?;
        let mut normalized = Self::default();

        for (raw_name, values) in fields.scoped {
            let name = normalize_snapshot_tag_name(&raw_name).map_err(serde::de::Error::custom)?;
            if fields.exclusive.contains_key(&raw_name) || normalized.exclusive.contains_key(&name)
            {
                return Err(serde::de::Error::custom(format!(
                    "snapshot tag prefix '{name}' cannot be used as both scoped and exclusive"
                )));
            }
            for raw_value in values {
                let value = raw_value.trim();
                if value.is_empty() {
                    return Err(serde::de::Error::custom(format!(
                        "snapshot scoped tag '{name}' cannot have an empty value"
                    )));
                }
                normalized.insert_scoped(name.clone(), value.to_string());
            }
        }

        for (raw_name, raw_value) in fields.exclusive {
            let name = normalize_snapshot_tag_name(&raw_name).map_err(serde::de::Error::custom)?;
            let value = raw_value.trim();
            if value.is_empty() {
                return Err(serde::de::Error::custom(format!(
                    "snapshot exclusive tag '{name}' cannot have an empty value"
                )));
            }
            if normalized.scoped.contains_key(&name) {
                return Err(serde::de::Error::custom(format!(
                    "snapshot tag prefix '{name}' cannot be used as both scoped and exclusive"
                )));
            }
            if let Some(existing) = normalized.exclusive.get(&name)
                && existing != value
            {
                return Err(serde::de::Error::custom(format!(
                    "conflicting exclusive snapshot tag prefix '{name}'"
                )));
            }
            normalized.upsert_exclusive(name, value.to_string());
        }

        Ok(normalized)
    }
}

fn normalize_snapshot_tag_name(raw_name: &str) -> crate::Result<String> {
    let name = raw_name.trim().to_ascii_lowercase();
    if !is_valid_snapshot_tag_name(&name) {
        return Err(crate::Error::manifest(format!(
            "snapshot tag prefix '{}' must use lower-snake-case",
            raw_name.trim()
        )));
    }
    Ok(name)
}

impl SnapshotTagSet {
    pub fn is_empty(&self) -> bool {
        self.scoped.is_empty() && self.exclusive.is_empty()
    }

    pub fn insert_scoped(&mut self, prefix: impl Into<String>, value: impl Into<String>) {
        let prefix = prefix.into();
        let value = value.into();
        if prefix.is_empty() || value.is_empty() {
            return;
        }
        self.scoped.entry(prefix).or_default().insert(value);
    }

    pub fn insert_exclusive_if_missing(
        &mut self,
        prefix: impl Into<String>,
        value: impl Into<String>,
    ) {
        let prefix = prefix.into();
        let value = value.into();
        if prefix.is_empty() || value.is_empty() {
            return;
        }
        self.exclusive.entry(prefix).or_insert(value);
    }

    pub fn upsert_exclusive(&mut self, prefix: impl Into<String>, value: impl Into<String>) {
        let prefix = prefix.into();
        let value = value.into();
        if prefix.is_empty() || value.is_empty() {
            return;
        }
        self.exclusive.insert(prefix, value);
    }

    pub fn has_exclusive(&self, prefix: &str) -> bool {
        self.exclusive.contains_key(prefix)
    }

    pub fn get_exclusive(&self, prefix: &str) -> Option<&str> {
        self.exclusive.get(prefix).map(String::as_str)
    }

    pub fn extend_missing_from(&mut self, other: &Self) {
        for (prefix, values) in &other.scoped {
            self.scoped
                .entry(prefix.clone())
                .or_default()
                .extend(values.iter().cloned());
        }
        for (prefix, value) in &other.exclusive {
            self.exclusive
                .entry(prefix.clone())
                .or_insert_with(|| value.clone());
        }
    }

    pub fn to_canonical_strings(&self) -> Vec<String> {
        let mut rendered = Vec::new();
        for (prefix, values) in &self.scoped {
            for value in values {
                rendered.push(format!("{}:{}", prefix, value));
            }
        }
        for (prefix, value) in &self.exclusive {
            rendered.push(format!("{}::{}", prefix, value));
        }
        rendered.sort();
        rendered
    }

    pub fn update_family_key(&self) -> Vec<String> {
        let is_release_varying = |name: &str| matches!(name, "version" | "released_at");
        let mut rendered = Vec::new();
        for (prefix, values) in &self.scoped {
            if is_release_varying(prefix) {
                continue;
            }
            rendered.extend(values.iter().map(|value| format!("{prefix}:{value}")));
        }
        for (prefix, value) in &self.exclusive {
            if !is_release_varying(prefix) {
                rendered.push(format!("{prefix}::{value}"));
            }
        }
        rendered.sort();
        rendered
    }

    pub fn from_compact_strings(tag_values: &[String]) -> crate::Result<Self> {
        let mut parsed_tags = Self::default();
        for tag_value in tag_values {
            parsed_tags.apply_compact_string(tag_value, true)?;
        }
        Ok(parsed_tags)
    }

    pub fn apply_compact_string(
        &mut self,
        raw_tag: &str,
        strict_exclusive: bool,
    ) -> crate::Result<()> {
        let trimmed = raw_tag.trim();
        if trimmed.is_empty() {
            return Err(crate::Error::manifest(
                "invalid empty tag value".to_string(),
            ));
        }

        if let Some((raw_prefix, value)) = trimmed.split_once("::") {
            let prefix = raw_prefix.trim().to_ascii_lowercase();
            if prefix.is_empty() || value.is_empty() || value.contains("::") {
                return Err(crate::Error::manifest(format!(
                    "invalid tag '{}'. Use scoped:value or exclusive::value",
                    raw_tag
                )));
            }
            if !is_valid_snapshot_tag_name(&prefix) {
                return Err(crate::Error::manifest(format!(
                    "invalid tag prefix '{}'. Prefixes must use lower-snake-case",
                    raw_prefix.trim()
                )));
            }
            if self.scoped.contains_key(&prefix) {
                return Err(crate::Error::manifest(format!(
                    "tag prefix '{}' cannot be used as both scoped and exclusive",
                    prefix
                )));
            }
            if strict_exclusive
                && let Some(existing_value) = self.exclusive.get(&prefix)
                && existing_value != value
            {
                return Err(crate::Error::manifest(format!(
                    "conflicting exclusive tag '{}'. Prefix '{}' already has value '{}'",
                    raw_tag, prefix, existing_value
                )));
            }
            self.upsert_exclusive(prefix, value.to_string());
            return Ok(());
        }

        if trimmed.contains(':') {
            let Some((raw_prefix, value)) = trimmed.split_once(':') else {
                return Err(crate::Error::manifest(format!(
                    "invalid tag '{}'. Use scoped:value or exclusive::value",
                    raw_tag
                )));
            };
            let prefix = raw_prefix.trim().to_ascii_lowercase();
            if prefix.is_empty() || value.is_empty() || value.contains(':') {
                return Err(crate::Error::manifest(format!(
                    "invalid tag '{}'. Use scoped:value or exclusive::value",
                    raw_tag
                )));
            }
            if !is_valid_snapshot_tag_name(&prefix) {
                return Err(crate::Error::manifest(format!(
                    "invalid tag prefix '{}'. Prefixes must use lower-snake-case",
                    raw_prefix.trim()
                )));
            }
            if self.exclusive.contains_key(&prefix) {
                return Err(crate::Error::manifest(format!(
                    "tag prefix '{}' cannot be used as both scoped and exclusive",
                    prefix
                )));
            }
            self.insert_scoped(prefix, value.to_string());
            return Ok(());
        }

        Err(crate::Error::manifest(format!(
            "invalid tag '{}'. Use scoped:value or exclusive::value",
            raw_tag
        )))
    }
}

fn is_valid_snapshot_tag_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|character| character.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

#[cfg(test)]
mod update_family_tests {
    use super::*;

    #[test]
    fn update_family_excludes_version_and_date_handlers_but_keeps_unknown_tags() {
        let tags = SnapshotTagSet::from_compact_strings(&[
            "version::1.2.0".into(),
            "released_at::2026-07-18".into(),
            "os::windows".into(),
            "tier::supporter".into(),
        ])
        .unwrap();

        assert_eq!(
            tags.update_family_key(),
            vec!["os::windows", "tier::supporter"]
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotTagKind {
    Scoped,
    Exclusive,
}

#[cfg(test)]
mod deserialization_tests {
    use super::SnapshotTagSet;

    #[test]
    fn structured_tags_normalize_names_and_values() {
        let tags: SnapshotTagSet = serde_json::from_str(
            r#"{"scoped":{"Labels":["alpha", " beta "]},"exclusive":{"VERSION":" 1.2.3 "}}"#,
        )
        .unwrap();

        assert_eq!(
            tags.scoped["labels"]
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        assert_eq!(
            tags.exclusive.get("version").map(String::as_str),
            Some("1.2.3")
        );
    }

    #[test]
    fn structured_tags_reject_unknown_fields_and_mixed_prefixes() {
        assert!(
            serde_json::from_str::<SnapshotTagSet>(r#"{"scoped":{},"exclusive":{},"unknown":{}}"#,)
                .is_err()
        );
        assert!(
            serde_json::from_str::<SnapshotTagSet>(
                r#"{"scoped":{"tier":["supporter"]},"exclusive":{"TIER":"creator"}}"#,
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<SnapshotTagSet>(r#"{"scoped":{"version":["1.2.3"]}}"#,).is_ok()
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
