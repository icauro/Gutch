use crate::hash::ChunkHash;
use crate::manifest::FileManifest;
use crate::{Error, Result};

#[derive(Clone, Copy)]
pub struct TypedChunkRef<'a> {
    pub hash: ChunkHash,
    pub raw_size: u64,
    pub hash_hex: &'a str,
}

pub struct TypedFileManifest<'a> {
    pub file_manifest: &'a FileManifest,
    pub full_file_hash: ChunkHash,
    pub chunks: Vec<TypedChunkRef<'a>>,
}

pub fn typed_file_manifest(file_manifest: &FileManifest) -> Result<TypedFileManifest<'_>> {
    let full_file_hash = ChunkHash::parse_hex(&file_manifest.full_file_hash).map_err(|err| {
        Error::verify(format!(
            "invalid full file hash '{}' in {}: {}",
            file_manifest.full_file_hash, file_manifest.path, err
        ))
    })?;
    let chunks = file_manifest
        .chunks
        .iter()
        .map(|chunk_ref| {
            Ok(TypedChunkRef {
                hash: ChunkHash::parse_hex(&chunk_ref.chunk_hash).map_err(|err| {
                    Error::verify(format!(
                        "invalid chunk hash '{}' in {}: {}",
                        chunk_ref.chunk_hash, file_manifest.path, err
                    ))
                })?,
                raw_size: chunk_ref.raw_size,
                hash_hex: chunk_ref.chunk_hash.as_str(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(TypedFileManifest {
        file_manifest,
        full_file_hash,
        chunks,
    })
}
