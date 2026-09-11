use crate::Error;
use crate::Result;
use crate::artifact::hash::ChunkHash;
use crate::manifest::FileManifest;

#[derive(Clone, Copy)]
pub(crate) struct TypedChunkRef<'a> {
    pub(crate) hash: ChunkHash,
    pub(crate) raw_size: u64,
    pub(crate) hash_hex: &'a str,
}

pub(crate) struct TypedFileManifest<'a> {
    pub(crate) full_file_hash: ChunkHash,
    pub(crate) chunks: Vec<TypedChunkRef<'a>>,
}

pub(crate) fn typed_file_manifest(file_manifest: &FileManifest) -> Result<TypedFileManifest<'_>> {
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
                raw_size: u64::from(chunk_ref.raw_size),
                hash_hex: chunk_ref.chunk_hash.as_str(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(TypedFileManifest {
        full_file_hash,
        chunks,
    })
}
