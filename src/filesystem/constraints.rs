//! Portable filename policies. These deliberately do not emulate volume-specific
//! Unicode tables; destination validation remains authoritative for a real volume.
use crate::manifest::constraints::{FilesystemTarget, RepositoryConstraints};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PathViolation {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflicting_path: Option<String>,
    pub target: String,
    pub code: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PathValidationFailure {
    pub operation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    pub violations: Vec<PathViolation>,
}

impl std::fmt::Display for PathValidationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "{} rejected: {} filename compatibility violation(s).",
            self.operation,
            self.violations.len()
        )?;
        if let Some(id) = &self.snapshot_id {
            writeln!(f, "Snapshot: {id}")?;
        }
        for violation in self.violations.iter().take(10) {
            // Debug escaping keeps control/bidi characters from altering terminal output.
            writeln!(
                f,
                "{:?}: {}: {}",
                violation.path, violation.target, violation.reason
            )?;
            if let Some(other) = &violation.conflicting_path {
                writeln!(f, "  Conflicts with {other:?}")?;
            }
        }
        if self.violations.len() > 10 {
            writeln!(
                f,
                "{} more; use --error-json-out for full details.",
                self.violations.len() - 10
            )?;
        }
        write!(
            f,
            "Rename incompatible source files, change repository constraints, or select a compatible destination."
        )
    }
}

pub(crate) fn windows_name_reason(name: &str) -> Option<&'static str> {
    if name.ends_with([' ', '.']) {
        return Some("trailing spaces and dots are not allowed");
    }
    if name.chars().any(|c| c < ' ' || "<>:\"|?*".contains(c)) {
        return Some("name contains a character forbidden by Windows");
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|n| {
            matches!(
                n,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    }) {
        return Some("reserved Windows device name");
    }
    None
}

