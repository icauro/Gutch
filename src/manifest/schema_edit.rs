use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use super::{TargetTagDefinition, TargetTagKind, TargetTagSchema, TargetTagValueDefinition};

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
