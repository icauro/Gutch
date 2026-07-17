use std::collections::HashMap;
use std::io::Read;

use crate::hash::ChunkHash;
use crate::path_guard::resolve_manifest_relative_path;
use crate::progress::GutchOperationControl;
use crate::{Error, Result};

use super::restore::verify_chunk_hash;
use super::types::{ReconstructOptions, VerifiedSourceInstall, typed_chunk_refs};

pub(super) fn read_reusable_source_chunks(
    source: VerifiedSourceInstall<'_>,
    needed_chunks: &std::collections::HashSet<ChunkHash>,
    control: &GutchOperationControl,
) -> Result<HashMap<ChunkHash, Vec<u8>>> {
    let mut chunk_map: HashMap<ChunkHash, Vec<u8>> = HashMap::new();
    let mut skip_buffer = Vec::new();

    for fm in &source.target_manifest.files {
        control.checkpoint()?;
        let typed_chunks = typed_chunk_refs(fm)?;
        if typed_chunks
            .iter()
            .all(|chunk_ref| !needed_chunks.contains(&chunk_ref.hash))
        {
            continue;
        }

        let file_path = resolve_manifest_relative_path(source.install_dir, &fm.path)?;
        let file = std::fs::File::open(&file_path)?;
        let mut reader = std::io::BufReader::new(file);

        let mut current_offset = 0u64;
        for chunk_ref in typed_chunks {
            control.checkpoint()?;
            let chunk_size = chunk_ref.raw_size as usize;
            let chunk_hash = chunk_ref.hash;
            if !needed_chunks.contains(&chunk_hash) || chunk_map.contains_key(&chunk_hash) {
                read_source_chunk_into(
                    &mut reader,
                    &mut skip_buffer,
                    chunk_size,
                    &fm.path,
                    current_offset,
                )?;
                current_offset += chunk_ref.raw_size;
                continue;
            }

            let mut chunk_data = vec![0u8; chunk_size];
            if let Err(error) = reader.read_exact(&mut chunk_data) {
                if error.kind() == std::io::ErrorKind::UnexpectedEof {
                    return Err(Error::Reconstruct(format!(
                        "source chunk out of bounds in {} at offset {}",
                        fm.path, current_offset
                    )));
                }
                return Err(error.into());
            }
            verify_chunk_hash(&chunk_data, chunk_hash, ReconstructOptions::default())?;

            chunk_map.insert(chunk_hash, chunk_data);
            current_offset += chunk_ref.raw_size;
        }
    }

    Ok(chunk_map)
}

fn read_source_chunk_into(
    reader: &mut impl Read,
    chunk_buffer: &mut Vec<u8>,
    chunk_size: usize,
    file_path: &str,
    current_offset: u64,
) -> Result<()> {
    chunk_buffer.resize(chunk_size, 0);
    if let Err(error) = reader.read_exact(chunk_buffer) {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            return Err(Error::Reconstruct(format!(
                "source chunk out of bounds in {} at offset {}",
                file_path, current_offset
            )));
        }
        return Err(error.into());
    }
    Ok(())
}
