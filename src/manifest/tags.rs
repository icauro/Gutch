use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::template;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct SnapshotTagSet {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scoped: BTreeMap<String, BTreeSet<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub exclusive: BTreeMap<String, String>,
}

impl SnapshotTagSet {
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

    pub fn to_ordered_strings(&self, schema: Option<&SnapshotTagSchema>) -> Vec<String> {
        let mut rendered = Vec::new();
        let mut consumed_scoped = BTreeSet::new();
        let mut consumed_exclusive = BTreeSet::new();

        if let Some(schema) = schema {
            for definition in &schema.snapshot_tags {
                match definition.kind {
                    SnapshotTagKind::Scoped => {
                        if let Some(values) = self.scoped.get(&definition.name) {
                            for value in values {
                                rendered.push(format!("{}:{}", definition.name, value));
                            }
                            consumed_scoped.insert(definition.name.clone());
                        }
                    }
                    SnapshotTagKind::Exclusive => {
                        if let Some(value) = self.exclusive.get(&definition.name) {
                            rendered.push(format!("{}::{}", definition.name, value));
                            consumed_exclusive.insert(definition.name.clone());
                        }
                    }
                }
            }
        }

        for (prefix, values) in self
            .scoped
            .iter()
            .filter(|(prefix, _)| !consumed_scoped.contains(*prefix))
        {
            for value in values {
                rendered.push(format!("{}:{}", prefix, value));
            }
        }
        for (prefix, value) in self
            .exclusive
            .iter()
            .filter(|(prefix, _)| !consumed_exclusive.contains(*prefix))
        {
            rendered.push(format!("{}::{}", prefix, value));
        }

        rendered
    }

    pub fn update_family_key(&self, schema: Option<&SnapshotTagSchema>) -> Vec<String> {
        let is_release_varying = |name: &str| {
            schema
                .and_then(|schema| schema.find_definition(name))
                .is_some_and(|definition| {
                    matches!(
                        definition.inference_handler,
                        InferenceHandler::Version | InferenceHandler::Date
                    )
                })
        };
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

        if let Some((prefix, value)) = trimmed.split_once("::") {
            if prefix.is_empty() || value.is_empty() || value.contains("::") {
                return Err(crate::Error::manifest(format!(
                    "invalid tag '{}'. Use scoped:value or exclusive::value",
                    raw_tag
                )));
            }
            if strict_exclusive
                && let Some(existing_value) = self.exclusive.get(prefix)
                && existing_value != value
            {
                return Err(crate::Error::manifest(format!(
                    "conflicting exclusive tag '{}'. Prefix '{}' already has value '{}'",
                    raw_tag, prefix, existing_value
                )));
            }
            self.upsert_exclusive(prefix.to_string(), value.to_string());
            return Ok(());
        }

        if trimmed.contains(':') {
            let Some((prefix, value)) = trimmed.split_once(':') else {
                return Err(crate::Error::manifest(format!(
                    "invalid tag '{}'. Use scoped:value or exclusive::value",
                    raw_tag
                )));
            };
            if prefix.is_empty() || value.is_empty() || value.contains(':') {
                return Err(crate::Error::manifest(format!(
                    "invalid tag '{}'. Use scoped:value or exclusive::value",
                    raw_tag
                )));
            }
            self.insert_scoped(prefix.to_string(), value.to_string());
            return Ok(());
        }

        Err(crate::Error::manifest(format!(
            "invalid tag '{}'. Use scoped:value or exclusive::value",
            raw_tag
        )))
    }
}

#[cfg(test)]
mod update_family_tests {
    use super::*;

