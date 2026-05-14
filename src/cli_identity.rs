use std::collections::BTreeMap;

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct ResolvedTargetIdentity {
    pub target_id: String,
    pub target_tags: BTreeMap<String, String>,
}

pub fn parse_cli_tags(tag_values: &[String]) -> Result<BTreeMap<String, String>> {
    let mut parsed_tags = BTreeMap::new();
    for tag_value in tag_values {
        let Some((name, value)) = tag_value.split_once('=') else {
            return Err(Error::manifest(format!(
                "invalid --tag '{}'. Use --tag name=value",
                tag_value
            )));
        };
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() || value.is_empty() {
            return Err(Error::manifest(format!(
                "invalid --tag '{}'. Use non-empty name=value",
                tag_value
            )));
        }
        parsed_tags.insert(name.to_string(), value.to_string());
    }
    Ok(parsed_tags)
}

pub fn resolve_target_identity(
    input_path: &std::path::Path,
    explicit_tags: &BTreeMap<String, String>,
    rules: &crate::manifest::AutoTaggingRules,
    context: &str,
) -> Result<ResolvedTargetIdentity> {
    let inferred_tags = infer_tags_from_path(input_path, rules);

    let mut target_tags = explicit_tags.clone();

    let version_inferred = inferred_tags.get("version").cloned();
    if !target_tags.contains_key("version") {
        if let Some(v) = version_inferred {
            target_tags.insert("version".to_string(), v);
        }
    }

    for (tag_name, value) in &inferred_tags {
        if tag_name == "version" {
            continue;
        }
        target_tags.entry(tag_name.clone()).or_insert_with(|| value.clone());
    }

    if !target_tags.contains_key("version") {
        return Err(Error::manifest(format!(
            "{context} is missing a recognizable version tag. Use --tag version=1.0.0 or infer from filename."
        )));
    }

    if matches!(rules.on_incomplete, crate::manifest::IncompleteInferenceBehavior::Error) {
        if let Some(tag) = rules.aliases.keys().find(|tag| !target_tags.contains_key(*tag)) {
            return Err(Error::manifest(format!(
                "{context} is missing tag '{}' which could not be inferred. Provide --tag {}=",
                tag, tag
            )));
        }
    }

    let target_id = compute_target_id(&target_tags);

    Ok(ResolvedTargetIdentity {
        target_id,
        target_tags,
    })
}

pub fn resolve_target_identity_from_rules(
    input_path: &std::path::Path,
    explicit_tags: &BTreeMap<String, String>,
    rules: Option<&crate::manifest::AutoTaggingRules>,
    context: &str,
) -> Result<ResolvedTargetIdentity> {
    let effective_rules = rules.cloned().unwrap_or_default();
    resolve_target_identity(input_path, explicit_tags, &effective_rules, context)
}

pub fn apply_resolved_target_identity(
    target_manifest: &mut crate::manifest::TargetManifest,
    resolved_identity: &ResolvedTargetIdentity,
) {
    target_manifest.target_id = resolved_identity.target_id.clone();
    target_manifest.target_tags = resolved_identity.target_tags.clone();
}

pub fn compute_target_id(target_tags: &BTreeMap<String, String>) -> String {
    let mut canonical = String::new();
    for (key, value) in target_tags {
        if !canonical.is_empty() {
            canonical.push('+');
        }
        canonical.push_str(&format!("{}={}", key, value));
    }
    blake3::hash(canonical.as_bytes()).to_hex().to_string()
}

fn tokenize_identity_name(name: &str) -> Vec<String> {
    name.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.'))
        .filter(|token| !token.is_empty())
        .map(|token| token.to_string())
        .collect()
}

fn normalize_version(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let candidate = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    if candidate.is_empty() {
        return None;
    }

    let parts: Vec<&str> = candidate.split('.').collect();
    if parts.is_empty()
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.chars().all(|ch| ch.is_ascii_digit()))
    {
        return None;
    }

    Some(candidate.to_string())
}

