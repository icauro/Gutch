use std::collections::{BTreeMap, BTreeSet};

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 1;
pub const CHUNKER_FASTCDC: &str = "fastcdc-v2020";
pub const CHUNK_HASH_BLAKE3: &str = "blake3";
pub const COMPRESSION_ZSTD: &str = "zstd";
pub const COMPRESSION_UNIT_CHUNK: &str = "chunk";
pub const COMPRESSION_UNIT_BUNDLE: &str = "bundle";
pub const BUNDLE_FORMAT: &str = "gutch-bundle-v1";
pub const CONTENT_REPO_FORMAT: &str = "gutch-content-repo-v1";
pub const CONTENT_REPO_MANIFEST_FILE: &str = "repo_manifest.json";

fn default_next_target_id() -> u64 {
    1
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactSizingPreset {
    Small,
    Balanced,
    Large,
}

impl Default for ArtifactSizingPreset {
    fn default() -> Self {
        Self::Balanced
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BundleSizingPreset {
    Auto,
    Tiny,
    Small,
    Balanced,
    Large,
    Huge,
    Massive,
}

impl Default for BundleSizingPreset {
    fn default() -> Self {
        Self::Balanced
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkSizingProfile {
    pub preset: ArtifactSizingPreset,
    pub chunker: String,
    #[serde(alias = "min_chunk_kibibytes")]
    pub min_chunk_kib: u32,
    #[serde(alias = "avg_chunk_kibibytes")]
    pub avg_chunk_kib: u32,
    #[serde(alias = "max_chunk_kibibytes")]
    pub max_chunk_kib: u32,
    pub gear_table_id: u32,
    pub chunk_hash_algorithm: String,
    pub format_version: u32,
}

impl Default for ChunkSizingProfile {
    fn default() -> Self {
        Self::for_preset(ArtifactSizingPreset::Balanced)
    }
}

impl ChunkSizingProfile {
    pub fn for_preset(preset: ArtifactSizingPreset) -> Self {
        let (min_chunk_kib, avg_chunk_kib, max_chunk_kib) = match preset {
            ArtifactSizingPreset::Small => (1, 4, 16),
            ArtifactSizingPreset::Balanced => (4, 16, 64),
            ArtifactSizingPreset::Large => (16, 64, 256),
        };

        ChunkSizingProfile {
            preset,
            chunker: CHUNKER_FASTCDC.to_string(),
            min_chunk_kib,
            avg_chunk_kib,
            max_chunk_kib,
            gear_table_id: 3,
            chunk_hash_algorithm: CHUNK_HASH_BLAKE3.to_string(),
            format_version: FORMAT_VERSION,
        }
    }

    pub fn min_chunk_size_bytes(&self) -> u32 {
        self.min_chunk_kib * 1024
    }

    pub fn avg_chunk_size_bytes(&self) -> u32 {
        self.avg_chunk_kib * 1024
    }

    pub fn max_chunk_size_bytes(&self) -> u32 {
        self.max_chunk_kib * 1024
    }

    pub fn auto_preset_for_median_file_size_bytes(
        median_file_size_bytes: u64,
    ) -> ArtifactSizingPreset {
        if median_file_size_bytes <= 256 * 1024 {
            ArtifactSizingPreset::Small
        } else if median_file_size_bytes <= 8 * 1024 * 1024 {
            ArtifactSizingPreset::Balanced
        } else {
            ArtifactSizingPreset::Large
        }
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
            BundleSizingPreset::Auto | BundleSizingPreset::Balanced => (32, 16),
            BundleSizingPreset::Tiny => (8, 4),
            BundleSizingPreset::Small => (16, 8),
            BundleSizingPreset::Large => (64, 32),
            BundleSizingPreset::Huge => (128, 64),
            BundleSizingPreset::Massive => (256, 128),
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
        target_logical_size_bytes: u64,
    ) -> BundleSizingPreset {
        if target_logical_size_bytes <= 64 * 1024 * 1024 {
            BundleSizingPreset::Tiny
        } else if target_logical_size_bytes <= 256 * 1024 * 1024 {
            BundleSizingPreset::Small
        } else if target_logical_size_bytes <= 1024 * 1024 * 1024 {
            BundleSizingPreset::Balanced
        } else if target_logical_size_bytes <= 4 * 1024 * 1024 * 1024 {
            BundleSizingPreset::Large
        } else if target_logical_size_bytes <= 16 * 1024 * 1024 * 1024 {
            BundleSizingPreset::Huge
        } else {
            BundleSizingPreset::Massive
        }
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
        Self::for_preset(BundleSizingPreset::Balanced)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionProfile {
    pub compression: String,
    pub compression_unit: String,
    pub zstd_level: i32,
}

impl Default for CompressionProfile {
    fn default() -> Self {
        CompressionProfile {
            compression: COMPRESSION_ZSTD.to_string(),
            compression_unit: COMPRESSION_UNIT_CHUNK.to_string(),
            zstd_level: 3,
        }
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TargetTagKind {
    Plain,
    Scoped,
    Exclusive,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TargetTagDefinition {
    pub name: String,
    pub kind: TargetTagKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetManifest {
    pub manifest_version: u32,
    pub product_id: String,
    pub content_repo_id: String,
    pub target_id: String,
    #[serde(default)]
    pub target_tags: TargetTagSet,
    pub created_at: String,
    #[serde(default, alias = "chunking_profile")]
    pub chunk_sizing_profile: ChunkSizingProfile,
    pub compression_profile: CompressionProfile,
    #[serde(default, alias = "sizing_profile")]
    pub bundle_sizing_profile: BundleSizingProfile,
    pub bundle_format_version: String,
    pub files: Vec<FileManifest>,
    pub total_logical_size: u64,
    pub total_compressed_size_estimate: u64,
    pub signature_metadata: Option<SignatureMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureMetadata {
    pub algorithm: String,
    pub key_id: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileManifest {
    pub path: String,
    pub size: u64,
    pub mode: u32,
    pub executable: bool,
    pub symlink_target: Option<String>,
    pub full_file_hash: String,
    pub chunks: Vec<ChunkRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkRef {
    pub chunk_hash: String,
    pub raw_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierOverlayManifest {
    pub format_version: u32,
    pub product_id: String,
    pub version_id: String,
    pub base_tier_id: String,
    pub target_tier_id: String,
    pub add_files: Vec<FileManifest>,
    pub replace_files: Vec<FileManifest>,
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
    COMPRESSION_UNIT_CHUNK.to_string()
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentRepoManifest {
    pub format: String,
    pub format_version: u32,
    pub product_id: String,
    pub content_repo_id: String,
    pub created_at: String,
    pub targets_dir: String,
    pub bundles_dir: String,
    #[serde(default = "default_next_target_id")]
    pub next_target_id: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "auto_tagging_rules"
    )]
    pub target_schema: Option<TargetTagSchema>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<PluginInfo>,
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
    pub target_id_order: Vec<String>,
    #[serde(default)]
    pub on_incomplete: IncompleteInferenceBehavior,
}

impl TargetTagSchema {
    pub fn find_definition(&self, name: &str) -> Option<&TargetTagDefinition> {
        self.target_tags
            .iter()
            .find(|definition| definition.name == name)
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
                    values: Vec::new(),
                    aliases: BTreeMap::new(),
                    default_value: None,
                },
                TargetTagDefinition {
                    name: "os".to_string(),
                    kind: TargetTagKind::Exclusive,
                    required: false,
                    values: Vec::new(),
                    aliases: BTreeMap::from([
                        ("win".to_string(), "windows".to_string()),
                        ("win32".to_string(), "windows".to_string()),
                        ("win64".to_string(), "windows".to_string()),
                        ("mac".to_string(), "macos".to_string()),
                        ("osx".to_string(), "macos".to_string()),
                        ("darwin".to_string(), "macos".to_string()),
                    ]),
                    default_value: None,
                },
                TargetTagDefinition {
                    name: "arch".to_string(),
                    kind: TargetTagKind::Exclusive,
                    required: false,
                    values: Vec::new(),
                    aliases: BTreeMap::from([
                        ("x64".to_string(), "x86_64".to_string()),
                        ("amd64".to_string(), "x86_64".to_string()),
                        ("win64".to_string(), "x86_64".to_string()),
                        ("x86".to_string(), "x86_32".to_string()),
                        ("win32".to_string(), "x86_32".to_string()),
                    ]),
                    default_value: None,
                },
                TargetTagDefinition {
                    name: "tier".to_string(),
                    kind: TargetTagKind::Exclusive,
                    required: false,
                    values: Vec::new(),
                    aliases: BTreeMap::new(),
                    default_value: None,
                },
            ],
            target_id_order: vec![
                "version".to_string(),
                "os".to_string(),
                "arch".to_string(),
                "tier".to_string(),
            ],
            on_incomplete: IncompleteInferenceBehavior::Error,
        }
    }
}

pub type AutoTaggingRules = TargetTagSchema;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkIndex {
    pub format_version: u32,
    pub updated_at: String,
    pub entries: BTreeMap<String, String>,
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
    pub content_repo_id: String,
    #[serde(default)]
    pub tag_constraints: BTreeMap<String, TagConstraint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginOverride {
    pub product_id: String,
    pub content_repo_id: String,
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

impl ContentRepoManifest {
    pub fn new(product_id: String, content_repo_id: String) -> Self {
        Self {
            format: CONTENT_REPO_FORMAT.to_string(),
            format_version: FORMAT_VERSION,
            product_id,
            content_repo_id,
            created_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            targets_dir: "targets".to_string(),
            bundles_dir: "bundles".to_string(),
            next_target_id: default_next_target_id(),
            target_schema: None,
            plugin: None,
        }
    }
}

impl TargetManifest {
    pub fn new(
        product_id: String,
        content_repo_id: String,
        target_id: String,
        target_tags: TargetTagSet,
    ) -> Self {
        TargetManifest {
            manifest_version: FORMAT_VERSION,
            product_id,
            content_repo_id,
            target_id,
            target_tags,
            created_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            chunk_sizing_profile: ChunkSizingProfile::default(),
            compression_profile: CompressionProfile::default(),
            bundle_sizing_profile: BundleSizingProfile::default(),
            bundle_format_version: BUNDLE_FORMAT.to_string(),
            files: Vec::new(),
            total_logical_size: 0,
            total_compressed_size_estimate: 0,
            signature_metadata: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunking_profile_defaults() {
        let p = ChunkSizingProfile::default();
        assert_eq!(p.preset, ArtifactSizingPreset::Balanced);
        assert_eq!(p.min_chunk_kib, 4);
        assert_eq!(p.avg_chunk_kib, 16);
        assert_eq!(p.max_chunk_kib, 64);
        assert_eq!(p.min_chunk_size_bytes(), 4 * 1024);
        assert_eq!(p.avg_chunk_size_bytes(), 16 * 1024);
        assert_eq!(p.max_chunk_size_bytes(), 64 * 1024);
        assert_eq!(p.chunk_hash_algorithm, "blake3");
    }

    #[test]
    fn test_bundle_sizing_profile_defaults() {
        let p = BundleSizingProfile::default();
        assert_eq!(p.preset, BundleSizingPreset::Balanced);
        assert_eq!(p.bundle_max_mib, 32);
        assert_eq!(p.bundle_preferred_mib, 16);
        assert_eq!(p.bundle_max_size_bytes(), 32 * 1024 * 1024);
        assert_eq!(p.bundle_preferred_size_bytes(), 16 * 1024 * 1024);
    }

    #[test]
    fn test_compression_profile_defaults() {
        let p = CompressionProfile::default();
        assert_eq!(p.zstd_level, 3);
        assert_eq!(p.compression, "zstd");
        assert_eq!(p.compression_unit, "chunk");
    }

    #[test]
    fn test_target_manifest_new() {
        let mut tags = TargetTagSet::default();
        tags.upsert_exclusive("version", "1.0.0");
        tags.upsert_exclusive("os", "windows");
        tags.upsert_exclusive("arch", "x86_64");
        tags.upsert_exclusive("tier", "base");
        let m = TargetManifest::new(
            "prod-1".into(),
            "base".into(),
            "abc123def".into(),
            tags.clone(),
        );
        assert_eq!(m.product_id, "prod-1");
        assert_eq!(m.content_repo_id, "base");
        assert_eq!(m.target_id, "abc123def");
        assert_eq!(m.target_tags.get_exclusive("os"), Some("windows"));
        assert_eq!(m.manifest_version, 1);
        assert!(m.signature_metadata.is_none());
        assert!(m.created_at.len() > 0);
    }

    #[test]
    fn test_manifest_serialization_roundtrip() {
        let mut tags = TargetTagSet::default();
        tags.upsert_exclusive("version", "1.0.0");
        tags.upsert_exclusive("os", "windows");
        tags.upsert_exclusive("arch", "x86_64");
        tags.upsert_exclusive("tier", "base");
        let m = TargetManifest::new(
            "prod-1".into(),
            "base".into(),
            "abc123def".into(),
            tags.clone(),
        );
        let json = serde_json::to_string_pretty(&m).unwrap();
        let m2: TargetManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(m.product_id, m2.product_id);
        assert_eq!(m.content_repo_id, m2.content_repo_id);
        assert_eq!(m.target_id, m2.target_id);
        assert_eq!(m.target_tags, m2.target_tags);
    }

    #[test]
    fn test_file_manifest_serialization() {
        let fm = FileManifest {
            path: "game.exe".to_string(),
            size: 12345,
            mode: 0o644,
            executable: true,
            symlink_target: None,
            full_file_hash: "abc123".to_string(),
            chunks: vec![
                ChunkRef {
                    chunk_hash: "def456".to_string(),
                    raw_size: 8192,
                },
                ChunkRef {
                    chunk_hash: "ghi789".to_string(),
                    raw_size: 4153,
                },
            ],
        };
        let json = serde_json::to_string(&fm).unwrap();
        let fm2: FileManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(fm.path, fm2.path);
        assert_eq!(fm.chunks.len(), 2);
    }

    #[test]
    fn test_content_repo_manifest_new() {
        let repo = ContentRepoManifest::new("prod-1".into(), "base".into());
        assert_eq!(repo.format, CONTENT_REPO_FORMAT);
        assert_eq!(repo.product_id, "prod-1");
        assert_eq!(repo.content_repo_id, "base");
        assert_eq!(repo.targets_dir, "targets");
        assert_eq!(repo.bundles_dir, "bundles");
        assert_eq!(repo.next_target_id, 1);
    }
}
