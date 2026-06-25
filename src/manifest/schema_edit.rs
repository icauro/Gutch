use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{TargetTagDefinition, TargetTagKind, TargetTagSchema, TargetTagValueDefinition};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetTagSchemaValidationError {
    pub field: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetTagSchemaValidationResult {
    pub valid: bool,
    pub errors: Vec<TargetTagSchemaValidationError>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditableTargetTagDefinition {
    pub name: String,
    pub kind: TargetTagKind,
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
}

pub fn validate_editable_target_tag_schema(
    definitions: Vec<EditableTargetTagDefinition>,
) -> TargetTagSchemaValidationResult {
    let mut errors = Vec::new();
    let mut seen_names = BTreeSet::new();
    let mut has_version = false;

    for (index, tag) in definitions.iter().enumerate() {
        let field_prefix = format!("targetTags[{index}]");
        let name = tag.name.trim();
        if name.is_empty() {
            errors.push(TargetTagSchemaValidationError {
                field: format!("{field_prefix}.name"),
                message: String::from("Tag name is required."),
            });
            continue;
        }
        if !seen_names.insert(name.to_string()) {
            errors.push(TargetTagSchemaValidationError {
                field: format!("{field_prefix}.name"),
                message: format!("Duplicate tag name '{name}'."),
            });
        }
        if name == "version" {
            has_version = true;
        }

        if tag.kind != TargetTagKind::Plain {
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
                    errors.push(TargetTagSchemaValidationError {
                        field: format!("{field_prefix}.aliases"),
                        message: format!("Alias '{alias}' points to unknown value '{canonical}'."),
                    });
                }
            }

            if let Some(default_value) = default_value {
                if !allowed_values.contains(&default_value) {
                    errors.push(TargetTagSchemaValidationError {
                        field: format!("{field_prefix}.defaultValue"),
                        message: String::from(
                            "Default value must be one of the allowed values or aliases.",
                        ),
                    });
                }
            }
        }
    }

    if !has_version {
        errors.push(TargetTagSchemaValidationError {
            field: String::from("targetTags"),
            message: String::from("Version tag is required."),
        });
    }

    TargetTagSchemaValidationResult {
        valid: errors.is_empty(),
        errors,
    }
}

pub fn sanitize_editable_target_tag_schema(
    definitions: Vec<EditableTargetTagDefinition>,
) -> Option<TargetTagSchema> {
    let target_tags = definitions
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

            Some(TargetTagDefinition {
                name,
                kind: tag.kind,
                required: tag.required,
                inference_prefixes: tag.inference_prefixes,
                values: canonical_values
                    .into_iter()
                    .map(|value| TargetTagValueDefinition {
                        aliases: aliases_by_value.remove(&value).unwrap_or_default(),
                        value,
                    })
                    .collect(),
                default_value,
            })
        })
        .collect::<Vec<_>>();

    if target_tags.is_empty() {
        None
    } else {
        Some(TargetTagSchema { target_tags })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn rejects_default_value_outside_allowed_values() {
        let result = validate_editable_target_tag_schema(vec![EditableTargetTagDefinition {
            name: String::from("version"),
            kind: TargetTagKind::Exclusive,
            required: true,
            inference_prefixes: vec![String::from("-")],
            values: vec![String::from("1.0.0")],
            aliases: BTreeMap::new(),
            default_value: Some(String::from("9.9.9")),
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
        let result = validate_editable_target_tag_schema(vec![EditableTargetTagDefinition {
            name: String::from("tier"),
            kind: TargetTagKind::Exclusive,
            required: false,
            inference_prefixes: vec![String::from("-")],
            values: vec![String::from("base")],
            aliases: BTreeMap::new(),
            default_value: None,
        }]);
        assert!(!result.valid);
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.field == "targetTags")
        );
    }
}
