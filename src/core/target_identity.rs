use crate::error::{Error, Result};
use crate::manifest::{
    StructuredVersion, TargetTagDefinition, TargetTagKind, TargetTagSchema, TargetTagSet,
    target::TargetManifest,
};

#[derive(Debug, Clone)]
pub struct ResolvedTargetIdentity {
    pub target_tags: TargetTagSet,
}

pub fn parse_cli_tags(tag_values: &[String]) -> Result<TargetTagSet> {
    TargetTagSet::from_compact_strings(tag_values)
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

    Ok(ResolvedTargetIdentity { target_tags })
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
    target_manifest.target_tags = resolved_identity.target_tags.clone();
}

fn identity_segments(name: &str) -> Vec<String> {
    name.split('-')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn normalize_version(raw: &str) -> Option<String> {
    StructuredVersion::parse(raw)
        .ok()
        .map(|version| version.canonical_string())
}

fn segment_candidate_for_prefix<'a>(segment: &'a str, prefix: &str) -> Option<&'a str> {
    if prefix == "-" {
        return Some(segment);
    }
    let required_prefix = prefix.strip_prefix('-')?;
    segment.strip_prefix(required_prefix)
}

fn definition_matches_token(definition: &TargetTagDefinition, token: &str) -> bool {
    definition
        .values
        .iter()
        .any(|value_definition| value_definition.matches_token(token))
}

fn another_definition_matches_token(
    rules: &TargetTagSchema,
    current_definition_name: &str,
    token: &str,
) -> bool {
    rules.target_tags.iter().any(|definition| {
        definition.name != current_definition_name && definition_matches_token(definition, token)
    })
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

    for segment in identity_segments(raw_name) {
        for definition in &rules.target_tags {
            if definition.kind == TargetTagKind::Plain {
                continue;
            }
            if definition.kind == TargetTagKind::Exclusive && tags.has_exclusive(&definition.name) {
                continue;
            }

            let candidate = definition
                .inference_prefixes
                .iter()
                .find_map(|prefix| segment_candidate_for_prefix(&segment, prefix));
            let Some(candidate) = candidate else {
                continue;
            };

            if definition.name == "version" && !found_version {
                if let Some(version) = normalize_version(candidate) {
                    tags.upsert_exclusive("version".to_string(), version);
                    found_version = true;
                    break;
                }
                continue;
            }

            let lowered = candidate.to_ascii_lowercase();
            if let Some(canonical) = definition
                .values
                .iter()
                .find(|value_definition| value_definition.matches_token(&lowered))
                .map(|value_definition| value_definition.value.as_str())
            {
                match definition.kind {
                    TargetTagKind::Plain => {}
                    TargetTagKind::Scoped => {
                        tags.insert_scoped(definition.name.clone(), canonical.to_string());
                    }
                    TargetTagKind::Exclusive => {
                        tags.insert_exclusive_if_missing(
                            definition.name.clone(),
                            canonical.to_string(),
                        );
                    }
                }
                break;
            }

            if definition.values.is_empty() {
                if definition.kind == TargetTagKind::Exclusive && !found_version {
                    continue;
                }
                if another_definition_matches_token(rules, &definition.name, &lowered) {
                    continue;
                }
                match definition.kind {
                    TargetTagKind::Plain => {}
                    TargetTagKind::Scoped => {
                        tags.insert_scoped(definition.name.clone(), lowered.clone());
                    }
                    TargetTagKind::Exclusive => {
                        tags.insert_exclusive_if_missing(definition.name.clone(), lowered.clone());
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

fn has_definition_value(target_tags: &TargetTagSet, definition: &TargetTagDefinition) -> bool {
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
#[path = "identity_tests.rs"]
mod tests;
