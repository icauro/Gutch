//! Cross-process repository protection and recoverable metadata publication.
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::fs::OpenOptions;
use std::path::Path;
use std::path::PathBuf;

use crate::Error;
use crate::Result;

thread_local! {
    static HELD_REPOSITORY_LOCKS: RefCell<HashMap<PathBuf, bool>> = RefCell::new(HashMap::new());
    static DEFERRED_ARTIFACT_DELETIONS: RefCell<HashMap<PathBuf, Vec<PathBuf>>> = RefCell::new(HashMap::new());
}
const REPOSITORY_METADATA_ENTRIES: &[&str] = &[
    "repo_metadata.json",
    "local_repo_config.json",
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
    with_repository_transaction(
        root,
        false,
        &crate::progress::GutchOperationControl::default(),
        work,
    )
}
pub(crate) fn write<T>(root: &Path, work: impl FnOnce() -> Result<T>) -> Result<T> {
    with_repository_transaction(
        root,
        true,
        &crate::progress::GutchOperationControl::default(),
        work,
    )
}

pub(crate) fn read_with_control<T>(
    root: &Path,
    control: &crate::progress::GutchOperationControl,
    work: impl FnOnce() -> Result<T>,
) -> Result<T> {
    with_repository_transaction(root, false, control, work)
}
pub(crate) fn write_with_control<T>(
    root: &Path,
    control: &crate::progress::GutchOperationControl,
    work: impl FnOnce() -> Result<T>,
) -> Result<T> {
    with_repository_transaction(root, true, control, work)
}

fn with_repository_transaction<T>(
    root: &Path,
    write: bool,
    control: &crate::progress::GutchOperationControl,
    work: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let root = std::fs::canonicalize(root)?;
    if let Some(exclusive) = HELD_REPOSITORY_LOCKS.with(|held| held.borrow().get(&root).copied()) {
        if write && !exclusive {
            return Err(Error::manifest("repository lock upgrade is not allowed"));
        }
        return work();
    }
    for owned in [".gutch-lock", ".gutch-transactions"] {
        if std::fs::symlink_metadata(root.join(owned))
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(Error::manifest(
                "repository coordination paths must not be symbolic links",
            ));
        }
    }
    let lock_path = root.join(".gutch-lock");
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
            Err(std::fs::TryLockError::WouldBlock) => {
                std::thread::sleep(std::time::Duration::from_millis(25))
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    HELD_REPOSITORY_LOCKS.with(|held| {
        held.borrow_mut().insert(root.clone(), write);
    });
    let _guard = RepositoryLockGuard {
        root: root.clone(),
        _file: file,
    };
    let journal = root.join(".gutch-transactions");
    let pending = journal.join("pending");
    if pending.exists() {
        if !write {
            return Err(Error::manifest(
                "repository publication interrupted; run repo regenerate-chunk-map or retry a mutation to recover",
            ));
        }
        restore_committed(&root, &journal)?;
        std::fs::remove_file(&pending)?;
    }
    if !write {
        return work();
    }
    std::fs::create_dir_all(&journal)?;
    // Save a complete generation before any mutation, including repositories
    // whose metadata was created by initialization or explicit fixture setup.
    publish_generation(&root, &journal)?;
    crate::filesystem::write(&pending, b"1")?;
    DEFERRED_ARTIFACT_DELETIONS.with(|paths| paths.borrow_mut().remove(&root));
    match work().and_then(|value| {
        control.checkpoint()?;
        Ok(value)
    }) {
        Ok(value) => {
            publish_generation(&root, &journal)?;
            std::fs::remove_file(&pending)?;
            let paths = DEFERRED_ARTIFACT_DELETIONS
                .with(|paths| paths.borrow_mut().remove(&root).unwrap_or_default());
            for path in paths {
                if path.exists() {
                    std::fs::remove_file(path)?;
                }
            }
            Ok(value)
        }
        Err(error) => {
            DEFERRED_ARTIFACT_DELETIONS.with(|paths| paths.borrow_mut().remove(&root));
            restore_committed(&root, &journal)?;
            std::fs::remove_file(&pending)?;
            Err(error)
        }
    }
}

