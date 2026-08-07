//! Source-root ingestion config (`.gutch_source_config.json`).
//!
//! A source root is the directory passed as `--product-dir` to
//! `target insert-many` or as the sources root to `target-sources sync` —
//! the directory whose (possibly nested) subdirectories are target sources.
//!
//! The config selects an ingestion mode:
//!
//! - `auto` (default): target sources are the immediate subdirectories of the
//!   source root, filtered by `auto_rules.ignore_paths` (plain folder-name
//!   fnmatch). Tags come from folder names plus CLI tags.
//! - `explicit`: target sources are exactly the `explicit_rules.targets`
//!   paths at any depth, with tags declared per entry. Folder names are
//!   irrelevant and name inference is disabled.
//!
//! `.gutch_source_config.json` itself is always ignored: it is a file at the source
//! root, so target discovery (which only picks directories) never sees it,
//! and it sits outside every target source tree.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::manifest::TargetTagSet;
use crate::{Error, Result};

pub const SOURCE_CONFIG_FILE_NAME: &str = ".gutch_source_config.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestionMode {
    #[default]
    Auto,
    Explicit,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct AutoRules {
    /// Plain folder names matched against immediate subdirectory names during
    /// `auto` discovery (simple fnmatch: `*` and `?`; no separators allowed).
    pub ignore_paths: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct ExplicitRules {
    pub targets: Vec<TargetRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct TargetRule {
    /// Relative path from the source root to the target source folder.
    pub path: String,
    /// `key::value` compact tags applied to this target (same format as CLI
    /// `--tag`). CLI `--tag` values still win over these.
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct SourceConfig {
    pub ingestion_mode: IngestionMode,
    pub auto_rules: AutoRules,
    pub explicit_rules: ExplicitRules,
}

/// A target source resolved from the config (or from an explicit `--dir`).
#[derive(Debug, Clone)]
pub struct ResolvedSourceTarget {
    /// Absolute path to the target source directory.
    pub path: PathBuf,
    /// Path relative to the source root (`/`-separated segments).
    pub relative_path: PathBuf,
    /// Tags declared in the config entry (explicit mode); `None` in auto mode
    /// or for explicit `--dir` arguments.
    pub config_tags: Option<TargetTagSet>,
    /// Whether name-based tag inference is allowed for this target.
    pub allow_name_inference: bool,
}

impl SourceConfig {
    pub fn default_skeleton() -> Self {
        Self {
            ingestion_mode: IngestionMode::Auto,
            ..Self::default()
        }
    }

    pub fn load(source_root: &Path) -> Result<Option<Self>> {
        let path = source_root.join(SOURCE_CONFIG_FILE_NAME);
        if !path.is_file() {
            return Ok(None);
        }
        let text = crate::helpers::read_to_string(&path)?;
        let config: Self = crate::helpers::from_json_str(&text)?;
        Ok(Some(config))
    }

    /// Load the config or fall back to the default `auto` skeleton without
    /// creating a file (used by read-only consumers such as repo checks).
    pub fn load_or_default(source_root: &Path) -> Result<Self> {
        Ok(Self::load(source_root)?.unwrap_or_else(Self::default_skeleton))
    }

    /// Load the config, creating the default skeleton on first use.
    pub fn load_or_create(source_root: &Path) -> Result<Self> {
        if let Some(config) = Self::load(source_root)? {
            return Ok(config);
        }
        let config = Self::default_skeleton();
        config.write(source_root)?;
        Ok(config)
    }

    pub fn write(&self, source_root: &Path) -> Result<()> {
        let path = source_root.join(SOURCE_CONFIG_FILE_NAME);
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| Error::manifest(format!("serialize source config: {err}")))?;
        crate::helpers::write(&path, text)
    }

    /// Resolve the target sources for this config under `source_root`.
    ///
    /// Validates the active mode only: `explicit` requires non-empty unique
    /// `targets` whose normalized paths exist as directories; `auto` requires
    /// `ignore_paths` to be plain folder names.
    pub fn discover_target_sources(&self, source_root: &Path) -> Result<Vec<ResolvedSourceTarget>> {
        match self.ingestion_mode {
            IngestionMode::Auto => self.discover_auto(source_root),
            IngestionMode::Explicit => self.discover_explicit(source_root),
        }
    }

    fn discover_auto(&self, source_root: &Path) -> Result<Vec<ResolvedSourceTarget>> {
        for pattern in &self.auto_rules.ignore_paths {
            if pattern.is_empty() || pattern.contains('/') || pattern.contains('\\') {
                return Err(Error::manifest(format!(
                    "auto_rules.ignore_paths entries must be plain folder names, got '{}'",
                    pattern
                )));
            }
        }
        if !source_root.is_dir() {
            return Err(Error::manifest(format!(
                "source root is not a directory: {}",
                source_root.display()
            )));
        }
        let mut targets = Vec::new();
        for entry in std::fs::read_dir(source_root)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
                .ok_or_else(|| {
                    Error::manifest(format!("invalid source folder name: {}", path.display()))
                })?;
            if self.auto_rules.ignore_paths.iter().any(|pattern| fnmatch_name(&name, pattern)) {
                continue;
            }
            targets.push(ResolvedSourceTarget {
                path,
                relative_path: PathBuf::from(&name),
                config_tags: None,
                allow_name_inference: true,
            });
        }
        targets.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        Ok(targets)
    }

    fn discover_explicit(&self, source_root: &Path) -> Result<Vec<ResolvedSourceTarget>> {
        if self.explicit_rules.targets.is_empty() {
            return Err(Error::manifest(
                "explicit ingestion mode requires at least one target in explicit_rules.targets",
            ));
        }
        let mut seen: HashSet<String> = HashSet::new();
        let mut targets = Vec::new();
        for rule in &self.explicit_rules.targets {
            let normalized = normalize_relative_path(&rule.path)?;
            if !seen.insert(normalized.clone()) {
                return Err(Error::manifest(format!(
                    "duplicate explicit target path '{}'",
                    rule.path
                )));
            }
            let tags = TargetTagSet::from_compact_strings(&rule.tags).map_err(|err| {
                Error::manifest(format!(
                    "invalid tags for explicit target '{}': {err}",
                    rule.path
                ))
            })?;
            let relative_path = PathBuf::from(&normalized);
            let path = source_root.join(&relative_path);
            if !path.is_dir() {
                return Err(Error::manifest(format!(
                    "explicit target path does not exist or is not a directory: {}",
                    path.display()
                )));
            }
            targets.push(ResolvedSourceTarget {
                path,
                relative_path,
                config_tags: Some(tags),
                allow_name_inference: false,
            });
        }
        targets.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        Ok(targets)
    }
}

/// Normalize a user-supplied relative path: strip one optional leading `./`,
/// convert backslashes to `/`, collapse duplicate slashes and bare `.`
/// segments. Any `..` segment or an absolute path is an error.
pub fn normalize_relative_path(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "." {
        return Err(Error::manifest("explicit target path must not be empty"));
    }
    let with_forward_slashes = trimmed.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for part in with_forward_slashes.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                return Err(Error::manifest(
                    "explicit target path must not contain '..' segments",
                ));
            }
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err(Error::manifest("explicit target path must not be empty"));
    }
    if with_forward_slashes.starts_with('/') || is_windows_drive_prefix(parts[0]) {
        return Err(Error::manifest(
            "explicit target path must be relative to the source root",
        ));
    }
    Ok(parts.join("/"))
}

