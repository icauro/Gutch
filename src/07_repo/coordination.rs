//! Cross-process repository protection and recoverable metadata publication.
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::fs::OpenOptions;
use std::path::Path;
use std::path::PathBuf;

use crate::Error;
use crate::Result;
use crate::filesystem;
use crate::filesystem::write_atomic_with;
use crate::progress::OperationControl;
use crate::repo::paths::LOCK_FILE;
use crate::repo::paths::TRANSACTIONS_DIR;
use crate::repo::paths::validate_path_component;
use std::fs;
use std::fs::TryLockError;
use std::io;
use std::thread;
use std::time::Duration;

thread_local! {
    static HELD_REPOSITORY_LOCKS: RefCell<HashMap<PathBuf, bool>> = RefCell::new(HashMap::new());
    static DEFERRED_ARTIFACT_DELETIONS: RefCell<HashMap<PathBuf, Vec<PathBuf>>> = RefCell::new(HashMap::new());
}
const REPOSITORY_METADATA_ENTRIES: &[&str] = &[
    "repo_metadata.json",
    super::paths::LOCAL_REPO_CONFIG_FILE,
    "chunk_map.gtcbin",
    "snapshots",
];

struct RepositoryLockGuard {
    root: PathBuf,
    _file: File,
}
impl Drop for RepositoryLockGuard {
    fn drop(&mut self) {
        HELD_REPOSITORY_LOCKS.with(|held| {
            held.borrow_mut().remove(&self.root);
        });
    }
}

pub(crate) fn read<T>(root: &Path, work: impl FnOnce() -> Result<T>) -> Result<T> {
    with_repository_transaction(root, false, &OperationControl::default(), work)
}
pub(crate) fn write<T>(root: &Path, work: impl FnOnce() -> Result<T>) -> Result<T> {
    with_repository_transaction(root, true, &OperationControl::default(), work)
}

pub(crate) fn read_with_control<T>(
    root: &Path,
    control: &OperationControl,
    work: impl FnOnce() -> Result<T>,
) -> Result<T> {
    with_repository_transaction(root, false, control, work)
}
pub(crate) fn write_with_control<T>(
    root: &Path,
    control: &OperationControl,
    work: impl FnOnce() -> Result<T>,
) -> Result<T> {
    with_repository_transaction(root, true, control, work)
}

fn with_repository_transaction<T>(
    root: &Path,
    write: bool,
    control: &OperationControl,
    work: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let root = fs::canonicalize(root)?;
    if let Some(exclusive) = HELD_REPOSITORY_LOCKS.with(|held| held.borrow().get(&root).copied()) {
        if write && !exclusive {
            return Err(Error::repo("repository lock upgrade is not allowed"));
        }
        return work();
    }
    for owned in [LOCK_FILE, TRANSACTIONS_DIR] {
        if fs::symlink_metadata(root.join(owned))
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(Error::repo(
                "repository coordination paths must not be symbolic links",
            ));
        }
    }
    let lock_path = root.join(LOCK_FILE);
    let file = if write || !lock_path.exists() {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?
    } else {
        File::open(&lock_path)?
    };
    loop {
        control.checkpoint()?;
        let result = if write {
            file.try_lock()
        } else {
            file.try_lock_shared()
        };
        match result {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(25)),
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    HELD_REPOSITORY_LOCKS.with(|held| {
        held.borrow_mut().insert(root.clone(), write);
    });
    let _guard = RepositoryLockGuard {
        root: root.clone(),
        _file: file,
    };
    let journal = root.join(TRANSACTIONS_DIR);
    let pending = journal.join("pending");
    if pending.exists() {
        if !write {
            return Err(Error::repo(
                "repository publication interrupted; run `gutch repo rebuild-index --apply` or retry any repository change to recover",
            ));
        }
        restore_committed(&root, &journal)?;
        fs::remove_file(&pending)?;
    }
    if !write {
        return work();
    }
    super::paths::ensure_repository_writable(&root)?;
    fs::create_dir_all(&journal)?;
    // Save a complete generation before any mutation, including repositories
    // whose metadata was created by initialization or explicit fixture setup.
    publish_generation(&root, &journal)?;
    filesystem::write(&pending, b"1")?;
    DEFERRED_ARTIFACT_DELETIONS.with(|paths| paths.borrow_mut().remove(&root));
    match work().and_then(|value| {
        control.checkpoint()?;
        validate_committable(&root)?;
        Ok(value)
    }) {
        Ok(value) => {
            publish_generation(&root, &journal)?;
            fs::remove_file(&pending)?;
            delete_deferred_artifacts(&root);
            Ok(value)
        }
        Err(error) => {
            DEFERRED_ARTIFACT_DELETIONS.with(|paths| paths.borrow_mut().remove(&root));
            // Keep the original failure visible. If rollback fails too, the
            // pending marker stays so the next access retries the restore.
            if let Err(restore_error) = restore_committed(&root, &journal) {
                return Err(Error::repo(format!(
                    "{error}; restoring the last committed repository metadata also failed: {restore_error}"
                )));
            }
            fs::remove_file(&pending)?;
            Err(error)
        }
    }
}

