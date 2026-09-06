//! Fresh reconstruction estimation for directory or single-file inputs.
use crate as g;
use std::path::{Path, PathBuf};
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
    let result = crate::build_from_directory(&input.dir, "estimate", tags)?;
    let estimate = crate::estimate_build(
        &result.snapshot_manifest,
        &Default::default(),
        1024 * 1024 * 1024,
    )?;
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

fn prepare_input_path(path: &Path) -> g::Result<PreparedInputPath> {
    if path.is_dir() {
        return Ok(PreparedInputPath {
            dir: path.to_path_buf(),
            _tempdir: None,
        });
    }

    if path.is_file() {
        let tempdir = tempfile::Builder::new().prefix("gutch_input_").tempdir()?;
        let filename = path.file_name().ok_or_else(|| {
            g::Error::manifest(format!("input file has no name: {}", path.display()))
        })?;
        let staged_path = tempdir.path().join(filename);
        std::fs::copy(path, &staged_path)?;
        return Ok(PreparedInputPath {
            dir: tempdir.path().to_path_buf(),
            _tempdir: Some(tempdir),
        });
    }

    Err(g::Error::manifest(format!(
        "input path does not exist or is unsupported: {}",
        path.display()
    )))
}
