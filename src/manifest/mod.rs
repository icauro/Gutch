//! Repository and snapshot schemas, structural validation, tags, and runtime metadata.

pub(crate) mod blueprint_codec;
pub(crate) mod blueprint_columns;
pub(crate) mod constraints;
mod content;
pub(crate) mod external_dependency;
pub(crate) mod gutch_repo;
pub(crate) mod launch;
pub(crate) mod snapshot;
mod tags;
pub(crate) mod typed;

pub const PUBLIC_METADATA_FORMAT_VERSION: &str = "1.0.0";
pub const CONTENT_REPO_FORMAT: &str = "gutch_repo";
pub const REPO_METADATA_FILE: &str = "repo_metadata.json";

pub use content::ChunkRef;
pub use content::DirectoryManifest;
pub use content::Estimate;
pub use content::FileManifest;
pub use content::SnapshotEntry;
pub use content::SymlinkManifest;
pub(crate) use content::validate_snapshot_entries;
pub use external_dependency::ExternalProductDependencyRequirement;
pub(crate) use external_dependency::validate_external_product_dependency_requirements;
pub use snapshot::SnapshotDirectRuntimeMetadata;
pub use snapshot::SnapshotHostVariant;
pub use snapshot::SnapshotHostVariantRuntimeMetadata;
pub use snapshot::SnapshotLaunchAction;
pub use snapshot::SnapshotManifest;
pub use snapshot::SnapshotProductMetadata;
pub use snapshot::SnapshotRuntimeMetadata;
pub(crate) use snapshot::current_host_tags;
pub use tags::SnapshotTagSet;
pub use tags::StructuredVersion;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/benchmarks/binary_layout_native.rs"]
mod binary_layout_benchmark;
