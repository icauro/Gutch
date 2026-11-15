//! A file manifest with its full-file hash parsed for verification.

use crate::Error;
use crate::Result;
use crate::artifact::hash::ChunkHash;
use crate::manifest::FileManifest;

pub(crate) struct TypedFileManifest<'a> {
    pub(crate) full_file_hash: ChunkHash,
    pub(crate) chunks: &'a [crate::manifest::ChunkRef],
}

pub(crate) fn typed_file_manifest(file_manifest: &FileManifest) -> Result<TypedFileManifest<'_>> {
    let full_file_hash = ChunkHash::parse_hex(&file_manifest.full_file_hash).map_err(|err| {
        Error::verify(format!(
            "invalid full file hash '{}' in {}: {}",
            file_manifest.full_file_hash, file_manifest.path, err
        ))
    })?;
    Ok(TypedFileManifest {
        full_file_hash,
        chunks: &file_manifest.chunks,
    })
}
