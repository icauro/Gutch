pub(crate) mod binary;
mod content;
pub(crate) mod external_dependency;
pub(crate) mod gutch_repo;
pub(crate) mod launch;
mod profiles;
pub(crate) mod runtime;
pub(crate) mod schema_edit;
pub(crate) mod snapshot;
mod tags;
pub(crate) mod template;

pub const FORMAT_VERSION: &str = "1";
pub const PUBLIC_METADATA_FORMAT_VERSION: &str = "1.0.0";
pub const CHUNKER_FASTCDC: &str = "fastcdc-v2020";
pub const CHUNK_HASH_BLAKE3: &str = "blake3";
pub const CONTENT_REPO_FORMAT: &str = "gutch_repo";
pub const REPO_METADATA_FILE: &str = "repo_metadata.json";

pub(crate) use content::validate_snapshot_entries;
pub use content::{
    BundleTableEntry, ChunkIndex, ChunkIndexEntry, ChunkRef, DirectoryManifest, Estimate,
    FileManifest, SnapshotEntry, SymlinkManifest,
};
pub use external_dependency::ExternalProductDependencyRequirement;
pub(crate) use external_dependency::validate_external_product_dependency_requirements;
pub use profiles::{
    ArtifactSizingPreset, ArtifactStorageMode, BundleSizingPreset, BundleSizingProfile,
    ChunkSizingProfile, CompressionKind, CompressionPreset, CompressionProfile, CompressionUnit,
};
pub use schema_edit::{
    EditableSnapshotTagDefinition, SnapshotTagSchemaValidationError,
    SnapshotTagSchemaValidationResult, sanitize_editable_snapshot_tag_schema,
    validate_editable_snapshot_tag_schema, validate_snapshot_tag_schema,
};
pub(crate) use snapshot::current_host_tags;
pub use snapshot::{
    SnapshotDirectRuntimeMetadata, SnapshotHostVariant, SnapshotHostVariantRuntimeMetadata,
    SnapshotLaunchAction, SnapshotProductMetadata, SnapshotRuntimeMetadata,
};
pub use tags::{
    AutoTaggingRules, InferenceHandler, SnapshotTagDefinition, SnapshotTagKind, SnapshotTagSchema,
    SnapshotTagSet, SnapshotTagValueDefinition, StructuredVersion,
};

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
