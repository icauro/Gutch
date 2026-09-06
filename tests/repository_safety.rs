use gutch::api::*;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

fn repository() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let source = root.path().join("source");
    initialize_gutch_repo_with_options(&repo, RepoInitOptions::default()).unwrap();
    fs::create_dir(&source).unwrap();
    fs::write(source.join("file.txt"), b"original content").unwrap();
    (root, repo, source)
}

fn capture(repo: &Path, source: &Path) -> String {
    capture_with_storage_mode(repo, source, ArtifactStorageMode::default())
}

fn capture_with_storage_mode(
    repo: &Path,
    source: &Path,
    storage_mode: ArtifactStorageMode,
) -> String {
    capture_repo_snapshot(RepoSnapshotCaptureRequest {
        repo_root: repo.into(),
        source_dir: source.into(),
        explicit_tags: Default::default(),
        chunk_profile: Default::default(),
        bundle_profile: Default::default(),
        compression_algorithm: Default::default(),
        compression_level: None,
        storage_mode,
        runtime_hints: RuntimeResourceHints { jobs: Some(1) },
        control: Default::default(),
    })
    .unwrap()
    .snapshot_id
}

fn reconstruct(
    repo: &Path,
    id: &str,
    destination: &Path,
    staging: Option<PathBuf>,
) -> Result<SnapshotReconstructFromRepoResult> {
    reconstruct_snapshot_from_repo(SnapshotReconstructFromRepoRequest {
        repo_path: repo.into(),
        snapshot_id: id.into(),
        destination_dir: destination.into(),
        staging_dir: staging,
        source_snapshot_id: None,
        source_dir: None,
        runtime_hints: RuntimeResourceHints { jobs: Some(1) },
        control: Default::default(),
    })
}

fn files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files(&path));
        } else {
            found.push(path);
        }
    }
    found.sort();
    found
}

#[test]
fn missing_index_prevents_deletion_of_snapshots_and_shared_artifacts() {
    let (_root, repo, source) = repository();
    let first = capture(&repo, &source);
    fs::write(source.join("second.txt"), b"second file").unwrap();
    let second = capture(&repo, &source);
    let bundles = files(&repo.join("bundles"));
    assert!(!bundles.is_empty());
    fs::remove_file(repo.join("chunk_map.gtcbin")).unwrap();
    assert!(delete_repo_snapshot_and_cleanup(&repo, &first, &Default::default()).is_err());
    let paths = GutchRepoPaths::from_repo_root(&repo);
    assert!(load_snapshot_manifest(&paths, &first).is_ok());
    assert!(load_snapshot_manifest(&paths, &second).is_ok());
    assert_eq!(files(&repo.join("bundles")), bundles);
}

#[test]
fn recapture_rebuilds_artifacts_missing_from_unchanged_files() {
    let (root, repo, source) = repository();
    capture(&repo, &source);
    for path in files(&repo.join("bundles")) {
        fs::remove_file(path).unwrap();
    }
    fs::write(source.join("second.txt"), b"new file").unwrap();
    let id = capture(&repo, &source);
    let destination = root.path().join("installed");
    reconstruct(&repo, &id, &destination, None).unwrap();
    assert_eq!(
        fs::read(destination.join("file.txt")).unwrap(),
        b"original content"
    );
    assert_eq!(
        fs::read(destination.join("second.txt")).unwrap(),
        b"new file"
    );
}

#[test]
fn identical_recapture_rebuilds_missing_artifacts_without_changing_identity() {
    let (root, repo, source) = repository();
    let original = capture(&repo, &source);
    for path in files(&repo.join("bundles")) {
        fs::remove_file(path).unwrap();
    }
    let recaptured = capture(&repo, &source);
    assert_eq!(recaptured, original);
    reconstruct(&repo, &recaptured, &root.path().join("installed"), None).unwrap();
}

#[test]
fn staged_loose_chunks_capture_and_reconstruct() {
    let (root, repo, source) = repository();
    let id = capture_with_storage_mode(&repo, &source, ArtifactStorageMode::LooseChunks);
    let destination = root.path().join("installed");
    reconstruct(&repo, &id, &destination, None).unwrap();
    assert_eq!(
        fs::read(destination.join("file.txt")).unwrap(),
        b"original content"
    );
    assert!(!files(&repo.join("chunks")).is_empty());
}

#[test]
fn overlapping_staging_paths_leave_installed_and_unrelated_files_intact() {
    let (root, repo, source) = repository();
    let id = capture(&repo, &source);
    let parent = root.path().join("installation");
    let destination = parent.join("content");
    fs::create_dir_all(&destination).unwrap();
    fs::write(parent.join("unrelated.txt"), b"unrelated").unwrap();
    fs::write(destination.join("file.txt"), b"corrupt").unwrap();
    for staging in [
        &parent,
        &destination,
        &destination.join("work"),
        &destination.join("..").join("content"),
    ] {
        assert!(reconstruct(&repo, &id, &destination, Some(staging.clone())).is_err());
        assert!(
            repair_snapshot_from_repo(SnapshotRepairFromRepoRequest {
                repo_path: repo.clone(),
                snapshot_id: id.clone(),
                destination_dir: destination.clone(),
                staging_dir: Some(staging.clone()),
                remove_added_files: false,
                runtime_hints: RuntimeResourceHints { jobs: Some(1) },
                control: Default::default(),
            })
            .is_err()
        );
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"corrupt");
        assert_eq!(
            fs::read(parent.join("unrelated.txt")).unwrap(),
            b"unrelated"
        );
    }
}

#[test]
fn staging_parent_preserves_preexisting_files_and_cleans_operation_children() {
    let (root, repo, source) = repository();
    let id = capture(&repo, &source);
    let staging = root.path().join("work");
    fs::create_dir(&staging).unwrap();
    fs::write(staging.join("keep.txt"), b"unrelated").unwrap();
    reconstruct(
        &repo,
        &id,
        &root.path().join("installed"),
        Some(staging.clone()),
    )
    .unwrap();
    assert_eq!(files(&staging), vec![staging.join("keep.txt")]);
    assert_eq!(fs::read(staging.join("keep.txt")).unwrap(), b"unrelated");
}

#[test]
fn invalid_download_leaves_cached_snapshot_bytes_unchanged() {
    let (_root, repo, source) = repository();
    let id = capture(&repo, &source);
    let paths = GutchRepoPaths::from_repo_root(&repo);
    let before: Vec<_> = files(&repo)
        .into_iter()
        .map(|path| {
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    let metadata = fs::read(snapshot_metadata_path(&paths, &id).unwrap()).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for body in [metadata, b"invalid blueprint".to_vec()] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0; 8192];
            stream.read(&mut buffer).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    let result = fetch_snapshot(&SnapshotFetchRequest {
        repo_path: repo.clone(),
        snapshot_id: id.clone(),
        repo_metadata_url: None,
        metadata_url: format!("{base}/metadata"),
        product_metadata_url: None,
        blueprint_url: format!("{base}/blueprint"),
        chunk_index_url: None,
        bundles: vec![],
        bundle_url_provider: None,
        transfer_policy: Default::default(),
        control: Default::default(),
    });
    server.join().unwrap();
    assert!(result.is_err());
    assert!(load_snapshot_manifest(&paths, &id).is_ok());
    for (path, bytes) in before {
        assert_eq!(
            fs::read(&path).unwrap(),
            bytes,
            "{} changed",
            path.display()
        );
    }
}
