use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::template;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TargetTagSet {
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub plain: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scoped: BTreeMap<String, BTreeSet<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub exclusive: BTreeMap<String, String>,
}

impl TargetTagSet {
    pub fn insert_plain(&mut self, value: impl Into<String>) {
        let value = value.into();
        if !value.is_empty() {
            self.plain.insert(value);
        }
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
        for value in &other.plain {
            self.plain.insert(value.clone());
        }
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
        rendered.extend(self.plain.iter().cloned());
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

    pub fn to_ordered_strings(&self, schema: Option<&TargetTagSchema>) -> Vec<String> {
        let mut rendered = Vec::new();
        let mut consumed_plain = BTreeSet::new();
        let mut consumed_scoped = BTreeSet::new();
        let mut consumed_exclusive = BTreeSet::new();

        if let Some(schema) = schema {
            for definition in &schema.target_tags {
                match definition.kind {
                    TargetTagKind::Plain => {
                        if self.plain.contains(&definition.name) {
                            rendered.push(definition.name.clone());
                            consumed_plain.insert(definition.name.clone());
                        }
                    }
                    TargetTagKind::Scoped => {
                        if let Some(values) = self.scoped.get(&definition.name) {
                            for value in values {
                                rendered.push(format!("{}:{}", definition.name, value));
                            }
                            consumed_scoped.insert(definition.name.clone());
                        }
                    }
                    TargetTagKind::Exclusive => {
                        if let Some(value) = self.exclusive.get(&definition.name) {
                            rendered.push(format!("{}::{}", definition.name, value));
                            consumed_exclusive.insert(definition.name.clone());
                        }
                    }
                }
            }
        }

        for value in self
            .plain
            .iter()
            .filter(|value| !consumed_plain.contains(*value))
        {
            rendered.push(value.clone());
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
                    "invalid tag '{}'. Use plain-tag, scoped:value, or exclusive::value",
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
                    "invalid tag '{}'. Use plain-tag, scoped:value, or exclusive::value",
                    raw_tag
                )));
            };
            if prefix.is_empty() || value.is_empty() || value.contains(':') {
                return Err(crate::Error::manifest(format!(
                    "invalid tag '{}'. Use plain-tag, scoped:value, or exclusive::value",
                    raw_tag
                )));
            }
            self.insert_scoped(prefix.to_string(), value.to_string());
            return Ok(());
        }

        self.insert_plain(trimmed.to_string());
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TargetTagKind {
    Plain,
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
    let normalized = raw.replace('_', ".");
    let parts: Vec<&str> = normalized.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let year: u32 = parts[0].parse().ok()?;
    let month: u32 = parts[1].parse().ok()?;
    let day: u32 = parts[2].parse().ok()?;
    if year < 1900 || year > 2100 || month < 1 || month > 12 || day < 1 || day > 31 {
        return None;
    }
    Some(format!("{:04}.{:02}.{:02}", year, month, day))
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VersionScheme {
    Semver,
    Calver,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredVersion {
    pub scheme: VersionScheme,
    pub major_or_year: u32,
    pub minor_or_month: u32,
    pub patch_or_day: u32,
    pub micro: u32,
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
        if parts.is_empty()
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

        if parsed_parts.len() == 3 {
            if parts[0].len() == 4
                && parsed_parts[1] >= 1
                && parsed_parts[1] <= 12
                && parsed_parts[2] >= 1
                && parsed_parts[2] <= 31
            {
                return Ok(Self {
                    scheme: VersionScheme::Calver,
                    major_or_year: parsed_parts[0],
                    minor_or_month: parsed_parts[1],
                    patch_or_day: parsed_parts[2],
                    micro: 0,
                });
            }

            return Ok(Self {
                scheme: VersionScheme::Semver,
                major_or_year: parsed_parts[0],
                minor_or_month: parsed_parts[1],
                patch_or_day: parsed_parts[2],
                micro: 0,
            });
        }

        if parsed_parts.len() == 4
            && parts[0].len() == 4
            && parsed_parts[1] >= 1
            && parsed_parts[1] <= 12
            && parsed_parts[2] >= 1
            && parsed_parts[2] <= 31
        {
            return Ok(Self {
                scheme: VersionScheme::Calver,
                major_or_year: parsed_parts[0],
                minor_or_month: parsed_parts[1],
                patch_or_day: parsed_parts[2],
                micro: parsed_parts[3],
            });
        }

        Err(crate::Error::manifest(format!("invalid version '{}'", raw)))
    }

    pub fn canonical_string(&self) -> String {
        match self.scheme {
            VersionScheme::Semver => format!(
                "{}.{}.{}",
                self.major_or_year, self.minor_or_month, self.patch_or_day
            ),
            VersionScheme::Calver => {
                if self.micro == 0 {
                    format!(
                        "{:04}.{:02}.{:02}",
                        self.major_or_year, self.minor_or_month, self.patch_or_day
                    )
                } else {
                    format!(
                        "{:04}.{:02}.{:02}.{}",
                        self.major_or_year, self.minor_or_month, self.patch_or_day, self.micro
                    )
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TargetTagValueDefinition {
    pub value: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

impl TargetTagValueDefinition {
    pub fn matches_token(&self, token: &str) -> bool {
        self.value.eq_ignore_ascii_case(token)
            || self
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(token))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TargetTagDefinition {
    pub name: String,
    pub kind: TargetTagKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub inference_prefixes: Vec<String>,
    #[serde(default)]
    pub values: Vec<TargetTagValueDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
    #[serde(default)]
    pub inference_handler: InferenceHandler,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TargetTagSchema {
    #[serde(default)]
    pub target_tags: Vec<TargetTagDefinition>,
}

impl TargetTagSchema {
    pub fn find_definition(&self, name: &str) -> Option<&TargetTagDefinition> {
        self.target_tags
            .iter()
            .find(|definition| definition.name == name)
    }
}

impl Default for TargetTagSchema {
    fn default() -> Self {
        template::template_schema(template::TaggingRuleTemplate::Default)
    }
}

pub type AutoTaggingRules = TargetTagSchema;
