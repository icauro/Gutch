//! Encrypted repositories end to end. One test, because unlocked keys are
//! held per process and the test locks them.
use gutch::api::*;
use std::fs;
use std::path::Path;

const SECRET_TEXT: &[u8] = b"launch codes for the spring release";
const SECRET_NAME: &str = "plans_for_the_merger.txt";
const SECRET_TAG: &str = "codename=bluebird";

fn capture(
    repo: &Path,
    source: &Path,
    storage_mode: ArtifactStorageMode,
    chunker: Chunker,
) -> Result<String> {
    capture_snapshot(&SnapshotCaptureRequest {
        repo_root: repo.to_path_buf(),
        source: source.to_path_buf(),
        tags: SnapshotTagSet::from_compact_strings(&[SECRET_TAG.to_owned()])?,
        chunk_profile: Default::default(),
        chunker,
        bundle_profile: Default::default(),
        // Uncompressed, so unencrypted content would show in the files.
        compression_algorithm: CompressionKind::None,
        compression_level: None,
        storage_mode,
        runtime_hints: RuntimeResourceHints {
            jobs: Some(2),
            ..Default::default()
        },
        reuse_remote_bundles: false,
        control: Default::default(),
    })
    .map(|capture| capture.snapshot_id)
}

fn reconstruct(repo: &Path, snapshot_id: &str, destination: &Path) -> Result<()> {
    reconstruct_installation(&InstallationReconstructRequest {
        repo_root: repo.to_path_buf(),
        snapshot_id: snapshot_id.to_owned(),
        destination_dir: destination.to_path_buf(),
        ..Default::default()
    })
    .map(|_| ())
}

/// Every file below `root`, by path and content.
fn files(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push((path.display().to_string(), fs::read(&path).unwrap()));
            }
        }
    }
    found
}

fn reveals_secrets(repo: &Path) -> bool {
    let contains = |bytes: &[u8], needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    files(repo).iter().any(|(path, bytes)| {
        path.contains(SECRET_NAME)
            || contains(bytes, SECRET_TEXT)
            || contains(bytes, SECRET_NAME.as_bytes())
            || contains(bytes, b"bluebird")
    })
}

#[test]
fn encrypted_repositories_hide_content_and_open_only_with_a_credential() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    fs::create_dir_all(source.join("docs")).unwrap();
    fs::write(
        source.join("docs").join(SECRET_NAME),
        SECRET_TEXT.repeat(4000),
    )
    .unwrap();
    fs::write(source.join("small.txt"), SECRET_TEXT).unwrap();
    let password = RepoCredential::Password("correct horse".into());

    for (name, storage_mode) in [
        ("bundled", ArtifactStorageMode::Bundled),
        ("loose", ArtifactStorageMode::LooseChunks),
    ] {
        // The same capture into a plain repository shows what is hidden.
        let plain = root.path().join(format!("{name}_plain"));
        Repo::init(&plain).unwrap();
        capture(&plain, &source, storage_mode, Chunker::FastCdc).unwrap();
        assert!(reveals_secrets(&plain), "{name}: the check finds nothing");

        let repo = root.path().join(name);
        Repo::init_encrypted(&repo, password.clone()).unwrap();
        let snapshot_id = capture(&repo, &source, storage_mode, Chunker::FastCdc).unwrap();
        assert!(
            !reveals_secrets(&repo),
            "{name}: the repository holds plaintext"
        );

        let installed = root.path().join(format!("{name}_installed"));
        reconstruct(&repo, &snapshot_id, &installed).unwrap();
        assert_eq!(
            fs::read(installed.join("docs").join(SECRET_NAME)).unwrap(),
            SECRET_TEXT.repeat(4000)
        );
        let verified = verify_repo(&RepoVerifyRequest {
            repo_root: repo.clone(),
            snapshot_id: None,
            mode: RepoVerifyMode::Strict,
            control: Default::default(),
        })
        .unwrap();
        assert!(verified.ok, "{name}: verification failed");

        // Chunkers without a key would reveal content through chunk sizes.
        assert!(capture(&repo, &source, storage_mode, Chunker::Jc).is_err());

        let key_file = root.path().join(format!("{name}.key"));
        add_repo_credential(&repo, &RepoCredential::create_key_file(&key_file).unwrap()).unwrap();

        lock_repos();
        let again = root.path().join(format!("{name}_again"));
        assert!(reconstruct(&repo, &snapshot_id, &again).is_err());
        assert!(unlock_repo(&repo, RepoCredential::Password("wrong".into())).is_err());
        unlock_repo(&repo, RepoCredential::read_key_file(&key_file).unwrap()).unwrap();
        reconstruct(&repo, &snapshot_id, &again).unwrap();
        assert_eq!(fs::read(again.join("small.txt")).unwrap(), SECRET_TEXT);
        lock_repos();
    }
}
