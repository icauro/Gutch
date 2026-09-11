//! Fresh reconstruction estimation for directory or single-file inputs.
use std::path::Path;
use std::path::PathBuf;
use tempfile::TempDir;

#[derive(Debug)]
pub struct SnapshotEstimateResult {
    pub file_count: usize,
    pub total_logical_size: u64,
    pub estimate: crate::manifest::Estimate,
}

pub fn estimate_snapshot(
    source: &Path,
    tags: &crate::manifest::SnapshotTagSet,
) -> crate::Result<SnapshotEstimateResult> {
    let input = prepare_input_path(source)?;
    let result = crate::capture::pipeline::build_from_directory(&input.dir, tags)?;
    let mut chunks = std::collections::HashMap::new();
    for file in result.snapshot_manifest.files() {
        for chunk in &file.chunks {
            chunks.insert(&chunk.chunk_hash, chunk.raw_size);
        }
    }
    let estimate = crate::manifest::Estimate {
        unique_chunk_bytes: chunks.values().map(|size| u64::from(*size)).sum(),
        unique_chunk_count: chunks.len(),
        encoded_artifact_bytes: result.bundles.iter().map(|bundle| bundle.size).sum::<u64>()
            + result
                .loose_chunks
                .iter()
                .map(|chunk| chunk.compressed_size)
                .sum::<u64>(),
        artifact_count: result.bundles.len() + result.loose_chunks.len(),
        bundle_count: result.bundles.len(),
    };
    Ok(SnapshotEstimateResult {
        file_count: result.snapshot_manifest.file_count(),
        total_logical_size: result.snapshot_manifest.total_logical_size,
        estimate,
    })
}

struct PreparedInputPath {
    dir: PathBuf,
    _tempdir: Option<TempDir>,
}

fn prepare_input_path(path: &Path) -> crate::Result<PreparedInputPath> {
    if path.is_dir() {
        return Ok(PreparedInputPath {
            dir: path.to_path_buf(),
            _tempdir: None,
        });
    }

    if path.is_file() {
        let tempdir = tempfile::Builder::new().prefix("gutch_input_").tempdir()?;
        let filename = path.file_name().ok_or_else(|| {
            crate::Error::manifest(format!("input file has no name: {}", path.display()))
        })?;
        let staged_path = tempdir.path().join(filename);
        std::fs::copy(path, &staged_path)?;
        return Ok(PreparedInputPath {
            dir: tempdir.path().to_path_buf(),
            _tempdir: Some(tempdir),
        });
    }

    Err(crate::Error::manifest(format!(
        "input path does not exist or is unsupported: {}",
        path.display()
    )))
}
