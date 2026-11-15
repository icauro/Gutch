//! Size limits for repository files downloaded by fetch and pull.

/// Repository metadata, snapshot metadata and product metadata (JSON).
pub(crate) const MAX_METADATA_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_SNAPSHOT_BLUEPRINT_BYTES: u64 = 256 * 1024 * 1024;
pub(crate) const MAX_CHUNK_INDEX_BYTES: u64 = 256 * 1024 * 1024;
pub(crate) const MAX_BUNDLE_BYTES: u64 = 256 * 1024 * 1024;
