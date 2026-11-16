use super::{SnapshotFileReadRequest, read_snapshot_file};
use crate::repo::paths::RepoPaths;
use crate::repo_handle::Repo;

/// Bytes that do not compress or deduplicate, so the file spans several chunks.
fn noise(len: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn bundle_files(repo: &Repo) -> Vec<std::path::PathBuf> {
    let root = RepoPaths::from_repo_root(repo.root()).bundles_root;
    let mut found = Vec::new();
    let mut folders = vec![root];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                folders.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found
}

#[test]
fn reads_one_file_without_installing_the_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    std::fs::create_dir_all(source.join("data")).unwrap();
    let big = noise(3 * 1024 * 1024);
    std::fs::write(source.join("data/big.bin"), &big).unwrap();
    std::fs::write(source.join("small.txt"), "hello\n").unwrap();
    let repo = Repo::init(root.path().join("repo")).unwrap();
    let id = repo.capture(&source, &[]).unwrap().snapshot_id;

    assert_eq!(repo.read_file(&id, "data/big.bin").unwrap(), big);
    assert_eq!(repo.read_file(&id, "small.txt").unwrap(), b"hello\n");
    let error = repo.read_file(&id, "data/missing.bin").unwrap_err();
    assert!(error.to_string().contains("has no file 'data/missing.bin'"));
}

#[test]
fn reports_missing_and_damaged_bundles() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("file.bin"), noise(256 * 1024)).unwrap();
    let repo = Repo::init(root.path().join("repo")).unwrap();
    let id = repo.capture(&source, &[]).unwrap().snapshot_id;
    let bundles = bundle_files(&repo);
    assert_eq!(bundles.len(), 1);

    let saved = std::fs::read(&bundles[0]).unwrap();
    let mut damaged = saved.clone();
    let middle = damaged.len() / 2;
    damaged[middle] ^= 0xFF;
    std::fs::write(&bundles[0], damaged).unwrap();
    let request = SnapshotFileReadRequest {
        repo_root: repo.root().to_path_buf(),
        snapshot_id: id,
        path: "file.bin".into(),
        ..Default::default()
    };
    assert!(read_snapshot_file(&request).is_err());

    std::fs::remove_file(&bundles[0]).unwrap();
    let error = read_snapshot_file(&request).unwrap_err();
    assert!(
        error.to_string().contains("pull the snapshot first"),
        "{error}"
    );
}
