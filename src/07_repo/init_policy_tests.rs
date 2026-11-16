use super::*;
use crate::manifest;
use crate::manifest::snapshot::SnapshotManifest;
use crate::repo::paths::ensure_repo_layout;
use crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts;
use crate::test_util::{test_chunk_hash, test_hash};

fn exclusive_tags(version: &str, os: &str, arch: &str, tier: &str) -> manifest::SnapshotTagSet {
    let mut tags = manifest::SnapshotTagSet::default();
    tags.insert("version", version).unwrap();
    tags.insert("os", os).unwrap();
    tags.insert("arch", arch).unwrap();
    tags.insert("tier", tier).unwrap();
    tags
}

#[test]
fn assess_repo_init_path_reports_existing_repo() {
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().join("repo");
    let repo_paths = RepoPaths::from_repo_root(&repo_root);
    ensure_repo_layout(&repo_paths, "base").unwrap();

    let assessment = assess_repo_init_path(&repo_root).unwrap();
    assert_eq!(assessment.path_kind, RepoInitPathKind::ExistingRepo);
    assert!(assessment.requires_confirmation);
}

#[test]
fn initialize_repo_init_without_force_rejects_existing_repo() {
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().join("repo");
    let repo_paths = RepoPaths::from_repo_root(&repo_root);
    ensure_repo_layout(&repo_paths, "base").unwrap();

    let err = initialize_repo(
        &repo_root,
        RepoInitOptions {
            force: false,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains(REPO_INIT_NEEDS_CONFIRMATION));
}

#[test]
fn initialize_repo_init_with_force_replaces_existing_repo_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().join("repo");
    let repo_paths = RepoPaths::from_repo_root(&repo_root);
    ensure_repo_layout(&repo_paths, "base").unwrap();
    let snapshot_id = test_hash("snapshot-one");
    let mut snapshot_manifest = SnapshotManifest::new(
        snapshot_id.clone(),
        exclusive_tags("1.0.0", "windows", "x86_64", "base"),
    );
    snapshot_manifest.push_file(manifest::FileManifest {
        path: "game.exe".to_string(),
        size: 4,
        executable: true,
        read_only: false,
        full_file_hash: test_chunk_hash("game.exe"),
        chunks: vec![crate::manifest::ChunkRef {
            chunk_hash: test_chunk_hash("game.exe"),
            raw_size: 4,
        }],
    });
    snapshot_manifest.snapshot_id =
        crate::snapshot::identity::snapshot_id_from_entries(&snapshot_manifest.entries);
    write_snapshot_manifest_artifacts(
        &repo_paths,
        &snapshot_manifest,
        &crate::progress::OperationControl::default(),
    )
    .unwrap();
    crate::test_util::index_manifest(&repo_paths, &snapshot_manifest);
    let bundle_path = crate::repo::paths::bundle_path(&repo_paths, &"a".repeat(64)).unwrap();
    std::fs::create_dir_all(bundle_path.parent().unwrap()).unwrap();
    std::fs::write(bundle_path, b"bundle").unwrap();

    initialize_repo(
        &repo_root,
        RepoInitOptions {
            force: true,
            ..Default::default()
        },
    )
    .unwrap();

    let status = read_repo_status(&repo_root).unwrap();
    assert!(status.is_repo);
    assert_eq!(status.snapshots.len(), 0);
    assert_eq!(status.bundle_count, 0);
}

#[test]
fn initialize_repo_init_with_force_keeps_unrelated_files_in_non_empty_path() {
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().join("repo");
    std::fs::create_dir_all(&repo_root).unwrap();
    std::fs::write(repo_root.join("notes.txt"), b"keep me").unwrap();

    initialize_repo(
        &repo_root,
        RepoInitOptions {
            force: true,
            ..Default::default()
        },
    )
    .unwrap();

    assert!(repo_root.join("notes.txt").exists());
    assert!(repo_root.join("snapshots").is_dir());
}
