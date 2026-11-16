//! Repository and snapshot schemas, structural validation, tags, and runtime metadata.

pub(crate) mod blueprint_codec;
pub(crate) mod blueprint_columns;
pub(crate) mod constraints;
mod content;
pub(crate) mod external_dependency;
pub(crate) mod format;
pub(crate) mod launch;
pub(crate) mod repo_manifest;
pub(crate) mod snapshot;
mod tags;

pub use content::ChunkRef;
pub use content::DirectoryManifest;
pub use content::Estimate;
pub use content::FileManifest;
pub use content::SnapshotEntry;
pub use content::SymlinkManifest;
pub(crate) use content::validate_snapshot_entries;
pub use external_dependency::ExternalProductDependencyRequirement;
pub(crate) use external_dependency::validate_external_product_dependency_requirements;
pub use format::{
    DocumentType, DocumentVersion, REPO_METADATA_FILE, REPOSITORY_FORMAT_VERSION, RepositoryFormat,
    VersionedDocument,
};
pub(crate) use format::{RepositoryAccess, check_document_markers, parse_document};
pub use snapshot::SnapshotDirectRuntimeMetadata;
pub use snapshot::SnapshotHostVariant;
pub use snapshot::SnapshotHostVariantRuntimeMetadata;
pub use snapshot::SnapshotLaunchAction;
pub use snapshot::SnapshotManifest;
pub use snapshot::SnapshotProductMetadata;
pub use snapshot::SnapshotRuntimeMetadata;
pub(crate) use snapshot::current_host_tags;
pub use tags::SnapshotTagSet;

#[cfg(test)]
#[path = "../../tests/benchmarks/binary_layout_native.rs"]
mod binary_layout_benchmark;
