pub mod binary;
pub mod gutch_repo;
pub mod target;

use std::collections::{BTreeMap, BTreeSet};

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: &str = "1.0.0";
pub const CHUNKER_FASTCDC: &str = "fastcdc-v2020";
pub const CHUNK_HASH_BLAKE3: &str = "blake3";
pub const COMPRESSION_ZSTD: &str = "zstd";
pub const COMPRESSION_UNIT_BUNDLE: &str = "bundle";
pub const CONTENT_REPO_FORMAT: &str = "gutch_repo";
pub const CONTENT_REPO_MANIFEST_FILE: &str = "repo_manifest.json";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
pub enum ArtifactSizingPreset {
    #[serde(rename = "chunk_1KiB")]
    #[value(name = "chunk_1KiB")]
    Chunk1KiB,
    #[serde(rename = "chunk_4KiB")]
    #[value(name = "chunk_4KiB")]
    Chunk4KiB,
    #[serde(rename = "chunk_16KiB")]
    #[value(name = "chunk_16KiB")]
    Chunk16KiB,
    #[serde(rename = "chunk_32KiB")]
    #[value(name = "chunk_32KiB")]
    Chunk32KiB,
    #[serde(rename = "chunk_64KiB")]
    #[value(name = "chunk_64KiB")]
    Chunk64KiB,
    #[serde(rename = "chunk_128KiB")]
    #[value(name = "chunk_128KiB")]
    Chunk128KiB,
    #[serde(rename = "chunk_256KiB")]
    #[value(name = "chunk_256KiB")]
    Chunk256KiB,
    #[serde(rename = "chunk_512KiB")]
    #[value(name = "chunk_512KiB")]
    Chunk512KiB,
    #[serde(rename = "chunk_1MiB")]
    #[value(name = "chunk_1MiB")]
    Chunk1MiB,
}

impl Default for ArtifactSizingPreset {
    fn default() -> Self {
        Self::Chunk128KiB
    }
}

impl ArtifactSizingPreset {
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Chunk1KiB => "chunk_1KiB",
            Self::Chunk4KiB => "chunk_4KiB",
            Self::Chunk16KiB => "chunk_16KiB",
            Self::Chunk32KiB => "chunk_32KiB",
            Self::Chunk64KiB => "chunk_64KiB",
            Self::Chunk128KiB => "chunk_128KiB",
            Self::Chunk256KiB => "chunk_256KiB",
            Self::Chunk512KiB => "chunk_512KiB",
            Self::Chunk1MiB => "chunk_1MiB",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
pub enum BundleSizingPreset {
    #[serde(rename = "auto")]
    #[value(name = "auto")]
    Auto,
    #[serde(rename = "bundle_4MiB")]
    #[value(name = "bundle_4MiB")]
    Bundle4MiB,
    #[serde(rename = "bundle_8MiB")]
    #[value(name = "bundle_8MiB")]
    Bundle8MiB,
    #[serde(rename = "bundle_16MiB")]
    #[value(name = "bundle_16MiB")]
    Bundle16MiB,
    #[serde(rename = "bundle_32MiB")]
    #[value(name = "bundle_32MiB")]
    Bundle32MiB,
    #[serde(rename = "bundle_64MiB")]
    #[value(name = "bundle_64MiB")]
    Bundle64MiB,
    #[serde(rename = "bundle_128MiB")]
    #[value(name = "bundle_128MiB")]
    Bundle128MiB,
}

impl Default for BundleSizingPreset {
    fn default() -> Self {
        Self::Bundle16MiB
    }
}

impl BundleSizingPreset {
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Bundle4MiB => "bundle_4MiB",
            Self::Bundle8MiB => "bundle_8MiB",
            Self::Bundle16MiB => "bundle_16MiB",
            Self::Bundle32MiB => "bundle_32MiB",
            Self::Bundle64MiB => "bundle_64MiB",
            Self::Bundle128MiB => "bundle_128MiB",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkSizingProfile {
    pub preset: ArtifactSizingPreset,
    pub chunker: String,
    pub min_chunk_bytes: u32,
    pub avg_chunk_bytes: u32,
    pub max_chunk_bytes: u32,
    pub gear_table_id: u32,
    pub chunk_hash_algorithm: String,
    pub format_version: String,
}

impl Default for ChunkSizingProfile {
    fn default() -> Self {
        Self::for_preset(ArtifactSizingPreset::Chunk128KiB)
    }
}

impl ChunkSizingProfile {
    pub fn for_preset(preset: ArtifactSizingPreset) -> Self {
        let (min_chunk_bytes, avg_chunk_bytes, max_chunk_bytes) = match preset {
            ArtifactSizingPreset::Chunk1KiB => (256, 1024, 4 * 1024),
            ArtifactSizingPreset::Chunk4KiB => (1024, 4 * 1024, 16 * 1024),
            ArtifactSizingPreset::Chunk16KiB => (4 * 1024, 16 * 1024, 64 * 1024),
            ArtifactSizingPreset::Chunk32KiB => (8 * 1024, 32 * 1024, 128 * 1024),
            ArtifactSizingPreset::Chunk64KiB => (16 * 1024, 64 * 1024, 256 * 1024),
            ArtifactSizingPreset::Chunk128KiB => (32 * 1024, 128 * 1024, 512 * 1024),
            ArtifactSizingPreset::Chunk256KiB => (64 * 1024, 256 * 1024, 1024 * 1024),
            ArtifactSizingPreset::Chunk512KiB => (128 * 1024, 512 * 1024, 2 * 1024 * 1024),
            ArtifactSizingPreset::Chunk1MiB => (256 * 1024, 1024 * 1024, 4 * 1024 * 1024),
        };

        ChunkSizingProfile {
            preset,
            chunker: CHUNKER_FASTCDC.to_string(),
            min_chunk_bytes,
            avg_chunk_bytes,
            max_chunk_bytes,
            gear_table_id: 3,
            chunk_hash_algorithm: CHUNK_HASH_BLAKE3.to_string(),
            format_version: FORMAT_VERSION.to_string(),
        }
    }