fn copy_entry(source: &Path, target: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(Error::manifest(
            "repository metadata must not contain symbolic links",
        ));
    }
    if metadata.is_dir() {
        std::fs::create_dir_all(target)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_entry(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else if metadata.is_file() {
        crate::filesystem::write_atomic_with(target, |output| {
            std::io::copy(&mut File::open(source)?, output)?;
            Ok(())
        })?;
    } else {
        return Err(Error::manifest("unsupported repository metadata entry"));
    }
    Ok(())
}

fn committed_directory(journal: &Path) -> Result<PathBuf> {
    let name = std::fs::read_to_string(journal.join("committed"))?;
    crate::repo::paths::validate_path_component(&name, "metadata generation")?;
    if !name.starts_with("generation-") {
        return Err(Error::manifest("invalid metadata generation"));
    }
    let directory = journal.join(name);
    if !directory.is_dir() {
        return Err(Error::manifest("committed metadata generation is missing"));
    }
    Ok(directory)
}

fn publish_generation(root: &Path, journal: &Path) -> Result<()> {
    let previous = if journal.join("committed").exists() {
        Some(committed_directory(journal)?)
    } else {
        None
    };
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
        .ok_or_else(|| Error::manifest("invalid generated directory name"))?;
    crate::filesystem::write(&journal.join("committed"), name.as_bytes())?;
    let _committed_generation = generation.keep();
    if let Some(previous) = previous {
        std::fs::remove_dir_all(previous)?;
    }
    Ok(())
}

fn restore_committed(root: &Path, journal: &Path) -> Result<()> {
    let generation = committed_directory(journal)?;
    for name in REPOSITORY_METADATA_ENTRIES {
        let target = root.join(name);
        if target.exists() {
            let metadata = std::fs::symlink_metadata(&target)?;
            if metadata.file_type().is_symlink() {
                return Err(Error::manifest("unsafe metadata recovery target"));
            }
            if metadata.is_dir() {
                std::fs::remove_dir_all(&target)?;
            } else {
                std::fs::remove_file(&target)?;
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
    let path = std::fs::canonicalize(path)?;
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
mod tests {
    use super::*;

    #[test]
    fn failed_mutation_restores_all_metadata_and_retains_artifacts() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("repo_metadata.json"), b"before").unwrap();
        let artifact = root.path().join("bundles/artifact");
        std::fs::create_dir_all(artifact.parent().unwrap()).unwrap();
        std::fs::write(&artifact, b"immutable").unwrap();
        let result: Result<()> = write(root.path(), || {
            std::fs::write(root.path().join("repo_metadata.json"), b"after")?;
            std::fs::write(root.path().join("chunk_map.gtcbin"), b"partial")?;
            assert!(defer_artifact_deletion(&artifact)?);
            Err(Error::manifest("injected publication failure"))
        });
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(root.path().join("repo_metadata.json")).unwrap(),
            b"before"
        );
        assert!(!root.path().join("chunk_map.gtcbin").exists());
        assert!(artifact.exists());
    }

    #[test]
    fn interrupted_publication_is_rejected_by_readers_and_recovered_by_writer() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("repo_metadata.json"), b"committed").unwrap();
        write(root.path(), || Ok(())).unwrap();
        std::fs::write(root.path().join(".gutch-transactions/pending"), b"1").unwrap();
        std::fs::write(root.path().join("repo_metadata.json"), b"partial").unwrap();
        assert!(read(root.path(), || Ok(())).is_err());
        write(root.path(), || {
            assert_eq!(
                std::fs::read(root.path().join("repo_metadata.json"))?,
                b"committed"
            );
            Ok(())
        })
        .unwrap();
        assert!(!root.path().join(".gutch-transactions/pending").exists());
    }

    #[test]
    fn writer_waits_for_reader_to_release_repository() {
        let root = tempfile::tempdir().unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let mut worker = None;
        read(root.path(), || {
            let path = root.path().to_path_buf();
            worker = Some(std::thread::spawn(move || {
                started_tx.send(()).unwrap();
                write(&path, || {
                    finished_tx.send(()).unwrap();
                    Ok(())
                })
                .unwrap();
            }));
            started_rx.recv().unwrap();
            assert!(
                finished_rx
                    .recv_timeout(std::time::Duration::from_millis(50))
                    .is_err()
            );
            Ok(())
        })
        .unwrap();
        finished_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        worker.unwrap().join().unwrap();
    }
    #[test]
    fn cancellation_interrupts_a_writer_waiting_for_a_reader() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        #[derive(Debug)]
        struct Cancel(AtomicBool);
        impl crate::progress::GutchCancellationHook for Cancel {
            fn is_cancelled(&self) -> bool {
                self.0.load(Ordering::Acquire)
            }
        }
        let root = tempfile::tempdir().unwrap();
        let cancelled = Arc::new(Cancel(AtomicBool::new(false)));
        let control =
            crate::progress::GutchOperationControl::default().with_cancellation(cancelled.clone());
        read(root.path(), || {
            let path = root.path().to_path_buf();
            let worker = std::thread::spawn(move || write_with_control(&path, &control, || Ok(())));
            std::thread::sleep(std::time::Duration::from_millis(50));
            cancelled.0.store(true, Ordering::Release);
            assert!(matches!(worker.join().unwrap(), Err(Error::Cancelled)));
            Ok(())
        })
        .unwrap();
    }
    #[test]
    fn shared_reader_does_not_require_write_access_to_existing_lock() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), || Ok(())).unwrap();
        let path = root.path().join(".gutch-lock");
        let original = std::fs::metadata(&path).unwrap().permissions();
        let mut read_only = original.clone();
        read_only.set_readonly(true);
        std::fs::set_permissions(&path, read_only).unwrap();
        let result = read(root.path(), || Ok(()));
        std::fs::set_permissions(path, original).unwrap();
        result.unwrap();
    }
}
