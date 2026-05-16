use crate::error::{Error, Result};
use crate::manifest::{
    IncompleteInferenceBehavior, TargetManifest, TargetTagKind, TargetTagSchema, TargetTagSet,
};

#[derive(Debug, Clone)]
pub struct ResolvedTargetIdentity {
    pub target_id: String,
    pub target_tags: TargetTagSet,
}

pub fn parse_cli_tags(tag_values: &[String]) -> Result<TargetTagSet> {
    let mut parsed_tags = TargetTagSet::default();
    for tag_value in tag_values {
        apply_raw_tag(&mut parsed_tags, tag_value, true)?;
    }
    Ok(parsed_tags)
}

pub fn resolve_target_identity(
    input_path: &std::path::Path,
    explicit_tags: &TargetTagSet,
    rules: &TargetTagSchema,
    context: &str,
) -> Result<ResolvedTargetIdentity> {
    let inferred_tags = infer_tags_from_path(input_path, rules);
    let mut target_tags = explicit_tags.clone();
    target_tags.extend_missing_from(&inferred_tags);

    apply_schema_defaults(&mut target_tags, rules);

    if target_tags.get_exclusive("version").is_none() {
        return Err(Error::manifest(format!(
            "{context} is missing a recognizable version tag. Use --version 1.0.0, --tag version::1.0.0, or infer from filename."
        )));
    }

    if matches!(rules.on_incomplete, IncompleteInferenceBehavior::Error) {
        if let Some(missing_name) = rules
            .target_tags
            .iter()
            .find(|definition| definition.required && !has_definition_value(&target_tags, definition))
            .map(|definition| definition.name.as_str())
        {
            return Err(Error::manifest(format!(
                "{context} is missing required tag '{}'.",
                missing_name
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
    explicit_tags: &TargetTagSet,
    rules: Option<&TargetTagSchema>,
    context: &str,
) -> Result<ResolvedTargetIdentity> {
    let effective_rules = rules.cloned().unwrap_or_default();
    resolve_target_identity(input_path, explicit_tags, &effective_rules, context)
}

pub fn apply_resolved_target_identity(
    target_manifest: &mut TargetManifest,
    resolved_identity: &ResolvedTargetIdentity,
) {
    target_manifest.target_id = resolved_identity.target_id.clone();
    target_manifest.target_tags = resolved_identity.target_tags.clone();
}

pub fn compute_target_id(target_tags: &TargetTagSet) -> String {
    let canonical = target_tags.to_canonical_strings().join("+");
    blake3::hash(canonical.as_bytes()).to_hex().to_string()
}

fn apply_raw_tag(target_tags: &mut TargetTagSet, raw_tag: &str, strict_exclusive: bool) -> Result<()> {
    let trimmed = raw_tag.trim();
    if trimmed.is_empty() {
        return Err(Error::manifest("invalid empty --tag value".to_string()));
    }

    if let Some((prefix, value)) = trimmed.split_once("::") {
        if prefix.is_empty() || value.is_empty() || value.contains("::") {
            return Err(Error::manifest(format!(
                "invalid --tag '{}'. Use plain-tag, scoped:value, or exclusive::value",
                raw_tag
            )));
        }
        if strict_exclusive {
            if let Some(existing_value) = target_tags.exclusive.get(prefix) {
                if existing_value != value {
                    return Err(Error::manifest(format!(
                        "conflicting exclusive tag '{}'. Prefix '{}' already has value '{}'",
                        raw_tag, prefix, existing_value
                    )));
                }
            }
        }
        target_tags.upsert_exclusive(prefix.to_string(), value.to_string());
        return Ok(());
    }

    if trimmed.contains(':') {
        let Some((prefix, value)) = trimmed.split_once(':') else {
            return Err(Error::manifest(format!(
                "invalid --tag '{}'. Use plain-tag, scoped:value, or exclusive::value",
                raw_tag
            )));
        };
        if prefix.is_empty() || value.is_empty() || value.contains(':') {
            return Err(Error::manifest(format!(
                "invalid --tag '{}'. Use plain-tag, scoped:value, or exclusive::value",
                raw_tag
            )));
        }
        target_tags.insert_scoped(prefix.to_string(), value.to_string());
        return Ok(());
    }

    target_tags.insert_plain(trimmed.to_string());
    Ok(())
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

fn infer_tags_from_path(path: &std::path::Path, rules: &TargetTagSchema) -> TargetTagSet {
    let raw_name = if path.is_file() {
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
    } else {
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
    };

    let mut tags = TargetTagSet::default();
    let mut found_version = false;

    for token in tokenize_identity_name(raw_name) {
        if !found_version {
            if let Some(version) = normalize_version(&token) {
                tags.upsert_exclusive("version".to_string(), version);
                found_version = true;
                continue;
            }
        }

        let lowered = token.to_ascii_lowercase();
        for definition in &rules.target_tags {
            if definition.kind == TargetTagKind::Plain {
                continue;
            }
            if definition.kind == TargetTagKind::Exclusive && tags.has_exclusive(&definition.name) {
                continue;
            }
            if let Some(canonical) = definition.aliases.get(&lowered) {
                match definition.kind {
                    TargetTagKind::Plain => {}
                    TargetTagKind::Scoped => {
                        tags.insert_scoped(definition.name.clone(), canonical.clone());
                    }
                    TargetTagKind::Exclusive => {
                        tags.insert_exclusive_if_missing(definition.name.clone(), canonical.clone());
                    }
                }
                break;
            }
        }
    }

    tags
}

fn apply_schema_defaults(target_tags: &mut TargetTagSet, rules: &TargetTagSchema) {
    for definition in &rules.target_tags {
        let Some(default_value) = definition.default_value.as_deref() else {
            continue;
        };
        if has_definition_value(target_tags, definition) {
            continue;
        }
        match definition.kind {
            TargetTagKind::Plain => target_tags.insert_plain(default_value.to_string()),
            TargetTagKind::Scoped => {
                target_tags.insert_scoped(definition.name.clone(), default_value.to_string());
            }
            TargetTagKind::Exclusive => {
                target_tags.insert_exclusive_if_missing(
                    definition.name.clone(),
                    default_value.to_string(),
                );
            }
        }
    }
}

fn has_definition_value(
    target_tags: &TargetTagSet,
    definition: &crate::manifest::TargetTagDefinition,
) -> bool {
    match definition.kind {
        TargetTagKind::Plain => target_tags.plain.contains(&definition.name),
        TargetTagKind::Scoped => target_tags
            .scoped
            .get(&definition.name)
            .is_some_and(|values| !values.is_empty()),
        TargetTagKind::Exclusive => target_tags.exclusive.contains_key(&definition.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_rules() -> TargetTagSchema {
        TargetTagSchema::default()
    }

    #[test]
    fn test_parse_cli_tags_supports_all_three_kinds() {
        let parsed = parse_cli_tags(&[
            "alpha".to_string(),
            "loader:fabric".to_string(),
            "os::windows".to_string(),
        ])
        .unwrap();

        assert!(parsed.plain.contains("alpha"));
        assert!(parsed
            .scoped
            .get("loader")
            .is_some_and(|values| values.contains("fabric")));
        assert_eq!(parsed.get_exclusive("os"), Some("windows"));
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
        let tags = infer_tags_from_path(std::path::Path::new("game-v1.0.0-win-x64"), &rules);

        assert_eq!(tags.get_exclusive("version"), Some("1.0.0"));
        assert_eq!(tags.get_exclusive("os"), Some("windows"));
        assert_eq!(tags.get_exclusive("arch"), Some("x86_64"));
    }

    #[test]
    fn test_resolve_identity_explicit_overrides_inference() {
        let rules = test_rules();
        let mut explicit = TargetTagSet::default();
        explicit.upsert_exclusive("version".to_string(), "2.0.0".to_string());
        explicit.upsert_exclusive("os".to_string(), "linux".to_string());

        let resolved = resolve_target_identity(
            std::path::Path::new("game-v1.0.0-win-x64"),
            &explicit,
            &rules,
            "test",
        )
        .unwrap();

        assert_eq!(resolved.target_tags.get_exclusive("version"), Some("2.0.0"));
        assert_eq!(resolved.target_tags.get_exclusive("os"), Some("linux"));
        assert_eq!(resolved.target_tags.get_exclusive("arch"), Some("x86_64"));
    }

    #[test]
    fn test_compute_target_id_is_deterministic() {
        let mut tags_a = TargetTagSet::default();
        tags_a.upsert_exclusive("version".to_string(), "1.0.0".to_string());
        tags_a.upsert_exclusive("os".to_string(), "windows".to_string());

        let mut tags_b = TargetTagSet::default();
        tags_b.upsert_exclusive("os".to_string(), "windows".to_string());
        tags_b.upsert_exclusive("version".to_string(), "1.0.0".to_string());

        assert_eq!(compute_target_id(&tags_a), compute_target_id(&tags_b));
    }

    #[test]
    fn test_resolve_missing_version_errors() {
        let rules = test_rules();
        let result =
            resolve_target_identity(std::path::Path::new("game-win-x64"), &TargetTagSet::default(), &rules, "test");

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("missing"));
    }

    #[test]
    fn test_resolve_custom_scoped_tags() {
        let mut rules = test_rules();
        rules.on_incomplete = IncompleteInferenceBehavior::Warn;
        rules.target_tags.push(crate::manifest::TargetTagDefinition {
            name: "loader".to_string(),
            kind: TargetTagKind::Scoped,
            required: false,
            values: Vec::new(),
            aliases: std::collections::BTreeMap::from([
                ("fabric".to_string(), "fabric".to_string()),
                ("forge".to_string(), "forge".to_string()),
            ]),
            default_value: None,
        });

        let resolved = resolve_target_identity(
            std::path::Path::new("mod-v1.0.0-fabric"),
            &TargetTagSet::default(),
            &rules,
            "test",
        )
        .unwrap();

        assert_eq!(resolved.target_tags.get_exclusive("version"), Some("1.0.0"));
        assert!(resolved
            .target_tags
            .scoped
            .get("loader")
            .is_some_and(|values| values.contains("fabric")));
    }

    #[test]
    fn test_explicit_tags_dont_need_inference() {
        let rules = test_rules();
        let mut explicit = TargetTagSet::default();
        explicit.upsert_exclusive("version".to_string(), "1.0.0".to_string());
        explicit.upsert_exclusive("os".to_string(), "windows".to_string());
        explicit.upsert_exclusive("arch".to_string(), "x86_64".to_string());

        let resolved = resolve_target_identity(
            std::path::Path::new("some-random-folder"),
            &explicit,
            &rules,
            "test",
        )
        .unwrap();

        assert_eq!(resolved.target_tags.get_exclusive("version"), Some("1.0.0"));
        assert_eq!(resolved.target_tags.get_exclusive("os"), Some("windows"));
    }
}
