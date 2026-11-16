//! Repository constraints, including the snapshot tag rules new captures must satisfy.

use serde::{Deserialize, Serialize};

pub use crate::filesystem::constraints::FilesystemTarget;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

/// Optional repository invariants; an empty value imposes no constraints.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RepoConstraints {
    #[serde(default)]
    pub targets: Vec<FilesystemTarget>,
    #[serde(default)]
    pub snapshot_tags: SnapshotTagConstraints,
    #[serde(default)]
    pub require_matching_tag_keys_across_snapshots: bool,
}

impl PartialEq for RepoConstraints {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot_tags == other.snapshot_tags
            && self.require_matching_tag_keys_across_snapshots
                == other.require_matching_tag_keys_across_snapshots
            && self.targets.iter().collect::<BTreeSet<_>>()
                == other.targets.iter().collect::<BTreeSet<_>>()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SnapshotTagConstraints {
    #[serde(default, deserialize_with = "deserialize_tag_rules")]
    pub key_value: BTreeMap<String, SnapshotTagRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SnapshotTagRule {
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<SnapshotTagValueFormat>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub enum SnapshotTagValueFormat {
    #[serde(rename = "u32.u32.u32.u32")]
    FourPartVersion,
    #[serde(rename = "yyyy_mm_dd")]
    CalendarDate,
}

impl SnapshotTagValueFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FourPartVersion => "u32.u32.u32.u32",
            Self::CalendarDate => "yyyy_mm_dd",
        }
    }

    pub fn accepts(self, value: &str) -> bool {
        match self {
            Self::FourPartVersion => {
                let parts: Vec<_> = value.split('.').collect();
                parts.len() == 4
                    && parts.iter().all(|part| {
                        !part.is_empty()
                            && (part.len() == 1 || !part.starts_with('0'))
                            && part.bytes().all(|b| b.is_ascii_digit())
                            && part.parse::<u32>().is_ok()
                    })
            }
            Self::CalendarDate => {
                let bytes = value.as_bytes();
                if bytes.len() != 10
                    || bytes[4] != b'_'
                    || bytes[7] != b'_'
                    || !bytes
                        .iter()
                        .enumerate()
                        .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
                {
                    return false;
                }
                // Positions above are ASCII, so these byte slices are safe.
                let year: u32 = value[..4].parse().unwrap();
                let month: u32 = value[5..7].parse().unwrap();
                let day: u32 = value[8..].parse().unwrap();
                let leap = year.is_multiple_of(4)
                    && (!year.is_multiple_of(100) || year.is_multiple_of(400));
                let days = match month {
                    2 => {
                        if leap {
                            29
                        } else {
                            28
                        }
                    }
                    4 | 6 | 9 | 11 => 30,
                    1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
                    _ => return false,
                };
                year != 0 && day >= 1 && day <= days
            }
        }
    }
}

fn deserialize_tag_rules<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, SnapshotTagRule>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let rules = BTreeMap::<String, SnapshotTagRule>::deserialize(deserializer)?;
    validate_tag_rule_keys(&rules).map_err(serde::de::Error::custom)?;
    Ok(rules)
}

fn validate_tag_rule_keys(rules: &BTreeMap<String, SnapshotTagRule>) -> crate::Result<()> {
    for key in rules.keys() {
        if super::tags::normalize_snapshot_tag_key(key)? != *key {
            return Err(crate::Error::manifest(format!(
                "constraint tag key '{key}' must already use lower snake case"
            )));
        }
    }
    Ok(())
}

impl RepoConstraints {
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
            && self.snapshot_tags.key_value.is_empty()
            && !self.require_matching_tag_keys_across_snapshots
    }

    /// Checks constraints built in code; deserialization already checks stored ones.
    pub fn validate_definition(&self) -> crate::Result<()> {
        validate_tag_rule_keys(&self.snapshot_tags.key_value)
    }

    pub fn validate_snapshot_tags(
        &self,
        tags: &super::SnapshotTagSet,
        snapshot_id: &str,
    ) -> crate::Result<()> {
        let tags = tags.normalized()?;
        let mut violations = Vec::new();
        for (key, rule) in &self.snapshot_tags.key_value {
            match tags.get(key) {
                None if rule.required => violations.push(format!("missing required key '{key}'")),
                Some(value) => {
                    if let Some(format) = rule.format
                        && !format.accepts(value)
                    {
                        violations.push(format!("key '{key}' must match {}", format.as_str()));
                    }
                }
                _ => {}
            }
        }
        if !violations.is_empty() {
            return Err(crate::Error::manifest(format!(
                "snapshot '{snapshot_id}': {}",
                violations.join("; ")
            )));
        }
        Ok(())
    }

    /// Checks every entry path of `manifest` against the filesystem targets.
    pub(crate) fn validate_snapshot_paths(
        &self,
        manifest: &super::SnapshotManifest,
        operation: &str,
    ) -> crate::Result<()> {
        crate::filesystem::constraints::validate_paths(
            &self.targets,
            manifest.entries.iter().map(|entry| entry.path()),
            operation,
            Some(&manifest.snapshot_id),
        )
    }

    /// Checks the tags and entry paths of `manifest`.
    pub(crate) fn validate_snapshot(
        &self,
        manifest: &super::SnapshotManifest,
        operation: &str,
    ) -> crate::Result<()> {
        self.validate_snapshot_tags(&manifest.snapshot_tags, &manifest.snapshot_id)?;
        self.validate_snapshot_paths(manifest, operation)
    }
}

#[cfg(test)]
#[path = "constraints_tests.rs"]
mod tests;
