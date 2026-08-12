use serde::{Deserialize, Serialize};

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetProductMetadata {
    #[serde(rename = "document_version")]
    pub document_version: String,
    pub runtime: TargetRuntimeMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum TargetRuntimeMetadata {
    Direct(TargetDirectRuntimeMetadata),
    HostVariants(TargetHostVariantRuntimeMetadata),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetDirectRuntimeMetadata {
    pub launch_actions: Vec<TargetLaunchAction>,
    pub external_dependencies: Vec<ExternalProductDependencyRequirement>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetHostVariantRuntimeMetadata {
    pub host_variants: Vec<TargetHostVariant>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetHostVariant {
    pub variant_id: String,
    pub host_tag_requirements: Vec<String>,
    pub launch_actions: Vec<TargetLaunchAction>,
    pub external_dependencies: Vec<ExternalProductDependencyRequirement>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetLaunchAction {
    pub action_name: String,
    pub executable_path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command_arguments: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory_path: Option<String>,
}

impl TargetProductMetadata {
    pub fn validate_structure(&self) -> crate::Result<()> {
        if self.document_version != FORMAT_VERSION {
            return Err(crate::Error::manifest(
                "target product metadata document_version is unsupported",
            ));
        }
        match &self.runtime {
            TargetRuntimeMetadata::Direct(runtime) => {
                validate_runtime_actions(&runtime.launch_actions)?;
                validate_external_product_dependency_requirements(&runtime.external_dependencies)?;
            }
            TargetRuntimeMetadata::HostVariants(runtime) => {
                if runtime.host_variants.is_empty() {
                    return Err(crate::Error::manifest(
                        "target product metadata host_variants must not be empty",
                    ));
                }
                let mut variant_ids = std::collections::HashSet::new();
                for variant in &runtime.host_variants {
                    if !is_runtime_identifier(&variant.variant_id)
                        || !variant_ids.insert(variant.variant_id.as_str())
                    {
                        return Err(crate::Error::manifest(
                            "target product metadata variant_id is invalid or duplicated",
                        ));
                    }
                    if variant.host_tag_requirements.is_empty()
                        || variant.host_tag_requirements.iter().any(|tag| tag.trim().is_empty())
                    {
                        return Err(crate::Error::manifest(
                            "target product metadata host_tag_requirements must not be empty",
                        ));
                    }
                    validate_runtime_actions(&variant.launch_actions)?;
                    validate_external_product_dependency_requirements(
                        &variant.external_dependencies,
                    )?;
                }
            }
        }
        Ok(())
    }

    pub fn from_launch(
        launch: &launch::ResolvedLaunchConfiguration,
        dependencies: Vec<ExternalProductDependencyRequirement>,
    ) -> Self {
        Self {
            document_version: FORMAT_VERSION.to_string(),
            runtime: TargetRuntimeMetadata::Direct(TargetDirectRuntimeMetadata {
                launch_actions: launch
                    .actions
                    .iter()
                    .map(|action| TargetLaunchAction {
                        action_name: action.name.clone(),
                        executable_path: action.executable.clone(),
                        command_arguments: action.arguments.clone(),
                        working_directory_path: action.working_directory.clone(),
                    })
                    .collect(),
                external_dependencies: dependencies,
            }),
        }
    }

    pub fn launch_for_host_tags(
        &self,
        host_tags: &TargetTagSet,
    ) -> Option<launch::ResolvedLaunchConfiguration> {
        let launch_actions = match &self.runtime {
            TargetRuntimeMetadata::Direct(runtime) => &runtime.launch_actions,
            TargetRuntimeMetadata::HostVariants(_) => &self.matching_host_variant(host_tags)?.launch_actions,
        };
        Some(launch::ResolvedLaunchConfiguration {
            primary: launch_actions
                .first()
                .map(|action| action.action_name.clone()),
            actions: launch_actions
                .iter()
                .map(|action| launch::ResolvedLaunchAction {
                    name: action.action_name.clone(),
                    executable: action.executable_path.clone(),
                    arguments: action.command_arguments.clone(),
                    working_directory: action.working_directory_path.clone(),
                })
                .collect(),
        })
    }

    pub fn external_dependencies_for_host_tags(
        &self,
        host_tags: &TargetTagSet,
    ) -> Vec<ExternalProductDependencyRequirement> {
        match &self.runtime {
            TargetRuntimeMetadata::Direct(runtime) => runtime.external_dependencies.clone(),
            TargetRuntimeMetadata::HostVariants(_) => self
                .matching_host_variant(host_tags)
                .map(|variant| variant.external_dependencies.clone())
                .unwrap_or_default(),
        }
    }

    fn matching_host_variant(&self, host_tags: &TargetTagSet) -> Option<&TargetHostVariant> {
        let TargetRuntimeMetadata::HostVariants(runtime) = &self.runtime else {
            return None;
        };
        let canonical_tags = host_tags.to_canonical_strings();
        let mut matches = runtime.host_variants.iter().filter(|variant| {
            variant
                .host_tag_requirements
                .iter()
                .all(|requirement| canonical_tags.contains(requirement))
        });
        let variant = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(variant)
    }
}

fn validate_runtime_actions(actions: &[TargetLaunchAction]) -> crate::Result<()> {
    if actions.is_empty() {
        return Err(crate::Error::manifest(
            "target product metadata launch_actions must not be empty",
        ));
    }
    let mut action_names = std::collections::HashSet::new();
    for action in actions {
        if action.action_name.trim().is_empty()
            || !action_names.insert(action.action_name.trim().to_ascii_lowercase())
        {
            return Err(crate::Error::manifest(
                "target product metadata action_name is empty or duplicated",
            ));
        }
    }
    Ok(())
}

fn is_runtime_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_lowercase())
        && value.len() <= 64
        && characters.all(|character| character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '-')
}

/// Returns the launcher host capabilities used by target product host variants.
/// These tags are runtime facts and are never written into target metadata.
pub fn current_host_tags() -> TargetTagSet {
    let mut tags = TargetTagSet::default();
    let operating_system = match std::env::consts::OS {
        "windows" => "windows",
        "linux" => "linux",
        "macos" => "macos",
        _ => "unknown",
    };
    let architecture = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "x86" => "x86_32",
        "aarch64" => "arm64",
        _ => "unknown",
    };
    tags.upsert_exclusive("os", operating_system);
    tags.upsert_exclusive("arch", architecture);
    tags
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetManifest {
    #[serde(rename = "target_metadata_version")]
    pub manifest_version: String,
    #[serde(skip)]
    pub target_id: String,
    #[serde(rename = "inserted_at")]
    pub inserted_at: String,
    /// Max source file mtime (UTC) recorded when this target was last synced from a source folder.
    #[serde(skip)]
    pub source_modified_at: String,
    #[serde(skip)]
    pub total_logical_size: u64,
    #[serde(skip)]
    pub total_compressed_size_estimate: u64,
    #[serde(
        default,
        serialize_with = "serialize_target_tags_as_compact_strings",
        deserialize_with = "deserialize_target_tags_from_compact_strings"
    )]
    pub target_tags: TargetTagSet,
    #[serde(skip)]
    pub launch: Option<launch::ResolvedLaunchConfiguration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_metadata: Option<TargetProductMetadata>,
    #[serde(skip)]
    pub files: Vec<FileManifest>,
}

impl TargetManifest {
    pub fn new(target_id: String, target_tags: TargetTagSet) -> Self {
        TargetManifest {
            manifest_version: FORMAT_VERSION.to_string(),
            target_id,
            inserted_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            source_modified_at: String::new(),
            total_logical_size: 0,
            total_compressed_size_estimate: 0,
            target_tags,
            launch: None,
            product_metadata: None,
            files: Vec::new(),
        }
    }

    pub fn sync_product_metadata_from_launch(&mut self) {
        if self.product_metadata.is_some() {
            return;
        }
        if let Some(launch) = self.launch.as_ref() {
            self.product_metadata = Some(TargetProductMetadata::from_launch(launch, Vec::new()));
        }
    }

    pub fn sync_launch_from_product_metadata(&mut self) {
        let host_tags = current_host_tags();
        self.launch = self
            .product_metadata
            .as_ref()
            .and_then(|metadata| metadata.launch_for_host_tags(&host_tags));
    }
}

pub(crate) fn serialize_target_tags_as_compact_strings<S>(
    target_tags: &TargetTagSet,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    target_tags.to_canonical_strings().serialize(serializer)
}

pub(crate) fn deserialize_target_tags_from_compact_strings<'de, D>(
    deserializer: D,
) -> std::result::Result<TargetTagSet, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<String>::deserialize(deserializer)?;
    TargetTagSet::from_compact_strings(&values).map_err(serde::de::Error::custom)
}

#[cfg(test)]
#[path = "target_tests.rs"]
mod tests;
