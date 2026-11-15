//! Repository constraints, including the snapshot tag rules new captures must satisfy.

use serde::{Deserialize, Serialize};

pub use crate::filesystem::constraints::FilesystemTarget;

/// Optional repository invariants; an empty value imposes no constraints.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepositoryConstraints {
    #[serde(default)]
    pub targets: Vec<FilesystemTarget>,
    #[serde(default)]
    pub snapshot_tags: SnapshotTagConstraints,
    #[serde(default)]
    pub require_matching_tag_keys_across_snapshots: bool,
}

impl PartialEq for RepositoryConstraints {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot_tags == other.snapshot_tags
            && self.require_matching_tag_keys_across_snapshots
                == other.require_matching_tag_keys_across_snapshots
            && self
                .targets
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                == other
                    .targets
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SnapshotTagConstraints {
    #[serde(default)]
    pub key_value: std::collections::BTreeMap<String, SnapshotTagRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SnapshotTagRule {
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<SnapshotTagValueFormat>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
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

impl RepositoryConstraints {
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
            && self.snapshot_tags.key_value.is_empty()
            && !self.require_matching_tag_keys_across_snapshots
    }

    pub fn validate_definition(&self) -> crate::Result<()> {
        for key in self.snapshot_tags.key_value.keys() {
            if super::tags::normalize_snapshot_tag_key(key)? != *key {
                return Err(crate::Error::manifest(format!(
                    "constraint tag key '{key}' must already use lower snake case"
                )));
            }
        }
        Ok(())
    }

    pub fn validate_snapshot_tags(
        &self,
        tags: &super::SnapshotTagSet,
        snapshot_id: &str,
    ) -> crate::Result<()> {
        self.validate_definition()?;
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
}

#[cfg(test)]
mod tag_rule_tests {
    use super::*;

    #[test]
    fn formats_check_numeric_bounds_and_real_calendar_dates() {
        for value in ["0.0.0.0", "4294967295.1.2.3"] {
            assert!(SnapshotTagValueFormat::FourPartVersion.accepts(value));
        }
        for value in [
            "1.2.3",
            "v1.2.3.4",
            "01.2.3.4",
            "4294967296.0.0.0",
            "+1.2.3.4",
        ] {
            assert!(
                !SnapshotTagValueFormat::FourPartVersion.accepts(value),
                "{value}"
            );
        }
        for value in ["0001_01_01", "2000_02_29", "9999_12_31"] {
            assert!(SnapshotTagValueFormat::CalendarDate.accepts(value));
        }
        for value in [
            "0000_01_01",
            "1900_02_29",
            "2026_04_31",
            "2026_1_01",
            "2026_00_01",
            "2026_01_00",
            "日本語",
        ] {
            assert!(
                !SnapshotTagValueFormat::CalendarDate.accepts(value),
                "{value}"
            );
        }
    }

    #[test]
    fn optional_rules_validate_only_present_values_and_reject_unknown_formats() {
        let constraints: RepositoryConstraints = serde_json::from_str(
            r#"{"snapshot_tags":{"key_value":{"build":{"format":"u32.u32.u32.u32"}}}}"#,
        )
        .unwrap();
        assert!(
            constraints
                .validate_snapshot_tags(&super::super::SnapshotTagSet::default(), "empty")
                .is_ok()
        );
        assert!(
            serde_json::from_str::<RepositoryConstraints>(
                r#"{"snapshot_tags":{"key_value":{"build":{"format":"regex"}}}}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<RepositoryConstraints>(
                r#"{"snapshot_tags":{"key_value":{"build":{"enabled":true}}}}"#
            )
            .is_err()
        );
    }
}
