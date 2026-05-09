use std::collections::{BTreeMap, HashSet};

use gutch as g;

#[derive(Debug, Clone)]
pub struct ResolvedTargetIdentity {
    pub version_id: String,
    pub target_id: String,
    pub platform: String,
    pub architecture: String,
    pub tier_id: String,
    pub target_tags: BTreeMap<String, String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct InferredTargetIdentity {
    version_id: Option<String>,
    platform: Option<String>,
    architecture: Option<String>,
}

const SUPPORTED_PLATFORM_ALIASES: &[(&str, &str)] = &[
    ("any", "any"),
    ("windows", "windows"),
    ("win", "windows"),
    ("win32", "windows"),
    ("win64", "windows"),
    ("linux", "linux"),
    ("mac", "macos"),
    ("macos", "macos"),
    ("osx", "macos"),
    ("darwin", "macos"),
];

const SUPPORTED_ARCHITECTURE_ALIASES: &[(&str, &str)] = &[
    ("any", "any"),
    ("x86_64", "x86_64"),
    ("x64", "x86_64"),
    ("amd64", "x86_64"),
    ("win64", "x86_64"),
    ("x86_32", "x86_32"),
    ("x86", "x86_32"),
    ("win32", "x86_32"),
];

pub fn default_target_schema() -> g::manifest::ContentRepoTargetSchema {
    g::manifest::ContentRepoTargetSchema {
        target_tags: vec![
            g::manifest::ContentRepoTargetTagDefinition {
                name: "platform".to_string(),
                required: true,
                values: vec![
                    "windows".to_string(),
                    "linux".to_string(),
                    "macos".to_string(),
                    "any".to_string(),
                ],
                aliases: BTreeMap::from([
                    ("win".to_string(), "windows".to_string()),
                    ("win32".to_string(), "windows".to_string()),
                    ("win64".to_string(), "windows".to_string()),
                    ("mac".to_string(), "macos".to_string()),
                    ("osx".to_string(), "macos".to_string()),
                    ("darwin".to_string(), "macos".to_string()),
                ]),
                default_value: Some("any".to_string()),
            },
            g::manifest::ContentRepoTargetTagDefinition {
                name: "arch".to_string(),
                required: true,
                values: vec![
                    "x86_64".to_string(),
                    "x86_32".to_string(),
                    "any".to_string(),
                ],
                aliases: BTreeMap::from([
                    ("x64".to_string(), "x86_64".to_string()),
                    ("amd64".to_string(), "x86_64".to_string()),
                    ("win64".to_string(), "x86_64".to_string()),
                    ("x86".to_string(), "x86_32".to_string()),
                    ("win32".to_string(), "x86_32".to_string()),
                ]),
                default_value: Some("any".to_string()),
            },
            g::manifest::ContentRepoTargetTagDefinition {
                name: "tier".to_string(),
                required: false,
                values: Vec::new(),
                aliases: BTreeMap::new(),
                default_value: Some("base".to_string()),
            },
        ],
        target_id_order: vec![
            "version".to_string(),
            "platform".to_string(),
            "arch".to_string(),
            "tier".to_string(),
        ],
    }
}

pub fn effective_target_schema(
    schema: Option<g::manifest::ContentRepoTargetSchema>,
) -> g::manifest::ContentRepoTargetSchema {
    schema.unwrap_or_else(default_target_schema)
}

pub fn validate_target_schema(schema: &g::manifest::ContentRepoTargetSchema) -> g::Result<()> {
    let mut seen = HashSet::new();
    for definition in &schema.target_tags {
        if definition.name.trim().is_empty() || definition.name == "version" {
            return Err(g::Error::manifest(
                "target tag names must be non-empty and cannot redefine 'version'",
            ));
        }
        if !seen.insert(definition.name.clone()) {
            return Err(g::Error::manifest(format!(
                "duplicate target tag definition '{}'",
                definition.name
            )));
        }
    }

    for tag_name in &schema.target_id_order {
        if tag_name != "version" && !seen.contains(tag_name) {
            return Err(g::Error::manifest(format!(
                "target_id_order references undefined tag '{}'",
                tag_name
            )));
        }
    }

    Ok(())
}

pub fn parse_cli_tags(tag_values: &[String]) -> g::Result<BTreeMap<String, String>> {
    let mut parsed_tags = BTreeMap::new();
    for tag_value in tag_values {
        let Some((name, value)) = tag_value.split_once('=') else {
            return Err(g::Error::manifest(format!(
                "invalid --tag '{}'. Use --tag name=value",
                tag_value
            )));
        };
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() || value.is_empty() {
            return Err(g::Error::manifest(format!(
                "invalid --tag '{}'. Use non-empty name=value",
                tag_value
            )));
        }
        parsed_tags.insert(name.to_string(), value.to_string());
    }
    Ok(parsed_tags)
}

pub fn resolve_target_input(
    spec: &crate::RepoInitTargetSpec,
    schema: &g::manifest::ContentRepoTargetSchema,
) -> g::Result<g::RepoInitTargetInput> {
    let context = format!("repo init target '{}'", spec.dir.display());
    let resolved_identity = resolve_target_identity(&spec.dir, &spec.tags, schema, &context)?;

    Ok(g::RepoInitTargetInput {
        dir: spec.dir.clone(),
        version_id: resolved_identity.version_id,
        platform: resolved_identity.platform,
        architecture: resolved_identity.architecture,
        tier_id: resolved_identity.tier_id,
        tags: resolved_identity.target_tags,
    })
}

pub fn resolve_target_identity(
    input_path: &std::path::Path,
    explicit_tags: &BTreeMap<String, String>,
    schema: &g::manifest::ContentRepoTargetSchema,
    context: &str,
) -> g::Result<ResolvedTargetIdentity> {
    validate_target_schema(schema)?;

    let mut allowed_explicit_tags: HashSet<&str> = HashSet::from(["version"]);
    for definition in &schema.target_tags {
        allowed_explicit_tags.insert(definition.name.as_str());
        if definition.name == "platform" {
            allowed_explicit_tags.insert("os");
        }
        if definition.name == "arch" {
            allowed_explicit_tags.insert("architecture");
        }
    }
    for explicit_tag_name in explicit_tags.keys() {
        if !allowed_explicit_tags.contains(explicit_tag_name.as_str()) {
            return Err(g::Error::manifest(format!(
                "{context} uses unsupported tag '{}'",
                explicit_tag_name
            )));
        }
    }

    let inferred = infer_target_identity_from_path(input_path);
    let inferred_custom_tags = infer_custom_tag_values_from_path(input_path, schema);
    let version_id = normalize_version_with_inference(
        tag_value(explicit_tags, &["version"]),
        inferred.version_id.as_deref(),
        context,
    )?;

    let mut target_tags = BTreeMap::new();
    target_tags.insert("version".to_string(), version_id.clone());

    for definition in &schema.target_tags {
        let explicit_value = explicit_tags
            .get(&definition.name)
            .map(String::as_str)
            .or_else(|| {
                if definition.name == "platform" {
                    tag_value(explicit_tags, &["os"])
                } else if definition.name == "arch" {
                    tag_value(explicit_tags, &["architecture"])
                } else {
                    None
                }
            });
        let inferred_value = inferred_custom_tags
            .get(&definition.name)
            .map(String::as_str)
            .or_else(|| {
                if definition.name == "platform" {
                    inferred.platform.as_deref()
                } else if definition.name == "arch" {
                    inferred.architecture.as_deref()
                } else {
                    None
                }
            });

        let normalized_value = if let Some(raw) = explicit_value {
            normalize_custom_tag_value(raw, definition).ok_or_else(|| {
                let supported_values = if definition.values.is_empty() {
                    "any non-empty value".to_string()
                } else {
                    definition.values.join(", ")
                };
                g::Error::manifest(format!(
                    "{context} has unsupported value '{}' for tag '{}'. Supported values: {}",
                    raw, definition.name, supported_values
                ))
            })?
        } else if let Some(value) = inferred_value {
            value.to_string()
        } else if let Some(default_value) = &definition.default_value {
            default_value.clone()
        } else if definition.required {
            return Err(g::Error::manifest(format!(
                "{context} is missing required tag '{}'",
                definition.name
            )));
        } else {
            continue;
        };

        target_tags.insert(definition.name.clone(), normalized_value);
    }

    let platform = target_tags
        .get("platform")
        .cloned()
        .unwrap_or_else(|| "any".to_string());
    let architecture = target_tags
        .get("arch")
        .cloned()
        .unwrap_or_else(|| "any".to_string());
    let tier_id = target_tags
        .get("tier")
        .cloned()
        .unwrap_or_else(|| "base".to_string());
    let target_id = build_target_id_from_tags(&version_id, &target_tags, schema);

    Ok(ResolvedTargetIdentity {
        version_id,
        target_id,
        platform,
        architecture,
        tier_id,
        target_tags,
    })
}

pub fn apply_resolved_target_identity(
    target_manifest: &mut g::manifest::TargetManifest,
    resolved_identity: &ResolvedTargetIdentity,
) {
    target_manifest.target_id = resolved_identity.target_id.clone();
    target_manifest.version_id = resolved_identity.version_id.clone();
    target_manifest.platform = resolved_identity.platform.clone();
    target_manifest.architecture = resolved_identity.architecture.clone();
    target_manifest.tier_id = resolved_identity.tier_id.clone();
    target_manifest.target_tags = resolved_identity.target_tags.clone();
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

fn normalize_platform(raw: &str) -> Option<String> {
    let lowered = raw.trim().to_ascii_lowercase();
    SUPPORTED_PLATFORM_ALIASES
        .iter()
        .find_map(|(alias, canonical)| (*alias == lowered).then(|| (*canonical).to_string()))
}

fn normalize_architecture(raw: &str) -> Option<String> {
    let lowered = raw.trim().to_ascii_lowercase();
    SUPPORTED_ARCHITECTURE_ALIASES
        .iter()
        .find_map(|(alias, canonical)| (*alias == lowered).then(|| (*canonical).to_string()))
}

fn infer_tag_value_from_token(
    token: &str,
    tag_definition: &g::manifest::ContentRepoTargetTagDefinition,
) -> Option<String> {
    let lowered = token.trim().to_ascii_lowercase();
    if let Some(value) = tag_definition.aliases.get(&lowered) {
        return Some(value.clone());
    }

    tag_definition
        .values
        .iter()
        .find(|value| value.eq_ignore_ascii_case(token))
        .cloned()
}

fn normalize_custom_tag_value(
    raw: &str,
    tag_definition: &g::manifest::ContentRepoTargetTagDefinition,
) -> Option<String> {
    if tag_definition.name == "platform" {
        return normalize_platform(raw);
    }
    if tag_definition.name == "arch" {
        return normalize_architecture(raw);
    }
    if tag_definition.name == "tier" && tag_definition.values.is_empty() {
        return Some(raw.trim().to_ascii_lowercase());
    }

    infer_tag_value_from_token(raw, tag_definition).or_else(|| {
        tag_definition
            .values
            .is_empty()
            .then(|| raw.trim().to_string())
    })
}

fn tokenize_identity_name(name: &str) -> Vec<String> {
    name.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.'))
        .filter(|token| !token.is_empty())
        .map(|token| token.to_string())
        .collect()
}

fn infer_target_identity_from_path(path: &std::path::Path) -> InferredTargetIdentity {
    let raw_name = if path.is_file() {
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
    } else {
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
    };

    let mut inferred = InferredTargetIdentity::default();
    for token in tokenize_identity_name(raw_name) {
        if inferred.version_id.is_none() {
            inferred.version_id = normalize_version(&token);
        }
        if inferred.platform.is_none() {
            inferred.platform = normalize_platform(&token);
        }
        if inferred.architecture.is_none() {
            inferred.architecture = normalize_architecture(&token);
        }
    }

    inferred
}

fn infer_custom_tag_values_from_path(
    path: &std::path::Path,
    schema: &g::manifest::ContentRepoTargetSchema,
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

    let mut inferred_tags = BTreeMap::new();
    for token in tokenize_identity_name(raw_name) {
        for definition in &schema.target_tags {
            if inferred_tags.contains_key(&definition.name) {
                continue;
            }
            if let Some(value) = normalize_custom_tag_value(&token, definition) {
                inferred_tags.insert(definition.name.clone(), value);
            }
        }
    }

    inferred_tags
}

fn normalize_version_with_inference(
    explicit: Option<&str>,
    inferred: Option<&str>,
    context: &str,
) -> g::Result<String> {
    if let Some(version) = explicit.and_then(normalize_version) {
        return Ok(version);
    }
    if let Some(version) = inferred.and_then(normalize_version) {
        return Ok(version);
    }

    Err(g::Error::manifest(format!(
        "{context} is missing a recognizable version. Use values like '1.0.0' or 'v1.0.0'."
    )))
}

fn tag_value<'a>(tags: &'a BTreeMap<String, String>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| tags.get(*key).map(String::as_str))
}