fn is_windows_drive_prefix(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Simple fnmatch supporting `*` (any run of characters) and `?` (one
/// character), matched against a plain folder name.
pub fn fnmatch_name(name: &str, pattern: &str) -> bool {
    fn match_here(name: &[char], pattern: &[char]) -> bool {
        if pattern.is_empty() {
            return name.is_empty();
        }
        match pattern[0] {
            '*' => {
                let mut pattern_index = 0;
                while pattern_index < pattern.len() && pattern[pattern_index] == '*' {
                    pattern_index += 1;
                }
                if pattern_index == pattern.len() {
                    return true;
                }
                (0..=name.len()).any(|index| match_here(&name[index..], &pattern[pattern_index..]))
            }
            '?' => !name.is_empty() && match_here(&name[1..], &pattern[1..]),
            expected => !name.is_empty() && name[0] == expected && match_here(&name[1..], &pattern[1..]),
        }
    }
    match_here(&name.chars().collect::<Vec<_>>(), &pattern.chars().collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helpers;

    fn write_config(source_root: &Path, json: &str) {
        helpers::write(&source_root.join(SOURCE_CONFIG_FILE_NAME), json).unwrap();
    }

    #[test]
    fn default_skeleton_is_auto_mode() {
        let config = SourceConfig::default_skeleton();
        assert_eq!(config.ingestion_mode, IngestionMode::Auto);
        assert!(config.auto_rules.ignore_paths.is_empty());
        assert!(config.explicit_rules.targets.is_empty());
    }

    #[test]
    fn load_missing_returns_none() {
        let temp = tempfile::tempdir().unwrap();
        assert!(SourceConfig::load(temp.path()).unwrap().is_none());
    }

    #[test]
    fn load_or_create_writes_skeleton() {
        let temp = tempfile::tempdir().unwrap();
        let config = SourceConfig::load_or_create(temp.path()).unwrap();
        assert_eq!(config.ingestion_mode, IngestionMode::Auto);
        assert!(temp.path().join(SOURCE_CONFIG_FILE_NAME).is_file());
        let reloaded = SourceConfig::load(temp.path()).unwrap().unwrap();
        assert_eq!(reloaded.ingestion_mode, IngestionMode::Auto);
    }

    #[test]
    fn normalize_relative_path_rules() {
        assert_eq!(normalize_relative_path("./a/b").unwrap(), "a/b");
        assert_eq!(normalize_relative_path("a\\b").unwrap(), "a/b");
        assert_eq!(normalize_relative_path("a//b/./c").unwrap(), "a/b/c");
        assert!(normalize_relative_path("../escape").is_err());
        assert!(normalize_relative_path("a/../b").is_err());
        assert!(normalize_relative_path("/abs").is_err());
        assert!(normalize_relative_path("C:\\abs").is_err());
        assert!(normalize_relative_path("").is_err());
        assert!(normalize_relative_path(".").is_err());
    }

    #[test]
    fn fnmatch_supports_star_and_question() {
        assert!(fnmatch_name(".archives", ".archives"));
        assert!(fnmatch_name(".archives", ".*"));
        assert!(fnmatch_name("foo.part", "*.part"));
        assert!(fnmatch_name("a.b", "a?b"));
        assert!(!fnmatch_name("ab", "a?b"));
        assert!(!fnmatch_name("foo", "*.part"));
        assert!(fnmatch_name("anything", "*"));
    }

    #[test]
    fn auto_discovery_filters_ignore_paths_and_skips_files() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("game-win")).unwrap();
        std::fs::create_dir(temp.path().join(".archives")).unwrap();
        std::fs::write(temp.path().join("notes.txt"), b"x").unwrap();
        write_config(
            temp.path(),
            r#"{"ingestion_mode":"auto","auto_rules":{"ignore_paths":[".archives"]}}"#,
        );
        let config = SourceConfig::load(temp.path()).unwrap().unwrap();

        let targets = config.discover_target_sources(temp.path()).unwrap();

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].relative_path, PathBuf::from("game-win"));
        assert!(targets[0].allow_name_inference);
        assert!(targets[0].config_tags.is_none());
    }

    #[test]
    fn explicit_discovery_resolves_nested_paths_with_tags() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("versions").join("4.6-stable").join("linux-x86_64");
        std::fs::create_dir_all(&nested).unwrap();
        write_config(
            temp.path(),
            r#"{"ingestion_mode":"explicit","explicit_rules":{"targets":[{"path":"./versions/4.6-stable/linux-x86_64","tags":["version::4.6-stable","os::linux","arch::x86_64"]}]}}"#,
        );
        let config = SourceConfig::load(temp.path()).unwrap().unwrap();

        let targets = config.discover_target_sources(temp.path()).unwrap();

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].relative_path, PathBuf::from("versions/4.6-stable/linux-x86_64"));
        assert!(!targets[0].allow_name_inference);
        let tags = targets[0].config_tags.as_ref().unwrap();
        assert_eq!(tags.get_exclusive("version").unwrap(), "4.6-stable");
        assert_eq!(tags.get_exclusive("os").unwrap(), "linux");
    }

    #[test]
    fn explicit_discovery_rejects_missing_targets_and_duplicates() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("game-win")).unwrap();
        write_config(
            temp.path(),
            r#"{"ingestion_mode":"explicit","explicit_rules":{"targets":[{"path":"game-win"},{"path":"./game-win"}]}}"#,
        );
        let config = SourceConfig::load(temp.path()).unwrap().unwrap();
        assert!(config.discover_target_sources(temp.path()).unwrap_err().to_string().contains("duplicate"));

        write_config(
            temp.path(),
            r#"{"ingestion_mode":"explicit","explicit_rules":{"targets":[{"path":"missing-dir"}]}}"#,
        );
        let config = SourceConfig::load(temp.path()).unwrap().unwrap();
        assert!(config.discover_target_sources(temp.path()).unwrap_err().to_string().contains("does not exist"));

        write_config(temp.path(), r#"{"ingestion_mode":"explicit","explicit_rules":{"targets":[]}}"#);
        let config = SourceConfig::load(temp.path()).unwrap().unwrap();
        assert!(config.discover_target_sources(temp.path()).unwrap_err().to_string().contains("requires at least one"));
    }

    #[test]
    fn auto_discovery_rejects_separators_in_ignore_paths() {
        let temp = tempfile::tempdir().unwrap();
        write_config(
            temp.path(),
            r#"{"ingestion_mode":"auto","auto_rules":{"ignore_paths":["deep/nested"]}}"#,
        );
        let config = SourceConfig::load(temp.path()).unwrap().unwrap();
        assert!(config.discover_target_sources(temp.path()).unwrap_err().to_string().contains("plain folder names"));
    }

    #[test]
    fn dormant_mode_object_is_not_validated() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("game-win")).unwrap();
        // Auto mode active; the explicit block references a missing dir and
        // empty targets, which must be ignored because it is dormant.
        write_config(
            temp.path(),
            r#"{"ingestion_mode":"auto","auto_rules":{"ignore_paths":[]},"explicit_rules":{"targets":[]}}"#,
        );
        let config = SourceConfig::load(temp.path()).unwrap().unwrap();
        let targets = config.discover_target_sources(temp.path()).unwrap();
        assert_eq!(targets.len(), 1);
    }
}