    #[test]
    fn update_family_excludes_version_and_date_handlers_but_keeps_unknown_tags() {
        let schema = SnapshotTagSchema {
            snapshot_tags: vec![
                SnapshotTagDefinition {
                    name: "version".into(),
                    kind: SnapshotTagKind::Exclusive,
                    required: true,
                    inference_prefixes: vec![],
                    values: vec![],
                    default_value: None,
                    inference_handler: InferenceHandler::Version,
                },
                SnapshotTagDefinition {
                    name: "released_at".into(),
                    kind: SnapshotTagKind::Exclusive,
                    required: true,
                    inference_prefixes: vec![],
                    values: vec![],
                    default_value: None,
                    inference_handler: InferenceHandler::Date,
                },
            ],
        };
        let tags = SnapshotTagSet::from_compact_strings(&[
            "version::1.2.0".into(),
            "released_at::2026_07_18".into(),
            "os::windows".into(),
            "tier::supporter".into(),
        ])
        .unwrap();

        assert_eq!(
            tags.update_family_key(Some(&schema)),
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum InferenceHandler {
    #[default]
    Default,
    Version,
    Date,
}

impl InferenceHandler {
    pub fn parse_token(&self, raw: &str) -> Option<String> {
        match self {
            Self::Default => Some(raw.to_string()),
            Self::Version => StructuredVersion::parse(raw)
                .ok()
                .map(|value| value.canonical_string()),
            Self::Date => parse_date_token(raw),
        }
    }
}

fn parse_date_token(raw: &str) -> Option<String> {
    let normalized = raw.replace(['_', '-'], ".");
    let parts: Vec<&str> = normalized.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let year: u32 = parts[0].parse().ok()?;
    let month: u32 = parts[1].parse().ok()?;
    let day: u32 = parts[2].parse().ok()?;
    if !(1900..=2100).contains(&year) {
        return None;
    }
    let date = chrono::NaiveDate::from_ymd_opt(year as i32, month, day)?;
    Some(date.format("%Y_%m_%d").to_string())
}

#[cfg(test)]
mod date_tests {
    use super::parse_date_token;

    #[test]
    fn date_handler_normalizes_supported_separators() {
        assert_eq!(
            parse_date_token("2026.07.14").as_deref(),
            Some("2026_07_14")
        );
        assert_eq!(
            parse_date_token("2026-07-14").as_deref(),
            Some("2026_07_14")
        );
        assert_eq!(
            parse_date_token("2026_07_14").as_deref(),
            Some("2026_07_14")
        );
    }

    #[test]
    fn date_handler_rejects_invalid_calendar_dates() {
        assert_eq!(parse_date_token("2026.02.31"), None);
        assert_eq!(parse_date_token("2026.13.01"), None);
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub struct SnapshotTagValueDefinition {
    pub value: String,
    #[serde(skip)]
    pub aliases: Vec<String>,
}

impl SnapshotTagValueDefinition {
    pub fn matches_token(&self, token: &str) -> bool {
        self.value.eq_ignore_ascii_case(token)
            || self
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(token))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub struct SnapshotTagDefinition {
    #[serde(rename = "tag_name")]
    pub name: String,
    #[serde(rename = "tag_kind")]
    pub kind: SnapshotTagKind,
    #[serde(rename = "is_required", default)]
    pub required: bool,
    #[serde(default)]
    pub inference_prefixes: Vec<String>,
    #[serde(
        rename = "allowed_values",
        default,
        serialize_with = "serialize_allowed_values",
        deserialize_with = "deserialize_allowed_values"
    )]
    pub values: Vec<SnapshotTagValueDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "is_default_inference_handler",
        rename = "inference_strategy"
    )]
    pub inference_handler: InferenceHandler,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub struct SnapshotTagSchema {
    #[serde(rename = "tag_definitions", default)]
    pub snapshot_tags: Vec<SnapshotTagDefinition>,
}

fn is_default_inference_handler(handler: &InferenceHandler) -> bool {
    *handler == InferenceHandler::Default
}

fn serialize_allowed_values<S>(
    values: &[SnapshotTagValueDefinition],
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    values
        .iter()
        .map(|value| value.value.as_str())
        .collect::<Vec<_>>()
        .serialize(serializer)
}

fn deserialize_allowed_values<'de, D>(
    deserializer: D,
) -> std::result::Result<Vec<SnapshotTagValueDefinition>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<String>::deserialize(deserializer)?;
    values
        .into_iter()
        .map(|value| {
            Ok(SnapshotTagValueDefinition {
                value,
                aliases: Vec::new(),
            })
        })
        .collect()
}

impl SnapshotTagSchema {
    pub fn find_definition(&self, name: &str) -> Option<&SnapshotTagDefinition> {
        self.snapshot_tags
            .iter()
            .find(|definition| definition.name == name)
    }
}

impl Default for SnapshotTagSchema {
    fn default() -> Self {
        template::template_schema(template::TaggingRuleTemplate::Default)
    }
}

pub type AutoTaggingRules = SnapshotTagSchema;