fn build_target_id_from_tags(
    version_id: &str,
    target_tags: &BTreeMap<String, String>,
    schema: &g::manifest::ContentRepoTargetSchema,
) -> String {
    let mut parts = vec![version_id.to_string()];
    let order = if schema.target_id_order.is_empty() {
        schema
            .target_tags
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>()
    } else {
        schema
            .target_id_order
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    };

    for tag_name in order {
        if tag_name == "version" {
            continue;
        }
        if let Some(tag_value) = target_tags.get(tag_name) {
            parts.push(tag_value.clone());
        }
    }

    parts.join("-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn normalize_version_strips_v_prefix() {
        assert_eq!(normalize_version("v1.0.0").as_deref(), Some("1.0.0"));
        assert_eq!(normalize_version("1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(normalize_version("version-1"), None);
    }

    #[test]
    fn normalize_platform_aliases_to_canonical_values() {
        assert_eq!(normalize_platform("win").as_deref(), Some("windows"));
        assert_eq!(normalize_platform("darwin").as_deref(), Some("macos"));
        assert_eq!(normalize_platform("linux").as_deref(), Some("linux"));
    }

    #[test]
    fn normalize_architecture_aliases_to_canonical_values() {
        assert_eq!(normalize_architecture("x64").as_deref(), Some("x86_64"));
        assert_eq!(normalize_architecture("amd64").as_deref(), Some("x86_64"));
        assert_eq!(normalize_architecture("x86").as_deref(), Some("x86_32"));
        assert_eq!(normalize_architecture("win32").as_deref(), Some("x86_32"));
    }

    #[test]
    fn infer_target_identity_from_path_name() {
        let inferred = infer_target_identity_from_path(std::path::Path::new(
            "D:\\builds\\game-v1.0.0-windows-x86",
        ));

        assert_eq!(inferred.version_id.as_deref(), Some("1.0.0"));
        assert_eq!(inferred.platform.as_deref(), Some("windows"));
        assert_eq!(inferred.architecture.as_deref(), Some("x86_32"));
    }

    #[test]
    fn resolve_target_input_uses_tag_aliases_and_inference() {
        let spec = crate::RepoInitTargetSpec {
            dir: PathBuf::from("D:\\builds\\game-v1.0.0-win-x64"),
            tags: BTreeMap::from([
                ("os".to_string(), "win".to_string()),
                ("tier".to_string(), "Base".to_string()),
            ]),
        };

        let resolved = resolve_target_input(&spec, &default_target_schema()).unwrap();

        assert_eq!(resolved.version_id, "1.0.0");
        assert_eq!(resolved.platform, "windows");
        assert_eq!(resolved.architecture, "x86_64");
        assert_eq!(resolved.tier_id, "base");
        assert_eq!(
            resolved.tags.get("platform").map(String::as_str),
            Some("windows")
        );
        assert_eq!(
            resolved.tags.get("arch").map(String::as_str),
            Some("x86_64")
        );
        assert_eq!(
            resolved.tags.get("version").map(String::as_str),
            Some("1.0.0")
        );
        assert!(!resolved.tags.contains_key("os"));
    }

    #[test]
    fn resolve_target_identity_supports_repo_defined_tags() {
        let schema = g::manifest::ContentRepoTargetSchema {
            target_tags: vec![
                g::manifest::ContentRepoTargetTagDefinition {
                    name: "loader".to_string(),
                    required: true,
                    values: vec![
                        "fabric".to_string(),
                        "forge".to_string(),
                        "neoforge".to_string(),
                    ],
                    aliases: BTreeMap::new(),
                    default_value: None,
                },
                g::manifest::ContentRepoTargetTagDefinition {
                    name: "mc".to_string(),
                    required: false,
                    values: vec!["1.20.1".to_string(), "1.21".to_string()],
                    aliases: BTreeMap::new(),
                    default_value: None,
                },
            ],
            target_id_order: vec![
                "version".to_string(),
                "loader".to_string(),
                "mc".to_string(),
            ],
        };
        let explicit_tags = BTreeMap::from([("loader".to_string(), "neoforge".to_string())]);

        let resolved = resolve_target_identity(
            std::path::Path::new("D:\\builds\\ae2-v15.4.10-neoforge-1.21"),
            &explicit_tags,
            &schema,
            "test target",
        )
        .unwrap();

        assert_eq!(resolved.version_id, "15.4.10");
        assert_eq!(resolved.target_id, "15.4.10-neoforge-1.21");
        assert_eq!(resolved.platform, "any");
        assert_eq!(resolved.architecture, "any");
        assert_eq!(resolved.tier_id, "base");
        assert_eq!(
            resolved.target_tags,
            BTreeMap::from([
                ("version".to_string(), "15.4.10".to_string()),
                ("loader".to_string(), "neoforge".to_string()),
                ("mc".to_string(), "1.21".to_string()),
            ])
        );
    }

    #[test]
    fn resolve_target_identity_rejects_unknown_tags() {
        let schema = g::manifest::ContentRepoTargetSchema {
            target_tags: vec![g::manifest::ContentRepoTargetTagDefinition {
                name: "loader".to_string(),
                required: true,
                values: vec!["fabric".to_string(), "forge".to_string()],
                aliases: BTreeMap::new(),
                default_value: None,
            }],
            target_id_order: vec!["version".to_string(), "loader".to_string()],
        };
        let explicit_tags = BTreeMap::from([
            ("loader".to_string(), "forge".to_string()),
            ("platform".to_string(), "windows".to_string()),
        ]);

        let error = resolve_target_identity(
            std::path::Path::new("D:\\builds\\game-v1.0.0-forge"),
            &explicit_tags,
            &schema,
            "test target",
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("test target uses unsupported tag 'platform'")
        );
    }
}
