use crate::error::{Error, Result};
use crate::manifest::{
    SnapshotTagDefinition, SnapshotTagKind, SnapshotTagSchema, SnapshotTagSet,
    snapshot::SnapshotManifest,
};

#[derive(Debug, Clone)]
pub struct ResolvedSnapshotIdentity {
    pub snapshot_tags: SnapshotTagSet,
}

pub(crate) fn parse_cli_tags(tag_values: &[String]) -> Result<SnapshotTagSet> {
    SnapshotTagSet::from_compact_strings(tag_values)
}

pub(crate) fn resolve_snapshot_identity(
    input_path: &std::path::Path,
    explicit_tags: &SnapshotTagSet,
    rules: &SnapshotTagSchema,
    context: &str,
) -> Result<ResolvedSnapshotIdentity> {
    resolve_snapshot_identity_with_inference(input_path, explicit_tags, rules, true, context)
}

/// Like [`resolve_snapshot_identity`], but name-based tag inference can be
/// disabled (explicit ingestion mode). Explicit tags still win over inferred
/// tags; inference only fills values missing from the explicit set.
pub(crate) fn resolve_snapshot_identity_with_inference(
    input_path: &std::path::Path,
    explicit_tags: &SnapshotTagSet,
    rules: &SnapshotTagSchema,
    allow_name_inference: bool,
    context: &str,
) -> Result<ResolvedSnapshotIdentity> {
    let mut snapshot_tags = normalize_explicit_tags(explicit_tags, rules, context)?;
    if allow_name_inference {
        let inferred_tags = infer_tags_from_path(input_path, rules);
        snapshot_tags.extend_missing_from(&inferred_tags);
    }

    apply_schema_defaults(&mut snapshot_tags, rules);

    if snapshot_tags.get_exclusive("version").is_none() {
        return Err(Error::manifest(format!(
            "{context} is missing a recognizable version tag. Use --version 1.0.0, --tag version::1.0.0, or infer from filename."
        )));
    }

    if let Some(missing_name) = rules
        .snapshot_tags
        .iter()
        .find(|definition| definition.required && !has_definition_value(&snapshot_tags, definition))
        .map(|definition| definition.name.as_str())
    {
        return Err(Error::manifest(format!(
            "{context} is missing required tag '{}'.",
            missing_name
        )));
    }

    Ok(ResolvedSnapshotIdentity { snapshot_tags })
}

pub(crate) fn apply_resolved_snapshot_identity(
    snapshot_manifest: &mut SnapshotManifest,
    resolved_identity: &ResolvedSnapshotIdentity,
) {
    snapshot_manifest.snapshot_tags = resolved_identity.snapshot_tags.clone();
}

fn identity_segments(name: &str) -> Vec<String> {
    name.split('-')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn segment_candidate_for_prefix<'a>(
    segments: &'a [String],
    segment_index: usize,
    prefix: &str,
) -> Option<(&'a str, usize)> {
    let segment = segments.get(segment_index)?;
    if prefix == "-" {
        return Some((segment, 1));
    }
    let required_prefix = prefix.strip_prefix('-')?;
    if let Some(marker) = required_prefix.strip_suffix('-') {
        if segment == marker {
            return segments
                .get(segment_index + 1)
                .map(|candidate| (candidate.as_str(), 2));
        }
        return None;
    }
    segment
        .strip_prefix(required_prefix)
        .map(|candidate| (candidate, 1))
}

fn definition_matches_token(definition: &SnapshotTagDefinition, token: &str) -> bool {
    definition
        .values
        .iter()
        .any(|value_definition| value_definition.matches_token(token))
}

fn another_definition_matches_token(
    rules: &SnapshotTagSchema,
    current_definition_name: &str,
    token: &str,
) -> bool {
    rules.snapshot_tags.iter().any(|definition| {
        definition.name != current_definition_name && definition_matches_token(definition, token)
    })
}

fn infer_tags_from_path(path: &std::path::Path, rules: &SnapshotTagSchema) -> SnapshotTagSet {
    let raw_name = if path.is_file() {
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
    } else {
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
    };

    let mut tags = SnapshotTagSet::default();

    let segments = identity_segments(raw_name);
    let mut consumed_segments = std::collections::BTreeSet::new();

    for segment_index in 0..segments.len() {
        if consumed_segments.contains(&segment_index) {
            continue;
        }
        // Named scoped markers are exact namespace declarations. Resolve them
        // before catch-all exclusive value matching so a marker can never be
        // mistaken for an exclusive value.
        for definition in rules
            .snapshot_tags
            .iter()
            .filter(|definition| definition.kind == SnapshotTagKind::Scoped)
            .chain(
                rules
                    .snapshot_tags
                    .iter()
                    .filter(|definition| definition.kind == SnapshotTagKind::Exclusive),
            )
        {
            if definition.kind == SnapshotTagKind::Exclusive && tags.has_exclusive(&definition.name)
            {
                continue;
            }

            let candidate = definition
                .inference_prefixes
                .iter()
                .find_map(|prefix| segment_candidate_for_prefix(&segments, segment_index, prefix));
            let Some((candidate, consumed_count)) = candidate else {
                continue;
            };

            // Use handler to parse/validate the candidate
            if let Some(normalized) = definition.inference_handler.parse_token(candidate) {
                // For tags with predefined values, match against them
                if !definition.values.is_empty() {
                    let lowered = normalized.to_ascii_lowercase();
                    if let Some(canonical) = definition
                        .values
                        .iter()
                        .find(|v| v.matches_token(&lowered))
                        .map(|v| v.value.as_str())
                    {
                        insert_tag(&mut tags, definition, canonical.to_string());
                        consumed_segments.extend(segment_index..segment_index + consumed_count);
                        break;
                    }
                    // Handler accepted but no value match - skip
                    continue;
                }

                // No predefined values, accept the normalized value
                let lowered = normalized.to_ascii_lowercase();
                if another_definition_matches_token(rules, &definition.name, &lowered) {
                    continue;
                }
                insert_tag(&mut tags, definition, normalized);
                consumed_segments.extend(segment_index..segment_index + consumed_count);
                break;
            }
        }
    }

    tags
}