pub(crate) fn validate_paths<'a>(
    constraints: &RepositoryConstraints,
    paths: impl Iterator<Item = &'a str>,
    operation: &str,
    snapshot_id: Option<&str>,
) -> Result<()> {
    if constraints.targets.is_empty() {
        for path in paths {
            super::paths::validate_manifest_relative_path(path, false)?;
        }
        return Ok(());
    }
    let paths = paths.collect::<BTreeSet<_>>();
    // Mandatory checks are never controlled by optional targets.
    for path in &paths {
        super::paths::validate_manifest_relative_path(path, false)?;
    }
    let mut violations = Vec::new();
    for target in constraints.targets.iter().copied().collect::<BTreeSet<_>>() {
        let uses_windows_naming = matches!(
            target,
            FilesystemTarget::WindowsNtfs | FilesystemTarget::WindowsExfat
        );
        let uses_apple_naming = matches!(
            target,
            FilesystemTarget::MacosApfs
                | FilesystemTarget::MacosApfsCaseSensitive
                | FilesystemTarget::IosApfs
        );
        let fold_case_for_comparison = uses_windows_naming
            || matches!(
                target,
                FilesystemTarget::MacosApfs | FilesystemTarget::LinuxExt4Casefold
            );
        let normalize_unicode_for_comparison =
            uses_apple_naming || matches!(target, FilesystemTarget::LinuxExt4Casefold);
        let mut original_paths_by_comparison_key = BTreeMap::<String, String>::new();
        let mut validated_path_prefixes = BTreeSet::new();
        for path in &paths {
            let mut record_violation =
                |path: &str, conflicting_path: Option<String>, code: &str, reason: &str| {
                    violations.push(PathViolation {
                        path: path.into(),
                        conflicting_path,
                        target: target.as_str().into(),
                        code: code.into(),
                        reason: reason.into(),
                    });
                };
            let path_length = if uses_windows_naming {
                path.encode_utf16().count()
            } else {
                path.len()
            };
            let path_length_limit = if uses_windows_naming {
                32766
            } else if uses_apple_naming {
                1023
            } else {
                4095
            };
            if path_length > path_length_limit {
                record_violation(
                    path,
                    None,
                    "path_too_long",
                    &format!(
                        "relative path exceeds {path_length_limit} {}",
                        if uses_windows_naming {
                            "UTF-16 units"
                        } else {
                            "bytes"
                        }
                    ),
                );
            }
            let mut prefix = String::new();
            for part in path.split('/') {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(part);
                if !validated_path_prefixes.insert(prefix.clone()) {
                    continue;
                }
                let length = if uses_windows_naming {
                    part.encode_utf16().count()
                } else {
                    part.len()
                };
                if length > 255 {
                    record_violation(
                        &prefix,
                        None,
                        "name_too_long",
                        &format!(
                            "filename exceeds 255 {}",
                            if uses_windows_naming {
                                "UTF-16 units"
                            } else {
                                "UTF-8 bytes (portable profile budget)"
                            }
                        ),
                    );
                }
                if uses_windows_naming && let Some(reason) = windows_name_reason(part) {
                    record_violation(&prefix, None, "unsupported_name", reason);
                }
                let comparison_key = if normalize_unicode_for_comparison {
                    prefix.nfd().collect::<String>()
                } else {
                    prefix.clone()
                };
                // Conservative policy: full uppercase may reject more names than a
                // particular volume's simple folding table. Never rewrite names.
                let comparison_key = if fold_case_for_comparison {
                    comparison_key.to_lowercase().to_uppercase()
                } else {
                    comparison_key
                };
                if let Some(conflicting_path) =
                    original_paths_by_comparison_key.get(&comparison_key)
                {
                    if conflicting_path != &prefix {
                        record_violation(
                            &prefix,
                            Some(conflicting_path.clone()),
                            "name_collision",
                            "distinct paths collide under the target's conservative naming rules",
                        );
                    }
                } else {
                    original_paths_by_comparison_key.insert(comparison_key, prefix.clone());
                }
            }
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(Error::PathValidation(PathValidationFailure {
            operation: operation.into(),
            snapshot_id: snapshot_id.map(str::to_owned),
            violations,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn check(target: FilesystemTarget, paths: &[&str]) -> Result<()> {
        validate_paths(
            &RepositoryConstraints {
                targets: vec![target],
                ..Default::default()
            },
            paths.iter().copied(),
            "capture",
            None,
        )
    }
    #[test]
    fn windows_rejects_names_and_directory_aliases_but_linux_accepts_them() {
        let paths = [
            "data/report:final.txt",
            "docs/A",
            "DOCS/B",
            "NUL.txt",
            "trailing.",
        ];
        assert!(check(FilesystemTarget::LinuxExt4, &paths).is_ok());
        let Error::PathValidation(failure) =
            check(FilesystemTarget::WindowsNtfs, &paths).unwrap_err()
        else {
            panic!()
        };
        assert_eq!(failure.violations.len(), 4);
        assert!(
            failure
                .violations
                .iter()
                .any(|v| v.conflicting_path.is_some())
        );
    }
    #[test]
    fn lengths_count_encoding_units_and_unicode_is_preserved() {
        let name = "é".repeat(128);
        assert!(check(FilesystemTarget::LinuxBtrfs, &[&name]).is_err());
        assert!(check(FilesystemTarget::WindowsNtfs, &[&name]).is_ok());
        assert!(check(FilesystemTarget::AndroidF2fs, &["日本語/مرحبا/😀.txt"]).is_ok());
        assert!(check(FilesystemTarget::MacosApfs, &["é", "e\u{301}"]).is_err());
        assert!(check(FilesystemTarget::LinuxExt4, &["é", "e\u{301}"]).is_ok());
        assert!(check(FilesystemTarget::IosApfs, &["A", "a"]).is_ok());
    }
    #[test]
    fn empty_policy_keeps_safety_and_unknown_targets_fail_closed() {
        let a = RepositoryConstraints {
            targets: vec![FilesystemTarget::LinuxExt4, FilesystemTarget::WindowsNtfs],
            ..Default::default()
        };
        let b = RepositoryConstraints {
            targets: vec![
                FilesystemTarget::WindowsNtfs,
                FilesystemTarget::LinuxExt4,
                FilesystemTarget::LinuxExt4,
            ],
            ..Default::default()
        };
        assert_eq!(a, b);

        assert!(
            validate_paths(
                &RepositoryConstraints::default(),
                ["NUL.txt"].into_iter(),
                "capture",
                None
            )
            .is_ok()
        );
        assert!(
            validate_paths(
                &RepositoryConstraints::default(),
                ["../bad"].into_iter(),
                "capture",
                None
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<RepositoryConstraints>(r#"{"targets":["windwos"]}"#).is_err()
        );
    }
}
