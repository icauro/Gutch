use gutch_s3_store::MemoryStore;

use super::*;

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text.repeat(500)).unwrap();
}

#[test]
fn capture_on_one_machine_install_and_repair_on_another() {
    let temp = tempfile::tempdir().unwrap();
    let bucket = MemoryStore::default();
    let first = Remote::new(bucket.clone(), temp.path().join("cache1"));
    let second = Remote::new(bucket, temp.path().join("cache2"));

    let repo_id = first
        .create_repo("team", "game", None, OperationControl::default())
        .unwrap();
    assert_eq!(repo_id, "team--game");
    assert_eq!(second.repos().unwrap()[0].repository_id, repo_id);
    assert!(second.snapshots(&repo_id).unwrap().is_empty());

    let source = temp.path().join("source");
    write(&source.join("data/a.txt"), "first\n");
    write(&source.join("b.txt"), "second\n");
    let message = first
        .capture_and_push(
            &repo_id,
            &source,
            "version=1.0",
            OperationControl::default(),
        )
        .unwrap();
    assert!(message.starts_with("captured "), "{message}");

    let snapshots = second.snapshots(&repo_id).unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].tags, ["version=1.0"]);
    let old_id = snapshots[0].snapshot_id.clone();

    let app = temp.path().join("app");
    second
        .install(&repo_id, &old_id, &app, None, OperationControl::default())
        .unwrap();
    fs::write(app.join("b.txt"), "damaged").unwrap();
    let verified = second
        .verify(&repo_id, &old_id, &app, OperationControl::default())
        .unwrap();
    assert!(
        verified.ends_with("0 missing, 1 corrupt, 0 extra"),
        "{verified}"
    );
    second
        .repair(&repo_id, &old_id, &app, OperationControl::default())
        .unwrap();
    let verified = second
        .verify(&repo_id, &old_id, &app, OperationControl::default())
        .unwrap();
    assert!(
        verified.ends_with("0 missing, 0 corrupt, 0 extra"),
        "{verified}"
    );

    // The second machine pushes 1.1; the first updates its install in place.
    write(&source.join("b.txt"), "changed\n");
    second
        .capture_and_push(
            &repo_id,
            &source,
            "version=1.1",
            OperationControl::default(),
        )
        .unwrap();
    let snapshots = first.snapshots(&repo_id).unwrap();
    assert_eq!(snapshots.len(), 2);
    let new_id = snapshots
        .iter()
        .find(|snapshot| snapshot.snapshot_id != old_id)
        .unwrap()
        .snapshot_id
        .clone();
    first
        .install(&repo_id, &old_id, &app, None, OperationControl::default())
        .unwrap();
    let message = first
        .install(
            &repo_id,
            &new_id,
            &app,
            Some(&old_id),
            OperationControl::default(),
        )
        .unwrap();
    assert!(!message.contains(" 0 of "), "{message}");
    assert_eq!(
        fs::read_to_string(app.join("b.txt")).unwrap(),
        "changed\n".repeat(500)
    );
}

#[test]
fn create_repo_refuses_a_taken_name_and_leaves_no_copy() {
    let temp = tempfile::tempdir().unwrap();
    let bucket = MemoryStore::default();
    let first = Remote::new(bucket.clone(), temp.path().join("cache1"));
    let second = Remote::new(bucket, temp.path().join("cache2"));
    first
        .create_repo("team", "game", None, OperationControl::default())
        .unwrap();

    let error = second
        .create_repo("team", "game", None, OperationControl::default())
        .unwrap_err();

    assert!(error.to_string().contains("already exists"), "{error}");
    assert!(!temp.path().join("cache2/team/game").exists());
}