fn insert_tag(tags: &mut SnapshotTagSet, definition: &SnapshotTagDefinition, value: String) {
    match definition.kind {
        SnapshotTagKind::Scoped => {
            tags.insert_scoped(definition.name.clone(), value);
        }
        SnapshotTagKind::Exclusive => {
            tags.insert_exclusive_if_missing(definition.name.clone(), value);
        }
    }
}

fn apply_schema_defaults(snapshot_tags: &mut SnapshotTagSet, rules: &SnapshotTagSchema) {
    for definition in &rules.snapshot_tags {
        let Some(default_value) = definition.default_value.as_deref() else {
            continue;
        };
        if has_definition_value(snapshot_tags, definition) {
            continue;
        }
        match definition.kind {
            SnapshotTagKind::Scoped => {
                snapshot_tags.insert_scoped(definition.name.clone(), default_value.to_string());
            }
            SnapshotTagKind::Exclusive => {
                snapshot_tags.insert_exclusive_if_missing(
                    definition.name.clone(),
                    default_value.to_string(),
                );
            }
        }
    }
}

fn has_definition_value(
    snapshot_tags: &SnapshotTagSet,
    definition: &SnapshotTagDefinition,
) -> bool {
    match definition.kind {
        SnapshotTagKind::Scoped => snapshot_tags
            .scoped
            .get(&definition.name)
            .is_some_and(|values| !values.is_empty()),
        SnapshotTagKind::Exclusive => snapshot_tags.exclusive.contains_key(&definition.name),
    }
}

fn normalize_explicit_tags(
    explicit_tags: &SnapshotTagSet,
    rules: &SnapshotTagSchema,
    context: &str,
) -> Result<SnapshotTagSet> {
    let mut normalized = SnapshotTagSet::default();
    for (raw_name, values) in &explicit_tags.scoped {
        let name = raw_name.trim().to_ascii_lowercase();
        if name.is_empty() {
            return Err(Error::manifest(format!(
                "{context} contains a scoped tag with an empty name"
            )));
        }
        if rules
            .find_definition(&name)
            .is_some_and(|definition| definition.kind == SnapshotTagKind::Exclusive)
        {
            return Err(Error::manifest(format!(
                "{context} uses scoped syntax for exclusive tag '{name}'"
            )));
        }
        let definition = rules.find_definition(&name);
        for value in values {
            let value = canonicalize_explicit_value(definition, value, context, &name)?;
            normalized.insert_scoped(name.clone(), value);
        }
    }
    for (raw_name, raw_value) in &explicit_tags.exclusive {
        let name = raw_name.trim().to_ascii_lowercase();
        if name.is_empty() {
            return Err(Error::manifest(format!(
                "{context} contains an exclusive tag with an empty name"
            )));
        }
        if rules
            .find_definition(&name)
            .is_some_and(|definition| definition.kind == SnapshotTagKind::Scoped)
        {
            return Err(Error::manifest(format!(
                "{context} uses exclusive syntax for scoped tag '{name}'"
            )));
        }
        let value = canonicalize_explicit_value(rules.find_definition(&name), raw_value, context, &name)?;
        if normalized.has_exclusive(&name)
            && normalized.get_exclusive(&name) != Some(value.as_str())
        {
            return Err(Error::manifest(format!(
                "{context} contains conflicting exclusive values for '{name}'"
            )));
        }
        normalized.upsert_exclusive(name, value);
    }
    Ok(normalized)
}

fn canonicalize_explicit_value(
    definition: Option<&SnapshotTagDefinition>,
    raw_value: &str,
    context: &str,
    name: &str,
) -> Result<String> {
    let trimmed = raw_value.trim();
    if trimmed.is_empty() {
        return Err(Error::manifest(format!(
            "{context} contains an empty value for tag '{name}'"
        )));
    }
    let Some(definition) = definition else {
        return Ok(trimmed.to_string());
    };
    let normalized = definition
        .inference_handler
        .parse_token(trimmed)
        .ok_or_else(|| Error::manifest(format!("invalid value '{trimmed}' for tag '{name}'")))?;
    if definition.values.is_empty() {
        return Ok(normalized);
    }
    definition
        .values
        .iter()
        .find(|value| value.matches_token(&normalized))
        .map(|value| value.value.clone())
        .ok_or_else(|| Error::manifest(format!("value '{trimmed}' is not allowed for tag '{name}'")))
}

#[cfg(test)]
#[path = "../core/identity_tests.rs"]
mod tests;