    pub fn min_chunk_size_bytes(&self) -> u32 {
        self.min_chunk_bytes
    }

    pub fn avg_chunk_size_bytes(&self) -> u32 {
        self.avg_chunk_bytes
    }

    pub fn max_chunk_size_bytes(&self) -> u32 {
        self.max_chunk_bytes
    }

    pub fn auto_preset_for_median_file_size_bytes(
        _median_file_size_bytes: u64,
    ) -> ArtifactSizingPreset {
        ArtifactSizingPreset::Chunk128KiB
    }

    pub fn for_selection(preset: ArtifactSizingPreset, _median_file_size_bytes: u64) -> Self {
        Self::for_preset(preset)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleSizingProfile {
    pub preset: BundleSizingPreset,
    #[serde(alias = "bundle_max_mibibytes")]
    pub bundle_max_mib: u64,
    #[serde(alias = "bundle_min_preferred_mibibytes")]
    pub bundle_preferred_mib: u64,
}

impl BundleSizingProfile {
    pub fn for_preset(preset: BundleSizingPreset) -> Self {
        let (bundle_max_mib, bundle_preferred_mib) = match preset {
            BundleSizingPreset::Auto | BundleSizingPreset::Bundle16MiB => (32, 16),
            BundleSizingPreset::Bundle4MiB => (8, 4),
            BundleSizingPreset::Bundle8MiB => (16, 8),
            BundleSizingPreset::Bundle32MiB => (64, 32),
            BundleSizingPreset::Bundle64MiB => (128, 64),
            BundleSizingPreset::Bundle128MiB => (256, 128),
        };

        BundleSizingProfile {
            preset,
            bundle_max_mib,
            bundle_preferred_mib,
        }
    }

    pub fn bundle_max_size_bytes(&self) -> u64 {
        self.bundle_max_mib * 1024 * 1024
    }

    pub fn bundle_preferred_size_bytes(&self) -> u64 {
        self.bundle_preferred_mib * 1024 * 1024
    }

    pub fn auto_preset_for_target_logical_size_bytes(
        _target_logical_size_bytes: u64,
    ) -> BundleSizingPreset {
        BundleSizingPreset::Bundle16MiB
    }

    pub fn for_selection(preset: BundleSizingPreset, target_logical_size_bytes: u64) -> Self {
        let resolved = match preset {
            BundleSizingPreset::Auto => {
                Self::auto_preset_for_target_logical_size_bytes(target_logical_size_bytes)
            }
            other => other,
        };
        Self::for_preset(resolved)
    }
}

impl Default for BundleSizingProfile {
    fn default() -> Self {
        Self::for_preset(BundleSizingPreset::Bundle16MiB)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
pub enum CompressionPreset {
    #[serde(rename = "uncompressed")]
    #[value(name = "uncompressed")]
    Uncompressed,
    #[serde(rename = "zstd_1")]
    #[value(name = "zstd_1")]
    Zstd1,
    #[serde(rename = "zstd_3")]
    #[value(name = "zstd_3")]
    Zstd3,
    #[serde(rename = "zstd_5")]
    #[value(name = "zstd_5")]
    Zstd5,
}

impl Default for CompressionPreset {
    fn default() -> Self {
        Self::Zstd3
    }
}

impl CompressionPreset {
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Uncompressed => "uncompressed",
            Self::Zstd1 => "zstd_1",
            Self::Zstd3 => "zstd_3",
            Self::Zstd5 => "zstd_5",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionProfile {
    pub preset: CompressionPreset,
    pub compression: String,
    pub compression_unit: String,
    pub zstd_level: i32,
}

impl CompressionProfile {
    pub fn for_preset(preset: CompressionPreset) -> Self {
        let (compression, zstd_level) = match preset {
            CompressionPreset::Uncompressed => ("none".to_string(), 0),
            CompressionPreset::Zstd1 => (COMPRESSION_ZSTD.to_string(), 1),
            CompressionPreset::Zstd3 => (COMPRESSION_ZSTD.to_string(), 3),
            CompressionPreset::Zstd5 => (COMPRESSION_ZSTD.to_string(), 5),
        };
        CompressionProfile {
            preset,
            compression,
            compression_unit: COMPRESSION_UNIT_BUNDLE.to_string(),
            zstd_level,
        }
    }
}

impl Default for CompressionProfile {
    fn default() -> Self {
        Self::for_preset(CompressionPreset::Zstd3)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TargetTagSet {
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub plain: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scoped: BTreeMap<String, BTreeSet<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub exclusive: BTreeMap<String, String>,
}

impl TargetTagSet {
    pub fn insert_plain(&mut self, value: impl Into<String>) {
        let value = value.into();
        if !value.is_empty() {
            self.plain.insert(value);
        }
    }

    pub fn insert_scoped(&mut self, prefix: impl Into<String>, value: impl Into<String>) {
        let prefix = prefix.into();
        let value = value.into();
        if prefix.is_empty() || value.is_empty() {
            return;
        }
        self.scoped.entry(prefix).or_default().insert(value);
    }

    pub fn insert_exclusive_if_missing(
        &mut self,
        prefix: impl Into<String>,
        value: impl Into<String>,
    ) {
        let prefix = prefix.into();
        let value = value.into();
        if prefix.is_empty() || value.is_empty() {
            return;
        }
        self.exclusive.entry(prefix).or_insert(value);
    }

    pub fn upsert_exclusive(&mut self, prefix: impl Into<String>, value: impl Into<String>) {
        let prefix = prefix.into();
        let value = value.into();
        if prefix.is_empty() || value.is_empty() {
            return;
        }
        self.exclusive.insert(prefix, value);
    }

    pub fn has_exclusive(&self, prefix: &str) -> bool {
        self.exclusive.contains_key(prefix)
    }

    pub fn get_exclusive(&self, prefix: &str) -> Option<&str> {
        self.exclusive.get(prefix).map(String::as_str)
    }

    pub fn extend_missing_from(&mut self, other: &Self) {
        for value in &other.plain {
            self.plain.insert(value.clone());
        }
        for (prefix, values) in &other.scoped {
            self.scoped
                .entry(prefix.clone())
                .or_default()
                .extend(values.iter().cloned());
        }
        for (prefix, value) in &other.exclusive {
            self.exclusive
                .entry(prefix.clone())
                .or_insert_with(|| value.clone());
        }
    }

    pub fn to_canonical_strings(&self) -> Vec<String> {
        let mut rendered = Vec::new();
        rendered.extend(self.plain.iter().cloned());
        for (prefix, values) in &self.scoped {
            for value in values {
                rendered.push(format!("{}:{}", prefix, value));
            }
        }
        for (prefix, value) in &self.exclusive {
            rendered.push(format!("{}::{}", prefix, value));
        }
        rendered.sort();
        rendered
    }

    pub fn to_ordered_strings(&self, schema: Option<&TargetTagSchema>) -> Vec<String> {
        let mut rendered = Vec::new();
        let mut consumed_plain = BTreeSet::new();
        let mut consumed_scoped = BTreeSet::new();
        let mut consumed_exclusive = BTreeSet::new();

        if let Some(schema) = schema {
            for definition in &schema.target_tags {
                match definition.kind {
                    TargetTagKind::Plain => {
                        if self.plain.contains(&definition.name) {
                            rendered.push(definition.name.clone());
                            consumed_plain.insert(definition.name.clone());
                        }
                    }
                    TargetTagKind::Scoped => {
                        if let Some(values) = self.scoped.get(&definition.name) {
                            for value in values {
                                rendered.push(format!("{}:{}", definition.name, value));
                            }
                            consumed_scoped.insert(definition.name.clone());
                        }
                    }
                    TargetTagKind::Exclusive => {
                        if let Some(value) = self.exclusive.get(&definition.name) {
                            rendered.push(format!("{}::{}", definition.name, value));
                            consumed_exclusive.insert(definition.name.clone());
                        }
                    }
                }
            }
        }

        for value in self
            .plain
            .iter()
            .filter(|value| !consumed_plain.contains(*value))
        {
            rendered.push(value.clone());
        }
        for (prefix, values) in self
            .scoped
            .iter()
            .filter(|(prefix, _)| !consumed_scoped.contains(*prefix))
        {
            for value in values {
                rendered.push(format!("{}:{}", prefix, value));
            }
        }
        for (prefix, value) in self
            .exclusive
            .iter()
            .filter(|(prefix, _)| !consumed_exclusive.contains(*prefix))
        {
            rendered.push(format!("{}::{}", prefix, value));
        }

        rendered
    }

    pub fn from_compact_strings(tag_values: &[String]) -> crate::Result<Self> {
        let mut parsed_tags = Self::default();
        for tag_value in tag_values {
            parsed_tags.apply_compact_string(tag_value, true)?;
        }
        Ok(parsed_tags)
    }

    pub fn apply_compact_string(
        &mut self,
        raw_tag: &str,
        strict_exclusive: bool,
    ) -> crate::Result<()> {
        let trimmed = raw_tag.trim();
        if trimmed.is_empty() {
            return Err(crate::Error::manifest(
                "invalid empty tag value".to_string(),
            ));
        }

        if let Some((prefix, value)) = trimmed.split_once("::") {
            if prefix.is_empty() || value.is_empty() || value.contains("::") {
                return Err(crate::Error::manifest(format!(
                    "invalid tag '{}'. Use plain-tag, scoped:value, or exclusive::value",
                    raw_tag
                )));
            }
            if strict_exclusive {
                if let Some(existing_value) = self.exclusive.get(prefix) {
                    if existing_value != value {
                        return Err(crate::Error::manifest(format!(
                            "conflicting exclusive tag '{}'. Prefix '{}' already has value '{}'",
                            raw_tag, prefix, existing_value
                        )));
                    }
                }
            }
            self.upsert_exclusive(prefix.to_string(), value.to_string());
            return Ok(());
        }

        if trimmed.contains(':') {
            let Some((prefix, value)) = trimmed.split_once(':') else {
                return Err(crate::Error::manifest(format!(
                    "invalid tag '{}'. Use plain-tag, scoped:value, or exclusive::value",
                    raw_tag
                )));
            };
            if prefix.is_empty() || value.is_empty() || value.contains(':') {
                return Err(crate::Error::manifest(format!(
                    "invalid tag '{}'. Use plain-tag, scoped:value, or exclusive::value",
                    raw_tag
                )));
            }
            self.insert_scoped(prefix.to_string(), value.to_string());
            return Ok(());
        }

        self.insert_plain(trimmed.to_string());
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TargetTagKind {
    Plain,
    Scoped,
    Exclusive,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VersionScheme {
    Semver,
    Calver,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredVersion {
    pub scheme: VersionScheme,
    pub major_or_year: u32,
    pub minor_or_month: u32,
    pub patch_or_day: u32,
    pub micro: u32,
}

impl StructuredVersion {
    pub fn parse(raw: &str) -> crate::Result<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(crate::Error::manifest("version cannot be empty"));
        }

        let candidate = trimmed
            .strip_prefix('v')
            .or_else(|| trimmed.strip_prefix('V'))
            .unwrap_or(trimmed);
        let parts: Vec<&str> = candidate.split('.').collect();
        if parts.is_empty()
            || parts.len() > 4
            || parts.iter().any(|part| {
                part.is_empty() || !part.chars().all(|character| character.is_ascii_digit())
            })
        {
            return Err(crate::Error::manifest(format!("invalid version '{}'", raw)));
        }

        let parsed_parts = parts
            .iter()
            .map(|part| {
                part.parse::<u32>()
                    .map_err(|_| crate::Error::manifest(format!("invalid version '{}'", raw)))
            })
            .collect::<crate::Result<Vec<_>>>()?;

        if parsed_parts.len() == 3 {
            if parts[0].len() == 4
                && parsed_parts[1] >= 1
                && parsed_parts[1] <= 12
                && parsed_parts[2] >= 1
                && parsed_parts[2] <= 31
            {
                return Ok(Self {
                    scheme: VersionScheme::Calver,
                    major_or_year: parsed_parts[0],
                    minor_or_month: parsed_parts[1],
                    patch_or_day: parsed_parts[2],
                    micro: 0,
                });
            }

            return Ok(Self {
                scheme: VersionScheme::Semver,
                major_or_year: parsed_parts[0],
                minor_or_month: parsed_parts[1],
                patch_or_day: parsed_parts[2],
                micro: 0,
            });
        }

        if parsed_parts.len() == 4
            && parts[0].len() == 4
            && parsed_parts[1] >= 1
            && parsed_parts[1] <= 12
            && parsed_parts[2] >= 1
            && parsed_parts[2] <= 31
        {
            return Ok(Self {
                scheme: VersionScheme::Calver,
                major_or_year: parsed_parts[0],
                minor_or_month: parsed_parts[1],
                patch_or_day: parsed_parts[2],
                micro: parsed_parts[3],
            });
        }

        Err(crate::Error::manifest(format!("invalid version '{}'", raw)))
    }

    pub fn canonical_string(&self) -> String {
        match self.scheme {
            VersionScheme::Semver => format!(
                "{}.{}.{}",
                self.major_or_year, self.minor_or_month, self.patch_or_day
            ),
            VersionScheme::Calver => {
                if self.micro == 0 {
                    format!(
                        "{:04}.{:02}.{:02}",
                        self.major_or_year, self.minor_or_month, self.patch_or_day
                    )
                } else {
                    format!(
                        "{:04}.{:02}.{:02}.{}",
                        self.major_or_year, self.minor_or_month, self.patch_or_day, self.micro
                    )
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TargetTagValueDefinition {
    pub value: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

impl TargetTagValueDefinition {
    pub fn matches_token(&self, token: &str) -> bool {
        self.value.eq_ignore_ascii_case(token)
            || self
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(token))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TargetTagDefinition {
    pub name: String,
    pub kind: TargetTagKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub inference_prefixes: Vec<String>,
    #[serde(default)]
    pub values: Vec<TargetTagValueDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileManifest {
    pub path: String,
    pub size: u64,
    pub executable: bool,
    pub full_file_hash: String,
    pub chunks: Vec<ChunkRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkRef {
    pub chunk_hash: String,
    pub raw_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleTableEntry {
    pub chunk_hash: String,
    pub raw_size: u64,
    pub compressed_size: u64,
    pub compressed_offset: u64,
    pub compression: String,
    #[serde(default = "default_compression_unit")]
    pub compression_unit: String,
    #[serde(default)]
    pub raw_offset: u64,
}

fn default_compression_unit() -> String {
    COMPRESSION_UNIT_BUNDLE.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Estimate {
    pub missing_logical_bytes: u64,
    pub missing_compressed_bytes: u64,
    pub missing_chunk_count: u64,
    pub required_bundle_count: u64,
    pub range_request_count: u64,
    pub local_cache_hits: u64,
    pub available_disk_space: u64,
    pub fragmentation_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IncompleteInferenceBehavior {
    Warn,
    Error,
}

impl Default for IncompleteInferenceBehavior {
    fn default() -> Self {
        Self::Error
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TargetTagSchema {
    #[serde(default)]
    pub target_tags: Vec<TargetTagDefinition>,
    #[serde(default)]
    pub on_incomplete: IncompleteInferenceBehavior,
}

impl TargetTagSchema {
    pub fn find_definition(&self, name: &str) -> Option<&TargetTagDefinition> {
        self.target_tags
            .iter()
            .find(|definition| definition.name == name)
    }

    pub fn find_value_definition<'a>(
        &'a self,
        definition_name: &str,
        token: &str,
    ) -> Option<&'a TargetTagValueDefinition> {
        self.find_definition(definition_name)?
            .values
            .iter()
            .find(|value_definition| value_definition.matches_token(token))
    }
}

impl Default for TargetTagSchema {
    fn default() -> Self {
        Self {
            target_tags: vec![
                TargetTagDefinition {
                    name: "version".to_string(),
                    kind: TargetTagKind::Exclusive,
                    required: true,
                    inference_prefixes: vec!["-".to_string(), "-v".to_string()],
                    values: Vec::new(),
                    default_value: None,
                },
                TargetTagDefinition {
                    name: "os".to_string(),
                    kind: TargetTagKind::Exclusive,
                    required: false,
                    inference_prefixes: vec!["-".to_string()],
                    values: vec![
                        TargetTagValueDefinition {
                            value: "windows".to_string(),
                            aliases: vec![
                                "win".to_string(),
                                "win32".to_string(),
                                "win64".to_string(),
                            ],
                        },
                        TargetTagValueDefinition {
                            value: "linux".to_string(),
                            aliases: Vec::new(),
                        },
                        TargetTagValueDefinition {
                            value: "macos".to_string(),
                            aliases: vec![
                                "mac".to_string(),
                                "osx".to_string(),
                                "darwin".to_string(),
                            ],
                        },
                    ],
                    default_value: None,
                },
                TargetTagDefinition {
                    name: "arch".to_string(),
                    kind: TargetTagKind::Exclusive,
                    required: false,
                    inference_prefixes: vec!["-".to_string()],
                    values: vec![
                        TargetTagValueDefinition {
                            value: "x86_64".to_string(),
                            aliases: vec![
                                "x64".to_string(),
                                "amd64".to_string(),
                                "win64".to_string(),
                            ],
                        },
                        TargetTagValueDefinition {
                            value: "x86_32".to_string(),
                            aliases: vec!["x86".to_string(), "win32".to_string()],
                        },
                        TargetTagValueDefinition {
                            value: "arm64".to_string(),
                            aliases: vec!["aarch64".to_string()],
                        },
                    ],
                    default_value: None,
                },
                TargetTagDefinition {
                    name: "tier".to_string(),
                    kind: TargetTagKind::Exclusive,
                    required: false,
                    inference_prefixes: vec!["-".to_string()],
                    values: Vec::new(),
                    default_value: None,
                },
            ],
            on_incomplete: IncompleteInferenceBehavior::Error,
        }
    }
}

pub type AutoTaggingRules = TargetTagSchema;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkIndex {
    pub format_version: String,
    pub updated_at: String,
    pub entries: BTreeMap<String, ChunkIndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChunkIndexEntry {
    pub bundle_id: String,
    pub raw_size: u64,
    pub compressed_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginType {
    Mod,
    Dlc,
    LangPack,
    Plugin,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TagConstraint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<String>,
    #[serde(default)]
    pub match_exact: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginDependency {
    pub product_id: String,
    pub gutch_repo_id: String,
    #[serde(default)]
    pub tag_constraints: BTreeMap<String, TagConstraint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginOverride {
    pub product_id: String,
    pub gutch_repo_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginInfo {
    pub plugin_type: PluginType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<PluginDependency>,
    #[serde(default)]
    pub overrides: Vec<PluginOverride>,
    #[serde(default)]
    pub incompatible_with: Vec<PluginOverride>,
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
