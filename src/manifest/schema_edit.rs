use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    InferenceHandler, SnapshotTagDefinition, SnapshotTagKind, SnapshotTagSchema,
    SnapshotTagValueDefinition, is_valid_snapshot_tag_name,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotTagSchemaValidationError {
    pub field: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotTagSchemaValidationResult {
    pub valid: bool,
    pub errors: Vec<SnapshotTagSchemaValidationError>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditableSnapshotTagDefinition {
    pub name: String,
    pub kind: SnapshotTagKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub inference_prefixes: Vec<String>,
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    #[serde(default)]
    pub default_value: Option<String>,
    #[serde(default)]
    pub inference_handler: InferenceHandler,
}

pub fn validate_editable_snapshot_tag_schema(
    definitions: Vec<EditableSnapshotTagDefinition>,
) -> SnapshotTagSchemaValidationResult {
    let mut errors = Vec::new();
    let mut seen_names = BTreeSet::new();
    let mut seen_prefixes = BTreeSet::new();
    let mut has_version = false;
    let mut reached_scoped_tags = false;

    for (index, tag) in definitions.iter().enumerate() {
        let field_prefix = format!("snapshotTags[{index}]");
        let name = tag.name.trim();
        if name.is_empty() {
            errors.push(SnapshotTagSchemaValidationError {
                field: format!("{field_prefix}.name"),
                message: String::from("Tag name is required."),
            });
            continue;
        }
        let canonical_name = name.to_ascii_lowercase();
        if name != canonical_name || !is_valid_snapshot_tag_name(&canonical_name) {
            errors.push(SnapshotTagSchemaValidationError {
                field: format!("{field_prefix}.name"),
                message: format!(
                    "Tag name '{name}' must use lower-snake-case (lowercase letters, digits, and underscores)."
                ),
            });
        }
        if !seen_names.insert(canonical_name.clone()) {
            errors.push(SnapshotTagSchemaValidationError {
                field: format!("{field_prefix}.name"),
                message: format!(
                    "Tag prefix '{canonical_name}' can only be defined once as scoped or exclusive."
                ),
            });
        }
        if canonical_name == "version" {
            has_version = true;
            if index != 0 {
                errors.push(SnapshotTagSchemaValidationError {
                    field: format!("{field_prefix}.name"),
                    message: String::from("Version tag must be first."),
                });
            }
            if tag.kind != SnapshotTagKind::Exclusive {
                errors.push(SnapshotTagSchemaValidationError {
                    field: format!("{field_prefix}.kind"),
                    message: String::from("Version tag must be exclusive."),
                });
            }
            if !tag.required {
                errors.push(SnapshotTagSchemaValidationError {
                    field: format!("{field_prefix}.required"),
                    message: String::from("Version tag must be required."),
                });
            }
            if tag.inference_handler != InferenceHandler::Version {
                errors.push(SnapshotTagSchemaValidationError {
                    field: format!("{field_prefix}.inferenceHandler"),
                    message: String::from("Version tag must use the version inference handler."),
                });
            }
        }

        if tag.kind == SnapshotTagKind::Scoped {
            reached_scoped_tags = true;
        } else if reached_scoped_tags {
            errors.push(SnapshotTagSchemaValidationError {
                field: format!("{field_prefix}.kind"),
                message: String::from("Exclusive tags must appear before scoped tags."),
            });
        }

        let prefixes = tag
            .inference_prefixes
            .iter()
            .map(|prefix| prefix.trim())
            .filter(|prefix| !prefix.is_empty())
            .collect::<Vec<_>>();
        if tag.kind == SnapshotTagKind::Scoped && prefixes.is_empty() {
            errors.push(SnapshotTagSchemaValidationError {
                field: format!("{field_prefix}.inferencePrefixes"),
                message: String::from("Scoped tags require a named prefix such as '-loader-'."),
            });
        }
        for prefix in prefixes {
            if !prefix.starts_with('-') {
                errors.push(SnapshotTagSchemaValidationError {
                    field: format!("{field_prefix}.inferencePrefixes"),
                    message: format!("Inference prefix '{prefix}' must start with '-'."),
                });
            }
            if tag.kind == SnapshotTagKind::Scoped && !is_named_scoped_prefix(prefix) {
                errors.push(SnapshotTagSchemaValidationError {
                    field: format!("{field_prefix}.inferencePrefixes"),
                    message: format!("Scoped prefix '{prefix}' must use the named form '-name-'."),
                });
            }
            if prefix != "-" && !seen_prefixes.insert(prefix.to_string()) {
                errors.push(SnapshotTagSchemaValidationError {
                    field: format!("{field_prefix}.inferencePrefixes"),
                    message: format!("Inference prefix '{prefix}' is already used by another tag."),
                });
            }
        }

        {
            let explicit_values = tag
                .values
                .iter()
                .map(|candidate| candidate.trim().to_string())
                .filter(|candidate| !candidate.is_empty())
                .collect::<BTreeSet<_>>();
            let alias_pairs = tag
                .aliases
                .iter()
                .filter_map(|(alias, value)| {
                    let alias = alias.trim().to_string();
                    let value = value.trim().to_string();
                    if alias.is_empty() || value.is_empty() {
                        None
                    } else {
                        Some((alias, value))
                    }
                })
                .collect::<Vec<_>>();
            let default_value = tag
                .default_value
                .as_ref()
                .map(|candidate| candidate.trim().to_string())
                .filter(|candidate| !candidate.is_empty());

            let mut allowed_values = explicit_values;
            for (_, canonical) in &alias_pairs {
                allowed_values.insert(canonical.clone());
            }

            for (alias, canonical) in alias_pairs {
                if !allowed_values.contains(&canonical) {
                    errors.push(SnapshotTagSchemaValidationError {
                        field: format!("{field_prefix}.aliases"),
                        message: format!("Alias '{alias}' points to unknown value '{canonical}'."),
                    });
                }
            }

            if let Some(default_value) = default_value
                && !allowed_values.contains(&default_value)
            {
                errors.push(SnapshotTagSchemaValidationError {
                    field: format!("{field_prefix}.defaultValue"),
                    message: String::from(
                        "Default value must be one of the allowed values or aliases.",
                    ),
                });
            }
        }
    }

    if !has_version {
        errors.push(SnapshotTagSchemaValidationError {
            field: String::from("snapshotTags"),
            message: String::from("Version tag is required."),
        });
    }

    SnapshotTagSchemaValidationResult {
        valid: errors.is_empty(),
        errors,
    }
}

pub fn validate_snapshot_tag_schema(
    schema: &SnapshotTagSchema,
) -> SnapshotTagSchemaValidationResult {
    validate_editable_snapshot_tag_schema(
        schema
            .snapshot_tags
            .iter()
            .map(|definition| EditableSnapshotTagDefinition {
                name: definition.name.clone(),
                kind: definition.kind.clone(),
                required: definition.required,
                inference_prefixes: definition.inference_prefixes.clone(),
                values: definition
                    .values
                    .iter()
                    .map(|value| value.value.clone())
                    .collect(),
                aliases: definition
                    .values
                    .iter()
                    .flat_map(|value| {
                        value
                            .aliases
                            .iter()
                            .map(|alias| (alias.clone(), value.value.clone()))
                    })
                    .collect(),
                default_value: definition.default_value.clone(),
                inference_handler: definition.inference_handler,
            })
            .collect(),
    )
}

pub fn sanitize_editable_snapshot_tag_schema(
    definitions: Vec<EditableSnapshotTagDefinition>,
) -> Option<SnapshotTagSchema> {
    let mut snapshot_tags = definitions
        .into_iter()
        .filter_map(|tag| {
            let name = tag.name.trim().to_string();
            if name.is_empty() {
                return None;
            }

            let explicit_values = tag
                .values
                .into_iter()
                .map(|candidate| candidate.trim().to_string())
                .filter(|candidate| !candidate.is_empty())
                .collect::<BTreeSet<_>>();
            let alias_pairs = tag
                .aliases
                .into_iter()
                .filter_map(|(alias, value)| {
                    let alias = alias.trim().to_string();
                    let value = value.trim().to_string();
                    if alias.is_empty() || value.is_empty() {
                        None
                    } else {
                        Some((alias, value))
                    }
                })
                .collect::<Vec<_>>();
            let default_value = tag
                .default_value
                .map(|candidate| candidate.trim().to_string())
                .filter(|candidate| !candidate.is_empty());

            let mut canonical_values = explicit_values;
            if let Some(default_value) = &default_value {
                canonical_values.insert(default_value.clone());
            }
            for (_, canonical) in &alias_pairs {
                canonical_values.insert(canonical.clone());
            }

            let mut aliases_by_value = BTreeMap::<String, Vec<String>>::new();
            for (alias, canonical) in alias_pairs {
                aliases_by_value.entry(canonical).or_default().push(alias);
            }

            Some(SnapshotTagDefinition {
                name,
                kind: tag.kind,
                required: tag.required,
                inference_prefixes: tag
                    .inference_prefixes
                    .into_iter()
                    .map(|prefix| prefix.trim().to_string())
                    .filter(|prefix| !prefix.is_empty())
                    .collect(),
                values: canonical_values
                    .into_iter()
                    .map(|value| SnapshotTagValueDefinition {
                        aliases: aliases_by_value.remove(&value).unwrap_or_default(),
                        value,
                    })
                    .collect(),
                default_value,
                inference_handler: tag.inference_handler,
            })
        })
        .collect::<Vec<_>>();

    snapshot_tags.sort_by_key(|definition| match definition.kind {
        SnapshotTagKind::Exclusive => 0,
        SnapshotTagKind::Scoped => 1,
    });

    if snapshot_tags.is_empty() {
        None
    } else {
        Some(SnapshotTagSchema { snapshot_tags })
    }
}

fn is_named_scoped_prefix(prefix: &str) -> bool {
    let Some(name) = prefix
        .strip_prefix('-')
        .and_then(|value| value.strip_suffix('-'))
    else {
        return false;
    };
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|character| character.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn rejects_default_value_outside_allowed_values() {
        let result = validate_editable_snapshot_tag_schema(vec![EditableSnapshotTagDefinition {
            name: String::from("version"),
            kind: SnapshotTagKind::Exclusive,
            required: true,
            inference_prefixes: vec![String::from("-")],
            values: vec![String::from("1.0.0")],
            aliases: BTreeMap::new(),
            default_value: Some(String::from("9.9.9")),
            inference_handler: InferenceHandler::Default,
        }]);
        assert!(!result.valid);
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.field.contains("defaultValue"))
        );
    }

    #[test]
    fn requires_version_tag() {
        let result = validate_editable_snapshot_tag_schema(vec![EditableSnapshotTagDefinition {
            name: String::from("tier"),
            kind: SnapshotTagKind::Exclusive,
            required: false,
            inference_prefixes: vec![String::from("-")],
            values: vec![String::from("base")],
            aliases: BTreeMap::new(),
            default_value: None,
            inference_handler: InferenceHandler::Default,
        }]);
        assert!(!result.valid);
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.field == "snapshotTags")
        );
    }

    #[test]
    fn version_tag_is_fixed_and_first() {
        let result = validate_editable_snapshot_tag_schema(vec![
            EditableSnapshotTagDefinition {
                name: String::from("os"),
                kind: SnapshotTagKind::Exclusive,
                required: false,
                inference_prefixes: vec![String::from("-")],
                values: vec![String::from("windows")],
                aliases: BTreeMap::new(),
                default_value: None,
                inference_handler: InferenceHandler::Default,
            },
            EditableSnapshotTagDefinition {
                name: String::from("version"),
                kind: SnapshotTagKind::Scoped,
                required: true,
                inference_prefixes: vec![String::from("-v")],
                values: Vec::new(),
                aliases: BTreeMap::new(),
                default_value: None,
                inference_handler: InferenceHandler::Default,
            },
        ]);

        assert!(!result.valid);
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.message == "Version tag must be first.")
        );
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.message == "Version tag must be exclusive.")
        );
        assert!(result.errors.iter().any(|error| {
            error.message == "Version tag must use the version inference handler."
        }));
    }

    #[test]
    fn version_tag_must_be_required() {
        let result = validate_editable_snapshot_tag_schema(vec![EditableSnapshotTagDefinition {
            name: String::from("version"),
            kind: SnapshotTagKind::Exclusive,
            required: false,
            inference_prefixes: vec![String::from("-v")],
            values: Vec::new(),
            aliases: BTreeMap::new(),
            default_value: None,
            inference_handler: InferenceHandler::Version,
        }]);

        assert!(!result.valid);
        assert!(
            result
                .errors
                .iter()
                .any(|error| { error.message == "Version tag must be required." })
        );
    }

    #[test]
    fn sanitization_trims_inference_prefixes() {
        let schema = sanitize_editable_snapshot_tag_schema(vec![EditableSnapshotTagDefinition {
            name: String::from("version"),
            kind: SnapshotTagKind::Exclusive,
            required: true,
            inference_prefixes: vec![String::from(" -v "), String::new()],
            values: Vec::new(),
            aliases: BTreeMap::new(),
            default_value: None,
            inference_handler: InferenceHandler::Version,
        }])
        .expect("schema");

        assert_eq!(schema.snapshot_tags[0].inference_prefixes, vec!["-v"]);
    }

    #[test]
    fn requires_named_scoped_prefixes_and_exclusive_first_order() {
        let result = validate_editable_snapshot_tag_schema(vec![
            EditableSnapshotTagDefinition {
                name: String::from("version"),
                kind: SnapshotTagKind::Exclusive,
                required: true,
                inference_prefixes: vec![String::from("-v")],
                values: Vec::new(),
                aliases: BTreeMap::new(),
                default_value: None,
                inference_handler: InferenceHandler::Version,
            },
            EditableSnapshotTagDefinition {
                name: String::from("loader"),
                kind: SnapshotTagKind::Scoped,
                required: false,
                inference_prefixes: vec![String::from("-")],
                values: vec![String::from("fabric")],
                aliases: BTreeMap::new(),
                default_value: None,
                inference_handler: InferenceHandler::Default,
            },
            EditableSnapshotTagDefinition {
                name: String::from("os"),
                kind: SnapshotTagKind::Exclusive,
                required: false,
                inference_prefixes: vec![String::from("os-")],
                values: vec![String::from("windows")],
                aliases: BTreeMap::new(),
                default_value: None,
                inference_handler: InferenceHandler::Default,
            },
        ]);

        assert!(!result.valid);
        assert!(result.errors.iter().any(|error| {
            error.message == "Scoped prefix '-' must use the named form '-name-'."
        }));
        assert!(
            result
                .errors
                .iter()
                .any(|error| { error.message == "Exclusive tags must appear before scoped tags." })
        );
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.message == "Inference prefix 'os-' must start with '-'.")
        );
    }

    #[test]
    fn sanitization_places_exclusive_tags_before_scoped_tags() {
        let schema = sanitize_editable_snapshot_tag_schema(vec![
            EditableSnapshotTagDefinition {
                name: String::from("loader"),
                kind: SnapshotTagKind::Scoped,
                required: false,
                inference_prefixes: vec![String::from("-loader-")],
                values: vec![String::from("fabric")],
                aliases: BTreeMap::new(),
                default_value: None,
                inference_handler: InferenceHandler::Default,
            },
            EditableSnapshotTagDefinition {
                name: String::from("version"),
                kind: SnapshotTagKind::Exclusive,
                required: true,
                inference_prefixes: vec![String::from("-v")],
                values: Vec::new(),
                aliases: BTreeMap::new(),
                default_value: None,
                inference_handler: InferenceHandler::Version,
            },
        ])
        .expect("schema");

        assert_eq!(schema.snapshot_tags[0].name, "version");
        assert_eq!(schema.snapshot_tags[1].name, "loader");
    }

    #[test]
    fn rejects_noncanonical_tag_names() {
        let result = validate_editable_snapshot_tag_schema(vec![
            EditableSnapshotTagDefinition {
                name: String::from("version"),
                kind: SnapshotTagKind::Exclusive,
                required: true,
                inference_prefixes: Vec::new(),
                values: Vec::new(),
                aliases: BTreeMap::new(),
                default_value: None,
                inference_handler: InferenceHandler::Version,
            },
            EditableSnapshotTagDefinition {
                name: String::from("Build-Type"),
                kind: SnapshotTagKind::Exclusive,
                required: false,
                inference_prefixes: Vec::new(),
                values: Vec::new(),
                aliases: BTreeMap::new(),
                default_value: None,
                inference_handler: InferenceHandler::Default,
            },
        ]);

        assert!(!result.valid);
        assert!(
            result
                .errors
                .iter()
                .any(|error| { error.message.contains("must use lower-snake-case") })
        );
    }

    #[test]
    fn rejects_a_prefix_defined_as_both_tag_kinds() {
        let result = validate_editable_snapshot_tag_schema(vec![
            EditableSnapshotTagDefinition {
                name: String::from("version"),
                kind: SnapshotTagKind::Exclusive,
                required: true,
                inference_prefixes: Vec::new(),
                values: Vec::new(),
                aliases: BTreeMap::new(),
                default_value: None,
                inference_handler: InferenceHandler::Version,
            },
            EditableSnapshotTagDefinition {
                name: String::from("platform"),
                kind: SnapshotTagKind::Exclusive,
                required: false,
                inference_prefixes: Vec::new(),
                values: Vec::new(),
                aliases: BTreeMap::new(),
                default_value: None,
                inference_handler: InferenceHandler::Default,
            },
            EditableSnapshotTagDefinition {
                name: String::from("platform"),
                kind: SnapshotTagKind::Scoped,
                required: false,
                inference_prefixes: vec![String::from("-platform-")],
                values: Vec::new(),
                aliases: BTreeMap::new(),
                default_value: None,
                inference_handler: InferenceHandler::Default,
            },
        ]);

        assert!(!result.valid);
        assert!(result.errors.iter().any(|error| {
            error
                .message
                .contains("can only be defined once as scoped or exclusive")
        }));
    }
}
