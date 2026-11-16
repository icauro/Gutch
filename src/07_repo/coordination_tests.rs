use super::*;

#[test]
#[cfg(windows)]
fn committed_mutation_survives_locked_obsolete_artifact() {
    use std::os::windows::fs::OpenOptionsExt;
    let root = tempfile::tempdir().unwrap();
    crate::repo::init_policy::initialize_repo(root.path(), Default::default()).unwrap();
    let artifact = root.path().join("bundles/obsolete");
    std::fs::create_dir_all(artifact.parent().unwrap()).unwrap();
    std::fs::write(&artifact, b"obsolete").unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(1 | 2)
        .open(&artifact)
        .unwrap();
    let result = write(root.path(), || {
        crate::filesystem::write(
            &root.path().join("local_repo_config.json"),
            br#"{"document_type":"gutch_local_repo_config","document_version":"1.0.0","remote_repo_id":"test"}"#,
        )?;
        assert!(defer_artifact_deletion(&artifact)?);
        Ok(())
    });
    drop(lock);
    result.unwrap();
    read(root.path(), || {
        assert_eq!(
            std::fs::read(root.path().join("local_repo_config.json"))?,
            br#"{"document_type":"gutch_local_repo_config","document_version":"1.0.0","remote_repo_id":"test"}"#
        );
        Ok(())
    })
    .unwrap();
    assert!(artifact.exists());
    write(root.path(), || {
        assert!(defer_artifact_deletion(&artifact)?);
        Ok(())
    })
    .unwrap();
    assert!(!artifact.exists());
}

#[test]
#[cfg(windows)]
fn locked_previous_generation_does_not_fail_metadata_publication() {
    use std::os::windows::fs::OpenOptionsExt;
    let root = tempfile::tempdir().unwrap();
    crate::repo::init_policy::initialize_repo(root.path(), Default::default()).unwrap();
    write(root.path(), || Ok(())).unwrap();
    let journal = root.path().join(".gutch-transactions");
    let previous = committed_directory(&journal).unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(1 | 2)
        .open(previous.join("repo_metadata.json"))
        .unwrap();
    let result = write(root.path(), || {
        crate::filesystem::write(
            &root.path().join("local_repo_config.json"),
            br#"{"document_type":"gutch_local_repo_config","document_version":"1.0.0","remote_repo_id":"test"}"#,
        )
    });
    drop(lock);
    result.unwrap();
    assert_ne!(committed_directory(&journal).unwrap(), previous);
    assert!(previous.exists());
    read(root.path(), || {
        assert_eq!(
            std::fs::read(root.path().join("local_repo_config.json"))?,
            br#"{"document_type":"gutch_local_repo_config","document_version":"1.0.0","remote_repo_id":"test"}"#
        );
        Ok(())
    })
    .unwrap();
    write(root.path(), || Ok(())).unwrap();
    assert!(!previous.exists());
}

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
        Err(Error::repository("injected publication failure"))
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
    crate::repo::init_policy::initialize_repo(root.path(), Default::default()).unwrap();
    let committed = std::fs::read(root.path().join("repo_metadata.json")).unwrap();
    write(root.path(), || Ok(())).unwrap();
    std::fs::write(root.path().join(".gutch-transactions/pending"), b"1").unwrap();
    std::fs::write(root.path().join("repo_metadata.json"), b"partial").unwrap();
    assert!(read(root.path(), || Ok(())).is_err());
    write(root.path(), || {
        assert_eq!(
            std::fs::read(root.path().join("repo_metadata.json"))?,
            committed
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
    use std::sync::Arc;
    let root = tempfile::tempdir().unwrap();
    let cancelled = Arc::new(crate::test_util::TestCancellation::default());
    let control = crate::progress::OperationControl::default().with_cancellation(cancelled.clone());
    read(root.path(), || {
        let path = root.path().to_path_buf();
        let worker = std::thread::spawn(move || write_with_control(&path, &control, || Ok(())));
        std::thread::sleep(std::time::Duration::from_millis(50));
        cancelled.cancel();
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
