use serde::{Deserialize, Serialize};

use super::{
    InferenceHandler, SnapshotTagDefinition, SnapshotTagKind, SnapshotTagSchema,
    SnapshotTagValueDefinition,
};

/// Named tagging rule templates that creators can pick when creating or configuring a repo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum TaggingRuleTemplate {
    Default,
    Minimal,
    SubscriptionExample,
}

impl TaggingRuleTemplate {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Minimal => "Minimal",
            Self::SubscriptionExample => "SubscriptionExample",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::Default => "version, os, arch, released_at",
            Self::Minimal => "version only",
            Self::SubscriptionExample => "version, os, arch, variant, released_at",
        }
    }
}

impl std::fmt::Display for TaggingRuleTemplate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl std::str::FromStr for TaggingRuleTemplate {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Default" => Ok(Self::Default),
            "Minimal" => Ok(Self::Minimal),
            "SubscriptionExample" => Ok(Self::SubscriptionExample),
            _ => Err(format!("Unknown tagging rule template: {s}")),
        }
    }
}

/// Returns all available templates.
pub fn list_templates() -> Vec<TaggingRuleTemplate> {
    vec![
        TaggingRuleTemplate::Default,
        TaggingRuleTemplate::Minimal,
        TaggingRuleTemplate::SubscriptionExample,
    ]
}

/// Returns the schema for the given template.
pub fn template_schema(template: TaggingRuleTemplate) -> SnapshotTagSchema {
    match template {
        TaggingRuleTemplate::Default => default_template(),
        TaggingRuleTemplate::Minimal => minimal_template(),
        TaggingRuleTemplate::SubscriptionExample => subscription_example_template(),
    }
}

fn version_tag() -> SnapshotTagDefinition {
    SnapshotTagDefinition {
        name: "version".to_string(),
        kind: SnapshotTagKind::Exclusive,
        required: true,
        inference_prefixes: vec!["-v".to_string()],
        values: Vec::new(),
        default_value: None,
        inference_handler: InferenceHandler::Version,
    }
}

fn os_tag() -> SnapshotTagDefinition {
    SnapshotTagDefinition {
        name: "os".to_string(),
        kind: SnapshotTagKind::Exclusive,
        required: false,
        inference_prefixes: vec!["-".to_string()],
        values: vec![
            SnapshotTagValueDefinition {
                value: "windows".to_string(),
                aliases: vec!["win".to_string()],
            },
            SnapshotTagValueDefinition {
                value: "linux".to_string(),
                aliases: Vec::new(),
            },
            SnapshotTagValueDefinition {
                value: "macos".to_string(),
                aliases: vec!["mac".to_string(), "osx".to_string(), "darwin".to_string()],
            },
        ],
        default_value: None,
        inference_handler: InferenceHandler::Default,
    }
}

fn arch_tag() -> SnapshotTagDefinition {
    SnapshotTagDefinition {
        name: "arch".to_string(),
        kind: SnapshotTagKind::Exclusive,
        required: false,
        inference_prefixes: vec!["-".to_string()],
        values: vec![
            SnapshotTagValueDefinition {
                value: "x86_64".to_string(),
                aliases: vec!["x64".to_string(), "amd64".to_string()],
            },
            SnapshotTagValueDefinition {
                value: "x86_32".to_string(),
                aliases: vec!["x86".to_string()],
            },
            SnapshotTagValueDefinition {
                value: "arm64".to_string(),
                aliases: vec!["aarch64".to_string()],
            },
        ],
        default_value: None,
        inference_handler: InferenceHandler::Default,
    }
}

fn released_at_tag() -> SnapshotTagDefinition {
    SnapshotTagDefinition {
        name: "released_at".to_string(),
        kind: SnapshotTagKind::Exclusive,
        required: false,
        inference_prefixes: vec!["-".to_string()],
        values: Vec::new(),
        default_value: None,
        inference_handler: InferenceHandler::Date,
    }
}

fn variant_tag() -> SnapshotTagDefinition {
    SnapshotTagDefinition {
        name: "variant".to_string(),
        kind: SnapshotTagKind::Exclusive,
        required: false,
        inference_prefixes: vec!["-".to_string()],
        values: vec![
            SnapshotTagValueDefinition {
                value: "free".to_string(),
                aliases: Vec::new(),
            },
            SnapshotTagValueDefinition {
                value: "tier1".to_string(),
                aliases: Vec::new(),
            },
            SnapshotTagValueDefinition {
                value: "tier2".to_string(),
                aliases: Vec::new(),
            },
        ],
        default_value: None,
        inference_handler: InferenceHandler::Default,
    }
}

fn default_template() -> SnapshotTagSchema {
    SnapshotTagSchema {
        snapshot_tags: vec![version_tag(), os_tag(), arch_tag(), released_at_tag()],
    }
}

fn minimal_template() -> SnapshotTagSchema {
    SnapshotTagSchema {
        snapshot_tags: vec![version_tag()],
    }
}

fn subscription_example_template() -> SnapshotTagSchema {
    SnapshotTagSchema {
        snapshot_tags: vec![
            version_tag(),
            os_tag(),
            arch_tag(),
            variant_tag(),
            released_at_tag(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_templates_returns_all() {
        let templates = list_templates();
        assert_eq!(templates.len(), 3);
        assert!(templates.contains(&TaggingRuleTemplate::Default));
        assert!(templates.contains(&TaggingRuleTemplate::Minimal));
        assert!(templates.contains(&TaggingRuleTemplate::SubscriptionExample));
    }

    #[test]
    fn default_template_has_standard_release_tags() {
        let schema = template_schema(TaggingRuleTemplate::Default);
        let names: Vec<&str> = schema
            .snapshot_tags
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, vec!["version", "os", "arch", "released_at"]);
        assert!(schema.snapshot_tags[0].required);
    }

    #[test]
    fn minimal_template_has_version_only() {
        let schema = template_schema(TaggingRuleTemplate::Minimal);
        assert_eq!(schema.snapshot_tags.len(), 1);
        assert_eq!(schema.snapshot_tags[0].name, "version");
        assert!(schema.snapshot_tags[0].required);
    }

    #[test]
    fn subscription_example_template_has_all_tags() {
        let schema = template_schema(TaggingRuleTemplate::SubscriptionExample);
        let names: Vec<&str> = schema
            .snapshot_tags
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["version", "os", "arch", "variant", "released_at"]
        );

        let variant = schema
            .snapshot_tags
            .iter()
            .find(|t| t.name == "variant")
            .unwrap();
        assert_eq!(variant.kind, SnapshotTagKind::Exclusive);
        let variant_values: Vec<&str> = variant.values.iter().map(|v| v.value.as_str()).collect();
        assert_eq!(variant_values, vec!["free", "tier1", "tier2"]);

        let released_at = schema
            .snapshot_tags
            .iter()
            .find(|t| t.name == "released_at")
            .unwrap();
        assert_eq!(released_at.kind, SnapshotTagKind::Exclusive);
    }

    #[test]
    fn default_template_has_no_access_tag() {
        let schema = template_schema(TaggingRuleTemplate::Default);
        assert!(!schema.snapshot_tags.iter().any(|t| t.name == "access"));
    }

    #[test]
    fn from_str_round_trips() {
        for template in list_templates() {
            let name = template.name();
            let parsed: TaggingRuleTemplate = name.parse().unwrap();
            assert_eq!(parsed, template);
        }
    }
}
