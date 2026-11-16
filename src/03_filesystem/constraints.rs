//! Portable filename policies. These deliberately do not emulate volume-specific
//! Unicode tables; destination validation remains authoritative for a real volume.
use crate::error::path_validation::{PathValidationFailure, PathViolation, ViolationCode};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use unicode_normalization::UnicodeNormalization;

/// Longest Windows path in UTF-16 code units, without the `\\?\` prefix.
pub(crate) const WINDOWS_MAX_PATH_UNITS: usize = 32766;
/// Longest Apple (APFS) path in bytes.
pub(crate) const APPLE_MAX_PATH_BYTES: usize = 1023;
/// Longest Linux and Android path in bytes.
pub(crate) const UNIX_MAX_PATH_BYTES: usize = 4095;
/// Longest single name: UTF-16 units on Windows, bytes elsewhere.
pub(crate) const MAX_NAME_LENGTH: usize = 255;

/// Conservative case folding: full uppercase after lowercase may join more
/// names than a particular volume's folding table does. Never rewrites names.
pub(crate) fn fold_case(key: &str) -> String {
    key.to_lowercase().to_uppercase()
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

/// Destination filesystem whose naming rules a repository may opt into.
///
/// `as_str` is the one spelling: serde and the CLI both use it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum FilesystemTarget {
    WindowsNtfs,
    WindowsExfat,
    LinuxExt4,
    LinuxExt4Casefold,
    LinuxBtrfs,
    MacosApfs,
    MacosApfsCaseSensitive,
    IosApfs,
    AndroidExt4,
    AndroidF2fs,
}

impl FilesystemTarget {
    pub const ALL: [Self; 10] = [
        Self::WindowsNtfs,
        Self::WindowsExfat,
        Self::LinuxExt4,
        Self::LinuxExt4Casefold,
        Self::LinuxBtrfs,
        Self::MacosApfs,
        Self::MacosApfsCaseSensitive,
        Self::IosApfs,
        Self::AndroidExt4,
        Self::AndroidF2fs,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::WindowsNtfs => "windows-ntfs",
            Self::WindowsExfat => "windows-exfat",
            Self::LinuxExt4 => "linux-ext4",
            Self::LinuxExt4Casefold => "linux-ext4-casefold",
            Self::LinuxBtrfs => "linux-btrfs",
            Self::MacosApfs => "macos-apfs",
            Self::MacosApfsCaseSensitive => "macos-apfs-case-sensitive",
            Self::IosApfs => "ios-apfs",
            Self::AndroidExt4 => "android-ext4",
            Self::AndroidF2fs => "android-f2fs",
        }
    }
}

pub(crate) fn validate_paths<'a>(
    targets: &[FilesystemTarget],
    paths: impl Iterator<Item = &'a str>,
    operation: &str,
    snapshot_id: Option<&str>,
) -> Result<()> {
    if targets.is_empty() {
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
    for target in targets.iter().copied().collect::<BTreeSet<_>>() {
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
                |path: &str, conflicting_path: Option<String>, code, reason: &str| {
                    violations.push(PathViolation {
                        path: path.into(),
                        conflicting_path,
                        target: target.as_str().into(),
                        code,
                        reason: reason.into(),
                    });
                };
            let path_length = if uses_windows_naming {
                path.encode_utf16().count()
            } else {
                path.len()
            };
            let path_length_limit = if uses_windows_naming {
                WINDOWS_MAX_PATH_UNITS
            } else if uses_apple_naming {
                APPLE_MAX_PATH_BYTES
            } else {
                UNIX_MAX_PATH_BYTES
            };
            if path_length > path_length_limit {
                record_violation(
                    path,
                    None,
                    ViolationCode::PathTooLong,
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
                if length > MAX_NAME_LENGTH {
                    record_violation(
                        &prefix,
                        None,
                        ViolationCode::NameTooLong,
                        &format!(
                            "filename exceeds {MAX_NAME_LENGTH} {}",
                            if uses_windows_naming {
                                "UTF-16 units"
                            } else {
                                "UTF-8 bytes (portable profile budget)"
                            }
                        ),
                    );
                }
                if uses_windows_naming && let Some(reason) = windows_name_reason(part) {
                    record_violation(&prefix, None, ViolationCode::UnsupportedName, reason);
                }
                let comparison_key = if normalize_unicode_for_comparison {
                    prefix.nfd().collect::<String>()
                } else {
                    prefix.clone()
                };
                let comparison_key = if fold_case_for_comparison {
                    fold_case(&comparison_key)
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
                            ViolationCode::NameCollision,
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
        validate_paths(&[target], paths.iter().copied(), "capture", None)
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
    fn empty_policy_keeps_mandatory_path_safety() {
        assert!(validate_paths(&[], ["NUL.txt"].into_iter(), "capture", None).is_ok());
        assert!(validate_paths(&[], ["../bad"].into_iter(), "capture", None).is_err());
    }
    #[test]
    fn serde_uses_the_as_str_spelling() {
        for target in FilesystemTarget::ALL {
            assert_eq!(
                serde_json::to_value(target).unwrap(),
                serde_json::Value::from(target.as_str())
            );
        }
    }
}