/// Commit the metadata changed so far by the write transaction this thread
/// holds on `root`. A later failure in the same transaction restores this
/// point instead of the transaction's start. The commit covers every change
/// made under the held lock, including those of enclosing operations.
pub(crate) fn commit_completed_changes(root: &Path) -> Result<()> {
    let root = fs::canonicalize(root)?;
    if HELD_REPOSITORY_LOCKS.with(|held| held.borrow().get(&root).copied()) != Some(true) {
        return Err(Error::repo(
            "committing completed changes needs the repository write lock",
        ));
    }
    validate_committable(&root)?;
    publish_generation(&root, &root.join(TRANSACTIONS_DIR))?;
    delete_deferred_artifacts(&root);
    Ok(())
}

/// Checks that need the complete metadata of a write before it is committed.
fn validate_committable(root: &Path) -> Result<()> {
    if root.join("repo_metadata.json").exists() {
        let (_, repository) = super::paths::load_repo_manifest(root)?;
        if let Some(constraints) = repository.constraints {
            super::updates::validate_repo_tag_constraints(root, &constraints)?;
        }
    }
    Ok(())
}

fn delete_deferred_artifacts(root: &Path) {
    let paths = DEFERRED_ARTIFACT_DELETIONS
        .with(|paths| paths.borrow_mut().remove(root).unwrap_or_default());
    for path in paths {
        // Metadata is committed. Locked obsolete artifacts remain orphans for
        // a later cleanup; they cannot undo this mutation.
        let _ = fs::remove_file(path);
    }
}

fn copy_entry(source: &Path, target: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(Error::repo(
            "repository metadata must not contain symbolic links",
        ));
    }
    if metadata.is_dir() {
        fs::create_dir_all(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_entry(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else if metadata.is_file() {
        write_atomic_with(target, |output| {
            io::copy(&mut File::open(source)?, output)?;
            Ok(())
        })?;
    } else {
        return Err(Error::repo("unsupported repository metadata entry"));
    }
    Ok(())
}

fn committed_directory(journal: &Path) -> Result<PathBuf> {
    let name = fs::read_to_string(journal.join("committed"))?;
    validate_path_component(&name, "metadata generation")?;
    if !name.starts_with("generation-") {
        return Err(Error::repo("invalid metadata generation"));
    }
    let directory = journal.join(name);
    if !directory.is_dir() {
        return Err(Error::repo("committed metadata generation is missing"));
    }
    Ok(directory)
}

fn publish_generation(root: &Path, journal: &Path) -> Result<()> {
    if journal.join("committed").exists() {
        committed_directory(journal)?;
    }
    let generation = tempfile::Builder::new()
        .prefix("generation-")
        .tempdir_in(journal)?;
    for name in REPOSITORY_METADATA_ENTRIES {
        let source = root.join(name);
        if source.exists() {
            copy_entry(&source, &generation.path().join(name))?;
        }
    }
    let name = generation
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::repo("invalid generated directory name"))?;
    filesystem::write(&journal.join("committed"), name.as_bytes())?;
    let committed_generation = generation.keep();
    // The pointer is committed. Retry obsolete generations left by interrupted
    // or blocked cleanup, without making publication depend on their removal.
    if let Ok(entries) = fs::read_dir(journal) {
        for entry in entries.flatten() {
            if entry.path() != committed_generation
                && entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("generation-")
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
            {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
    Ok(())
}

fn restore_committed(root: &Path, journal: &Path) -> Result<()> {
    let generation = committed_directory(journal)?;
    for name in REPOSITORY_METADATA_ENTRIES {
        let target = root.join(name);
        if target.exists() {
            let metadata = fs::symlink_metadata(&target)?;
            if metadata.file_type().is_symlink() {
                return Err(Error::repo("unsafe metadata recovery target"));
            }
            if metadata.is_dir() {
                fs::remove_dir_all(&target)?;
            } else {
                fs::remove_file(&target)?;
            }
        }
        let source = generation.join(name);
        if source.exists() {
            copy_entry(&source, &target)?;
        }
    }
    Ok(())
}

pub(crate) fn defer_artifact_deletion(path: &Path) -> Result<bool> {
    let path = fs::canonicalize(path)?;
    let owner = HELD_REPOSITORY_LOCKS.with(|held| {
        held.borrow()
            .iter()
            .find(|(root, exclusive)| {
                **exclusive
                    && (path.starts_with(root.join("bundles"))
                        || path.starts_with(root.join("chunks")))
            })
            .map(|(root, _)| root.clone())
    });
    if let Some(root) = owner {
        DEFERRED_ARTIFACT_DELETIONS
            .with(|paths| paths.borrow_mut().entry(root).or_default().push(path));
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
#[path = "coordination_tests.rs"]
mod tests;
