pub mod binary;
mod content;
pub mod external_dependency;
pub mod gutch_repo;
pub mod launch;
mod profiles;
pub mod runtime;
pub mod schema_edit;
pub mod snapshot;
mod tags;
pub mod template;

pub const FORMAT_VERSION: &str = "1";
pub const PUBLIC_METADATA_FORMAT_VERSION: &str = "1.0.0";
pub const CHUNKER_FASTCDC: &str = "fastcdc-v2020";
pub const CHUNK_HASH_BLAKE3: &str = "blake3";
pub const COMPRESSION_ZSTD: &str = "zstd";
pub const COMPRESSION_UNIT_BUNDLE: &str = "bundle";
pub const CONTENT_REPO_FORMAT: &str = "gutch_repo";
pub const REPO_METADATA_FILE: &str = "repo_metadata.json";

pub use content::{
    BundleTableEntry, ChunkIndex, ChunkIndexEntry, ChunkRef, DirectoryManifest, Estimate,
    FileManifest, SnapshotEntry, SymlinkManifest, validate_snapshot_entries,
};
pub use external_dependency::{
    ExternalProductDependencyRequirement, validate_external_product_dependency_requirements,
};
pub use profiles::{
    ArtifactSizingPreset, ArtifactStorageMode, BundleSizingPreset, BundleSizingProfile,
    ChunkSizingProfile, CompressionKind, CompressionPreset, CompressionProfile, CompressionUnit,
};
pub use schema_edit::{
    EditableSnapshotTagDefinition, SnapshotTagSchemaValidationError,
    SnapshotTagSchemaValidationResult, sanitize_editable_snapshot_tag_schema,
    validate_editable_snapshot_tag_schema, validate_snapshot_tag_schema,
};
pub use snapshot::{
    SnapshotBlueprint, SnapshotDirectRuntimeMetadata, SnapshotHostVariant,
    SnapshotHostVariantRuntimeMetadata, SnapshotLaunchAction, SnapshotMetadata,
    SnapshotProductMetadata, SnapshotRuntimeMetadata, current_host_tags,
};
pub use tags::{
    AutoTaggingRules, InferenceHandler, SnapshotTagDefinition, SnapshotTagKind, SnapshotTagSchema,
    SnapshotTagSet, SnapshotTagValueDefinition, StructuredVersion,
};

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