fn infer_tags_from_path(
    path: &std::path::Path,
    rules: &crate::manifest::AutoTaggingRules,
) -> BTreeMap<String, String> {
    let raw_name = if path.is_file() {
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
    } else {
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
    };

    let mut tags = BTreeMap::new();
    let mut found_version = false;

    for token in tokenize_identity_name(raw_name) {
        if !found_version {
            if let Some(version) = normalize_version(&token) {
                tags.insert("version".to_string(), version);
                found_version = true;
                continue;
            }
        }

        let lowered = token.to_ascii_lowercase();
        for (tag_name, alias_map) in &rules.aliases {
            if tags.contains_key(tag_name) {
                continue;
            }
            if let Some(canonical) = alias_map.get(&lowered) {
                tags.insert(tag_name.clone(), canonical.clone());
                break;
            }
        }
    }

    tags
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn test_rules() -> crate::manifest::AutoTaggingRules {
        crate::manifest::AutoTaggingRules::default()
    }

    #[test]
    fn test_version_strips_v_prefix() {
        assert_eq!(normalize_version("v1.0.0").as_deref(), Some("1.0.0"));
        assert_eq!(normalize_version("1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(normalize_version("version-1"), None);
    }

    #[test]
    fn test_infer_tags_from_path_default_aliases() {
        let rules = test_rules();
        let tags = infer_tags_from_path(
            std::path::Path::new("game-v1.0.0-win-x64"),
            &rules,
        );

        assert_eq!(tags.get("version").map(String::as_str), Some("1.0.0"));
        assert_eq!(tags.get("platform").map(String::as_str), Some("windows"));
        assert_eq!(tags.get("arch").map(String::as_str), Some("x86_64"));
    }

    #[test]
    fn test_resolve_identity_explicit_overrides_inference() {
        let rules = test_rules();
        let explicit = BTreeMap::from([
            ("version".to_string(), "2.0.0".to_string()),
            ("platform".to_string(), "linux".to_string()),
        ]);

        let resolved = resolve_target_identity(
            std::path::Path::new("game-v1.0.0-win-x64"),
            &explicit,
            &rules,
            "test",
        )
        .unwrap();

        assert_eq!(resolved.target_tags.get("version").map(String::as_str), Some("2.0.0"));
        assert_eq!(resolved.target_tags.get("platform").map(String::as_str), Some("linux"));
        assert_eq!(resolved.target_tags.get("arch").map(String::as_str), Some("x86_64"));
    }

    #[test]
    fn test_compute_target_id_is_deterministic() {
        let tags_a = BTreeMap::from([
            ("version".to_string(), "1.0.0".to_string()),
            ("platform".to_string(), "windows".to_string()),
        ]);
        let tags_b = BTreeMap::from([
            ("platform".to_string(), "windows".to_string()),
            ("version".to_string(), "1.0.0".to_string()),
        ]);

        assert_eq!(compute_target_id(&tags_a), compute_target_id(&tags_b));
    }

    #[test]
    fn test_resolve_missing_version_errors() {
        let rules = test_rules();
        let result = resolve_target_identity(
            std::path::Path::new("game-win-x64"),
            &BTreeMap::new(),
            &rules,
            "test",
        );

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("missing"));
    }

    #[test]
    fn test_resolve_custom_tags() {
        let mut rules = test_rules();
        rules.on_incomplete = crate::manifest::IncompleteInferenceBehavior::Warn;
        rules.aliases.insert(
            "loader".to_string(),
            BTreeMap::from([
                ("fabric".to_string(), "fabric".to_string()),
                ("forge".to_string(), "forge".to_string()),
            ]),
        );

        let resolved = resolve_target_identity(
            std::path::Path::new("mod-v1.0.0-fabric"),
            &BTreeMap::new(),
            &rules,
            "test",
        )
        .unwrap();

        assert_eq!(resolved.target_tags.get("version").map(String::as_str), Some("1.0.0"));
        assert_eq!(resolved.target_tags.get("loader").map(String::as_str), Some("fabric"));
    }

    #[test]
    fn test_explicit_tags_dont_need_inference() {
        let rules = test_rules();
        let explicit = BTreeMap::from([
            ("version".to_string(), "1.0.0".to_string()),
            ("platform".to_string(), "windows".to_string()),
            ("arch".to_string(), "x86_64".to_string()),
        ]);

        let resolved = resolve_target_identity(
            std::path::Path::new("some-random-folder"),
            &explicit,
            &rules,
            "test",
        )
        .unwrap();

        assert_eq!(resolved.target_tags.get("version").map(String::as_str), Some("1.0.0"));
        assert_eq!(resolved.target_tags.get("platform").map(String::as_str), Some("windows"));
    }
}





