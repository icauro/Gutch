//! Chunk-to-artifact locations, index persistence, and required-artifact queries.

pub(crate) mod binary_codec;
mod entries;
mod records;
mod required;
mod store;

pub(crate) use entries::ChunkIndexEntries;
pub(crate) use records::{ChunkIndex, ChunkIndexEntry};
pub use required::{required_bundle_ids, required_chunk_hashes};
pub(crate) use store::{
    empty_chunk_index, load_chunk_index, preserve_local_chunk_locations, write_chunk_index,
};
