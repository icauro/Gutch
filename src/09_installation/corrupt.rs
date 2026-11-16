//! Deliberate destination corruption for tests and benchmarks.

use std::fs;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::path::Path;

use crate::Result;
use crate::filesystem::paths::resolve_manifest_relative_path;

use crate::installation::contracts::InstallationCorruptionOperation;
use crate::installation::contracts::InstallationCorruptionRequest;
use crate::installation::contracts::InstallationCorruptionResult;

/// Apply an explicit corruption plan to a reconstructed destination.
///
/// This operation intentionally has no production workflow semantics. It exists so tests and
/// benchmark scenarios can exercise verification and repair against identical deterministic
/// damage patterns.
pub fn corrupt_installation(
    request: InstallationCorruptionRequest,
) -> Result<InstallationCorruptionResult> {
    request.control.checkpoint()?;
    let _operation_span = request.control.profile_span("installation.corrupt");
    let root_metadata = fs::symlink_metadata(&request.destination_dir)?;
    if !root_metadata.file_type().is_dir() {
        return Err(crate::Error::invalid_input(format!(
            "reconstructed destination is not a directory: {}",
            request.destination_dir.display()
        )));
    }

    for operation in &request.plan.operations {
        request.control.checkpoint()?;
        apply_operation(&request.destination_dir, operation)?;
    }

    Ok(InstallationCorruptionResult {
        destination_dir: request.destination_dir,
        applied_operation_count: request.plan.operations.len(),
    })
}

fn apply_operation(root: &Path, operation: &InstallationCorruptionOperation) -> Result<()> {
    match operation {
        InstallationCorruptionOperation::FlipByte { path, offset, mask } => {
            flip_byte(root, path, *offset, *mask)
        }
        InstallationCorruptionOperation::Delete { path } => delete_entry(root, path),
        InstallationCorruptionOperation::AddFile { path, content } => add_file(root, path, content),
        InstallationCorruptionOperation::SetReadOnly { path, read_only } => {
            set_read_only(root, path, *read_only)
        }
    }
}

fn flip_byte(root: &Path, relative: &str, offset: u64, mask: u8) -> Result<()> {
    let path = resolve_manifest_relative_path(root, relative)?;
    let mut file = OpenOptions::new().read(true).write(true).open(&path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut byte = [0u8; 1];
    file.read_exact(&mut byte).map_err(|error| {
        crate::Error::invalid_input(format!(
            "cannot flip byte at offset {offset} in '{}': {error}",
            relative
        ))
    })?;
    byte[0] ^= mask;
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(&byte)?;
    Ok(())
}

fn delete_entry(root: &Path, relative: &str) -> Result<()> {
    let path = resolve_manifest_relative_path(root, relative)?;
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        crate::Error::invalid_input(format!(
            "cannot delete corruption target '{}': {error}",
            relative
        ))
    })?;
    if metadata.file_type().is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn add_file(root: &Path, relative: &str, content: &[u8]) -> Result<()> {
    let path = resolve_manifest_relative_path(root, relative)?;
    if path.exists() {
        return Err(crate::Error::invalid_input(format!(
            "cannot add corruption file because it already exists: {}",
            relative
        )));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(content)?;
    Ok(())
}

fn set_read_only(root: &Path, relative: &str, read_only: bool) -> Result<()> {
    let path = resolve_manifest_relative_path(root, relative)?;
    let mut permissions = fs::symlink_metadata(&path)?.permissions();
    crate::filesystem::set_read_only(&mut permissions, read_only);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installation::contracts::InstallationCorruptionOperation;
    use crate::installation::contracts::InstallationCorruptionPlan;

    #[test]
    fn applies_deterministic_file_corruption_operations() {
        let root = tempfile::tempdir().unwrap();
        let file_path = root.path().join("game.bin");
        fs::write(&file_path, [0x10, 0x20, 0x30]).unwrap();

        corrupt_installation(InstallationCorruptionRequest {
            destination_dir: root.path().to_path_buf(),
            plan: InstallationCorruptionPlan {
                operations: vec![
                    InstallationCorruptionOperation::FlipByte {
                        path: "game.bin".into(),
                        offset: 1,
                        mask: 0xff,
                    },
                    InstallationCorruptionOperation::AddFile {
                        path: "extra.txt".into(),
                        content: b"extra".to_vec(),
                    },
                    InstallationCorruptionOperation::SetReadOnly {
                        path: "game.bin".into(),
                        read_only: true,
                    },
                ],
            },
            control: crate::progress::OperationControl::default(),
        })
        .unwrap();

        assert_eq!(fs::read(&file_path).unwrap(), [0x10, 0xdf, 0x30]);
        assert_eq!(fs::read(root.path().join("extra.txt")).unwrap(), b"extra");
        assert!(fs::metadata(file_path).unwrap().permissions().readonly());
    }

    #[test]
    fn rejects_paths_outside_destination_root() {
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().parent().unwrap().join("outside.txt");
        fs::write(&outside, b"safe").unwrap();
        let result = corrupt_installation(InstallationCorruptionRequest {
            destination_dir: root.path().to_path_buf(),
            plan: InstallationCorruptionPlan {
                operations: vec![InstallationCorruptionOperation::Delete {
                    path: "../outside.txt".into(),
                }],
            },
            control: crate::progress::OperationControl::default(),
        });
        assert!(result.is_err());
        assert!(outside.exists());
        let _ = fs::remove_file(outside);
    }
}
