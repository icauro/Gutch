//! Snapshot manifest and metadata: product, runtime host variants and launch actions.

use crate::artifact::profiles::Chunker;
use serde::Deserialize;
use serde::Serialize;

use super::{
    ExternalProductDependencyRequirement, FileManifest, PUBLIC_METADATA_FORMAT_VERSION,
    SnapshotEntry, SnapshotTagSet, launch, validate_external_product_dependency_requirements,
};
use std::collections::HashSet;
use std::env::consts::ARCH;
use std::env::consts::OS;
use std::slice::from_ref;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SnapshotProductMetadata {
    #[serde(rename = "document_version")]
    pub document_version: String,
    pub runtime: SnapshotRuntimeMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum SnapshotRuntimeMetadata {
    Direct(SnapshotDirectRuntimeMetadata),
    HostVariants(SnapshotHostVariantRuntimeMetadata),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SnapshotDirectRuntimeMetadata {
    pub launch_actions: Vec<SnapshotLaunchAction>,
    pub external_dependencies: Vec<ExternalProductDependencyRequirement>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SnapshotHostVariantRuntimeMetadata {
    pub host_variants: Vec<SnapshotHostVariant>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SnapshotHostVariant {
    pub variant_id: String,
    pub host_tag_requirements: Vec<String>,
    pub launch_actions: Vec<SnapshotLaunchAction>,
    pub external_dependencies: Vec<ExternalProductDependencyRequirement>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SnapshotLaunchAction {
    pub action_name: String,
    pub executable_path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command_arguments: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory_path: Option<String>,
}

impl SnapshotProductMetadata {
    /// Parses a product metadata document and rejects structurally invalid ones.
    pub(crate) fn from_json_bytes(bytes: &[u8]) -> crate::Result<Self> {
        let metadata: Self = crate::filesystem::from_json_bytes(bytes)?;
        metadata.validate_structure()?;
        Ok(metadata)
    }

    pub fn validate_structure(&self) -> crate::Result<()> {
        if self.document_version != PUBLIC_METADATA_FORMAT_VERSION {
            return Err(crate::Error::manifest(
                "snapshot product metadata document_version is unsupported",
            ));
        }
        match &self.runtime {
            SnapshotRuntimeMetadata::Direct(runtime) => {
                validate_runtime_actions(&runtime.launch_actions, true)?;
                validate_external_product_dependency_requirements(&runtime.external_dependencies)?;
            }
            SnapshotRuntimeMetadata::HostVariants(runtime) => {
                if runtime.host_variants.is_empty() {
                    return Err(crate::Error::manifest(
                        "snapshot product metadata host_variants must not be empty",
                    ));
                }
                let mut variant_ids = HashSet::new();
                for variant in &runtime.host_variants {
                    if !is_runtime_identifier(&variant.variant_id)
                        || !variant_ids.insert(variant.variant_id.as_str())
                    {
                        return Err(crate::Error::manifest(
                            "snapshot product metadata variant_id is invalid or duplicated",
                        ));
                    }
                    if variant.host_tag_requirements.is_empty() {
                        return Err(crate::Error::manifest(
                            "snapshot product metadata host_tag_requirements must not be empty",
                        ));
                    }
                    // Matching compares canonical `key=value` strings exactly, so a
                    // non-canonical requirement would silently never match any host.
                    for requirement in &variant.host_tag_requirements {
                        let canonical =
                            SnapshotTagSet::from_compact_strings(from_ref(requirement))?
                                .to_canonical_strings();
                        if canonical != [requirement.as_str()] {
                            return Err(crate::Error::manifest(format!(
                                "snapshot product metadata host tag requirement '{requirement}' must be canonical key=value"
                            )));
                        }
                    }
                    validate_runtime_actions(&variant.launch_actions, true)?;
                    validate_external_product_dependency_requirements(
                        &variant.external_dependencies,
                    )?;
                }
                validate_host_variants_are_exclusive(&runtime.host_variants)?;
            }
        }
        Ok(())
    }

    pub fn from_launch(
        launch: &launch::ResolvedLaunchConfiguration,
        dependencies: Vec<ExternalProductDependencyRequirement>,
    ) -> Self {
        Self {
            document_version: PUBLIC_METADATA_FORMAT_VERSION.to_string(),
            runtime: SnapshotRuntimeMetadata::Direct(SnapshotDirectRuntimeMetadata {
                launch_actions: launch
                    .actions
                    .iter()
                    .map(|action| SnapshotLaunchAction {
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
        host_tags: &SnapshotTagSet,
    ) -> Option<launch::ResolvedLaunchConfiguration> {
        let launch_actions = match &self.runtime {
            SnapshotRuntimeMetadata::Direct(runtime) => &runtime.launch_actions,
            SnapshotRuntimeMetadata::HostVariants(_) => {
                &self.matching_host_variant(host_tags)?.launch_actions
            }
        };
        Some(resolved_launch_configuration(launch_actions))
    }

    pub fn validate_for_snapshot(&self, snapshot: &SnapshotManifest) -> crate::Result<()> {
        self.validate_structure()?;
        match &self.runtime {
            SnapshotRuntimeMetadata::Direct(runtime) => {
                validate_snapshot_launch_actions_for_snapshot(
                    &runtime.launch_actions,
                    snapshot,
                    None,
                )?;
            }
            SnapshotRuntimeMetadata::HostVariants(runtime) => {
                for variant in &runtime.host_variants {
                    let target_os = variant
                        .host_tag_requirements
                        .iter()
                        .find_map(|requirement| requirement.strip_prefix("os="));
                    validate_snapshot_launch_actions_for_snapshot(
                        &variant.launch_actions,
                        snapshot,
                        target_os,
                    )?;
                }
            }
        }
        Ok(())
    }

    pub fn external_dependencies_for_host_tags(
        &self,
        host_tags: &SnapshotTagSet,
    ) -> Vec<ExternalProductDependencyRequirement> {
        match &self.runtime {
            SnapshotRuntimeMetadata::Direct(runtime) => runtime.external_dependencies.clone(),
            SnapshotRuntimeMetadata::HostVariants(_) => self
                .matching_host_variant(host_tags)
                .map(|variant| variant.external_dependencies.clone())
                .unwrap_or_default(),
        }
    }

    fn matching_host_variant(&self, host_tags: &SnapshotTagSet) -> Option<&SnapshotHostVariant> {
        let SnapshotRuntimeMetadata::HostVariants(runtime) = &self.runtime else {
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

/// A host has one value per tag key, so two variants are exclusive only when
/// they require different values for a shared key. Otherwise one host could
/// match both and launch resolution would be ambiguous.
fn validate_host_variants_are_exclusive(variants: &[SnapshotHostVariant]) -> crate::Result<()> {
    fn requirements(variant: &SnapshotHostVariant) -> Vec<(&str, &str)> {
        variant
            .host_tag_requirements
            .iter()
            .filter_map(|requirement| requirement.split_once('='))
            .collect()
    }
    for (index, first) in variants.iter().enumerate() {
        let first_requirements = requirements(first);
        for second in &variants[index + 1..] {
            let exclusive = requirements(second).iter().any(|(key, value)| {
                first_requirements
                    .iter()
                    .any(|(other_key, other_value)| other_key == key && other_value != value)
            });
            if !exclusive {
                return Err(crate::Error::manifest(format!(
                    "snapshot product metadata host variants '{}' and '{}' can match the same host",
                    first.variant_id, second.variant_id
                )));
            }
        }
    }
    Ok(())
}

fn resolved_launch_configuration(
    actions: &[SnapshotLaunchAction],
) -> launch::ResolvedLaunchConfiguration {
    launch::ResolvedLaunchConfiguration {
        primary: actions.first().map(|action| action.action_name.clone()),
        actions: actions
            .iter()
            .map(|action| launch::ResolvedLaunchAction {
                name: action.action_name.clone(),
                executable: action.executable_path.clone(),
                arguments: action.command_arguments.clone(),
                working_directory: action.working_directory_path.clone(),
            })
            .collect(),
    }
}

fn validate_snapshot_launch_actions_for_snapshot(
    actions: &[SnapshotLaunchAction],
    snapshot: &SnapshotManifest,
    target_os: Option<&str>,
) -> crate::Result<()> {
    let configuration = resolved_launch_configuration(actions);
    launch::validate_resolved_launch_for_snapshot_os(&configuration, snapshot, target_os)
}

pub(crate) fn validate_runtime_actions(
    actions: &[SnapshotLaunchAction],
    require_actions: bool,
) -> crate::Result<()> {
    if require_actions && actions.is_empty() {
        return Err(crate::Error::manifest(
            "snapshot product metadata launch_actions must not be empty",
        ));
    }
    let mut action_names = HashSet::new();
    for action in actions {
        if action.action_name.trim().is_empty()
            || !action_names.insert(action.action_name.trim().to_ascii_lowercase())
        {
            return Err(crate::Error::manifest(
                "snapshot product metadata action_name is empty or duplicated",
            ));
        }
    }
    Ok(())
}

fn is_runtime_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_lowercase())
        && value.len() <= 64
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

/// Returns the launcher host capabilities used by snapshot product host variants.
/// These tags are runtime facts and are never written into snapshot metadata.
pub(crate) fn current_host_tags() -> SnapshotTagSet {
    let mut tags = SnapshotTagSet::default();
    let operating_system = match OS {
        "windows" => "windows",
        "linux" => "linux",
        "macos" => "macos",
        _ => "unknown",
    };
    let architecture = match ARCH {
        "x86_64" => "x86_64",
        "x86" => "x86_32",
        "aarch64" => "arm64",
        _ => "unknown",
    };
    tags.insert("os", operating_system).unwrap();
    tags.insert("arch", architecture).unwrap();
    tags
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SnapshotMetadata {
    #[serde(default)]
    pub chunker: Chunker,
    #[serde(
        rename = "snapshot_metadata_version",
        deserialize_with = "super::deserialize_document_version"
    )]
    pub metadata_version: String,
    #[serde(skip)]
    pub snapshot_id: String,
    #[serde(rename = "captured_at")]
    pub captured_at: String,
    #[serde(default, rename = "tags")]
    pub snapshot_tags: SnapshotTagSet,
}

#[derive(Debug, Clone)]
pub(crate) struct SnapshotBlueprint {
    pub entries: Vec<SnapshotEntry>,
}

#[derive(Debug, Clone)]
pub struct SnapshotManifest {
    pub chunker: Chunker,
    pub metadata_version: String,
    pub snapshot_id: String,
    pub captured_at: String,
    /// Max source file mtime (UTC) recorded when this snapshot was last captured from a source folder.
    pub source_modified_at: String,
    pub total_logical_size: u64,
    pub total_compressed_size_estimate: u64,
    pub snapshot_tags: SnapshotTagSet,
    pub launch: Option<launch::ResolvedLaunchConfiguration>,
    pub product_metadata: Option<SnapshotProductMetadata>,
    pub entries: Vec<SnapshotEntry>,
}

impl SnapshotManifest {
    pub fn new(snapshot_id: String, snapshot_tags: SnapshotTagSet) -> Self {
        SnapshotManifest {
            chunker: Chunker::default(),
            metadata_version: PUBLIC_METADATA_FORMAT_VERSION.to_string(),
            snapshot_id,
            captured_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            source_modified_at: String::new(),
            total_logical_size: 0,
            total_compressed_size_estimate: 0,
            snapshot_tags,
            launch: None,
            product_metadata: None,
            entries: Vec::new(),
        }
    }

    pub(crate) fn metadata(&self) -> SnapshotMetadata {
        SnapshotMetadata {
            chunker: self.chunker,
            metadata_version: self.metadata_version.clone(),
            snapshot_id: self.snapshot_id.clone(),
            captured_at: self.captured_at.clone(),
            snapshot_tags: self.snapshot_tags.clone(),
        }
    }

    pub(crate) fn blueprint(&self) -> SnapshotBlueprint {
        SnapshotBlueprint {
            entries: self.entries.clone(),
        }
    }

    pub(crate) fn from_metadata(metadata: SnapshotMetadata) -> Self {
        Self {
            chunker: metadata.chunker,
            metadata_version: metadata.metadata_version,
            snapshot_id: metadata.snapshot_id,
            captured_at: metadata.captured_at,
            source_modified_at: String::new(),
            total_logical_size: 0,
            total_compressed_size_estimate: 0,
            snapshot_tags: metadata.snapshot_tags,
            launch: None,
            product_metadata: None,
            entries: Vec::new(),
        }
    }

    pub fn sync_product_metadata_from_launch(&mut self) {
        if self.product_metadata.is_some() {
            return;
        }
        if let Some(launch) = self.launch.as_ref() {
            self.product_metadata = Some(SnapshotProductMetadata::from_launch(launch, Vec::new()));
        }
    }

    pub fn sync_launch_from_product_metadata(&mut self) {
        let host_tags = current_host_tags();
        self.launch = self
            .product_metadata
            .as_ref()
            .and_then(|metadata| metadata.launch_for_host_tags(&host_tags));
    }

    pub fn files(&self) -> impl Iterator<Item = &FileManifest> {
        self.entries.iter().filter_map(SnapshotEntry::as_file)
    }

    pub fn file_count(&self) -> usize {
        self.files().count()
    }

    pub fn push_file(&mut self, file: FileManifest) {
        self.entries.push(SnapshotEntry::File(file));
    }

    pub fn set_files(&mut self, files: Vec<FileManifest>) {
        self.entries = files.into_iter().map(SnapshotEntry::File).collect();
    }
}

#[cfg(test)]
#[path = "snapshot_tests.rs"]
mod tests;