#[test]
fn capture_rejects_malformed_tags_before_touching_the_remote() {
    let temp = tempfile::tempdir().unwrap();
    let remote = Remote::new(MemoryStore::default(), temp.path().join("cache"));
    let error = remote
        .capture_and_push(
            "team--missing",
            temp.path(),
            "version",
            OperationControl::default(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("use key=value"), "{error}");
}

#[test]
fn browsing_a_snapshot_downloads_only_that_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let bucket = MemoryStore::default();
    let first = Remote::new(bucket.clone(), temp.path().join("cache1"));
    let second = Remote::new(bucket, temp.path().join("cache2"));
    let repo_id = first
        .create_repo("team", "game", None, OperationControl::default())
        .unwrap();
    let source = temp.path().join("source");
    let control = OperationControl::default;
    write(&source.join("data/a.txt"), "first\n");
    first
        .capture_and_push(&repo_id, &source, "version=1.0", control())
        .unwrap();
    write(&source.join("data/a.txt"), "second\n");
    first
        .capture_and_push(&repo_id, &source, "version=1.1", control())
        .unwrap();
    let snapshots = second.snapshots(&repo_id).unwrap();
    let id_tagged = |tag: &str| {
        let snapshot = snapshots.iter().find(|snapshot| snapshot.tags == [tag]);
        snapshot.unwrap().snapshot_id.clone()
    };
    let (wanted, other) = (id_tagged("version=1.0"), id_tagged("version=1.1"));

    let bundle_count = || count_files(&temp.path().join("cache2/team/game/bundles"));
    let files = second.files(&repo_id, &wanted, control()).unwrap();
    assert_eq!(files, [("data/a.txt".to_owned(), 3000)]);
    assert_eq!(bundle_count(), 0, "listing files downloads no bundles");
    let target = temp.path().join("a.txt");
    second
        .save_file(&repo_id, &wanted, "data/a.txt", &target, control())
        .unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "first\n".repeat(500));
    assert_eq!(bundle_count(), 1);

    // The other snapshot was never downloaded into this machine's copy.
    let local = Repo::open(temp.path().join("cache2/team/game")).unwrap();
    assert!(local.read_file(&other, "data/a.txt").is_err());
}

fn count_files(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() { count_files(&path) } else { 1 }
        })
        .sum()
}

#[test]
fn an_encrypted_repository_opens_only_with_its_password() {
    let temp = tempfile::tempdir().unwrap();
    let bucket = MemoryStore::default();
    let first = Remote::new(bucket.clone(), temp.path().join("cache1"));
    let second = Remote::new(bucket.clone(), temp.path().join("cache2"));
    let password = RepoCredential::Password("correct horse".into());

    let repo_id = first
        .create_repo(
            "team",
            "vault",
            Some(password.clone()),
            OperationControl::default(),
        )
        .unwrap();
    let source = temp.path().join("source");
    write(&source.join("secret.txt"), "only for the team\n");
    first
        .capture_and_push(
            &repo_id,
            &source,
            "version=1.0",
            OperationControl::default(),
        )
        .unwrap();
    let stored = bucket.objects();
    assert!(
        stored.values().all(|bytes| !bytes
            .windows(17)
            .any(|window| window == b"only for the team")),
        "the bucket holds plaintext"
    );
    assert!(stored.keys().all(|key| !key.contains("version")));

    // A second machine has no unlocked keys and no password yet.
    gutch::api::lock_repos();
    let snapshot_id = second.snapshots(&repo_id).unwrap()[0].snapshot_id.clone();
    let app = temp.path().join("app");
    let install = |control| second.install(&repo_id, &snapshot_id, &app, None, control);
    assert!(install(OperationControl::default()).is_err());
    gutch::api::set_default_repo_credential(Some(RepoCredential::Password("wrong".into())));
    assert!(install(OperationControl::default()).is_err());
    gutch::api::set_default_repo_credential(Some(password));
    install(OperationControl::default()).unwrap();
    assert_eq!(
        fs::read_to_string(app.join("secret.txt")).unwrap(),
        "only for the team\n".repeat(500)
    );
    gutch::api::lock_repos();
}
