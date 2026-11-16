use super::write;
use super::write_atomic_with;

#[test]
fn atomic_write_preserves_existing_file_when_writer_fails() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("artifact");
    write(&path, b"original").unwrap();

    let result = write_atomic_with(&path, |_file| {
        Err(crate::Error::repo("simulated write failure"))
    });

    assert!(result.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
#[cfg(windows)]
fn atomic_write_supports_long_windows_artifact_paths() {
    let directory = tempfile::tempdir().unwrap();
    let parent = directory.path().join("snapshot-cache".repeat(14));
    let path = parent.join("snapshot_product_metadata.json");
    assert!(path.as_os_str().len() > 260);
    write(&path, b"original").unwrap();
    write(&path, b"replacement").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
    assert_eq!(std::fs::read_dir(parent).unwrap().count(), 1);
}

#[test]
fn atomic_write_replaces_existing_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("artifact");
    write(&path, b"original").unwrap();

    write(&path, b"replacement").unwrap();

    assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}
