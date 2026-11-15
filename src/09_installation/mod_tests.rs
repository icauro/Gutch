use crate::artifact::format::FORMAT_VERSION;
use crate::artifact::hash::ChunkHash;
use crate::artifact::hash::hash_hex;
use crate::artifact::profiles::ArtifactStorageMode;
use crate::filesystem;
use crate::filesystem::set_read_only;
use crate::installation::InstallationRepairPlanRequest;
use crate::installation::InstallationRepairRequest;
use crate::installation::execute::entries::prepare_snapshot_entries;
use crate::installation::repair::plan_installation_repair;
use crate::installation::repair_installation;
use crate::installation::resolve_snapshot_state;
use crate::manifest;
use crate::manifest::ChunkRef;
use crate::manifest::FileManifest;
use crate::manifest::SnapshotEntry;
use crate::manifest::SnapshotTagSet;
use crate::manifest::launch::ResolvedLaunchAction;
use crate::manifest::launch::ResolvedLaunchConfiguration;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::OperationControl;
use crate::repo::index::ChunkIndex;
use crate::repo::index::ChunkIndexEntry;
use crate::repo::index::write_chunk_index;
use crate::repo::paths::RepoPaths;
use crate::repo::paths::ensure_gutch_repo_layout;
use crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts;
use crate::runtime::RuntimeResourceHints;
use crate::snapshot::identity::compute_snapshot_id_from_entries;
use crate::sync::fetch;
use crate::sync::fetch_types::SnapshotFetchRequest;
use crate::test_util::chunk_entries;
use crate::test_util::temp_path;
use crate::transfer::types::ArtifactDownload;
use crate::transfer::types::TransferConcurrencyPolicy;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

fn test_snapshot_manifest(files: Vec<(&str, &[u8])>) -> SnapshotManifest {
    let mut manifest =
        SnapshotManifest::new("repair-snapshot".to_string(), SnapshotTagSet::default());
    manifest.set_files(
        files
            .into_iter()
            .map(|(path, bytes)| FileManifest {
                path: path.to_string(),
                size: bytes.len() as u64,
                executable: false,
                read_only: false,
                full_file_hash: hash_hex(&blake3::hash(bytes)),
                chunks: Vec::new(),
            })
            .collect(),
    );
    manifest.snapshot_id = compute_snapshot_id_from_entries(&manifest.entries);
    manifest
}

fn test_hash(label: &str) -> String {
    hash_hex(&blake3::hash(label.as_bytes()))
}

fn write_repair_test_repo(root: &Path, manifest: &SnapshotManifest) {
    let paths = RepoPaths::from_repo_root(root);
    ensure_gutch_repo_layout(&paths, "test").unwrap();
    write_snapshot_manifest_artifacts(
        &paths,
        manifest,
        &crate::progress::OperationControl::default(),
    )
    .unwrap();
}

#[test]
fn artifact_download_deserializes_artifact_response() {
    let bundle: ArtifactDownload = serde_json::from_str(
        r#"{"artifactId":"abc123","url":"https://example.com/bundle","expiresInSeconds":3600}"#,
    )
    .unwrap();
    assert_eq!(bundle.artifact_id, "abc123");
    assert_eq!(bundle.url, "https://example.com/bundle");
}

#[test]
fn artifact_url_provider_rejects_unexpected_artifact_ids() {
    let required_artifact_ids = HashSet::from(["bundle-a".to_string()]);
    let request = SnapshotFetchRequest {
        repo_path: PathBuf::from("C:\\repo"),
        snapshot_id: "snapshot".to_string(),
        repo_metadata_url: None,
        metadata_url: "https://example.com/metadata".to_string(),
        product_metadata_url: None,
        blueprint_url: "https://example.com/blueprint".to_string(),
        chunk_index_url: None,
        artifacts: Vec::new(),
        artifact_url_provider: Some(Arc::new(
            |_artifact_ids: &[String], _control: &OperationControl| {
                Ok(vec![ArtifactDownload {
                    artifact_id: "bundle-b".to_string(),
                    url: "https://example.com/bundle-b".to_string(),
                }])
            },
        )),
        control: OperationControl::default(),
        transfer_policy: TransferConcurrencyPolicy::default(),
    };

    let error = fetch::fetch_artifact_urls(&request, &required_artifact_ids, &HashSet::new())
        .expect_err("unexpected artifact IDs must fail resolution");
    assert!(error.to_string().contains("unexpected artifact 'bundle-b'"));
}

#[test]
fn direct_bundle_urls_reject_duplicate_artifact_ids() {
    let required_artifact_ids = HashSet::from(["bundle-a".to_string()]);
    let request = SnapshotFetchRequest {
        repo_path: PathBuf::from("C:\\repo"),
        snapshot_id: "snapshot".to_string(),
        repo_metadata_url: None,
        metadata_url: "https://example.com/metadata".to_string(),
        product_metadata_url: None,
        blueprint_url: "https://example.com/blueprint".to_string(),
        chunk_index_url: None,
        artifacts: vec![
            ArtifactDownload {
                artifact_id: "bundle-a".to_string(),
                url: "https://example.com/a".to_string(),
            },
            ArtifactDownload {
                artifact_id: " bundle-a ".to_string(),
                url: "https://example.com/a2".to_string(),
            },
        ],
        artifact_url_provider: None,
        control: OperationControl::default(),
        transfer_policy: TransferConcurrencyPolicy::default(),
    };

    let error = fetch::fetch_artifact_urls(&request, &required_artifact_ids, &HashSet::new())
        .expect_err("duplicate bundle IDs must fail normalization");
    assert!(error.to_string().contains("duplicate direct artifact URL"));
}

#[test]
fn resolved_snapshot_state_uses_product_runtime() {
    let root = temp_path("gutch_resolved_snapshot_launch_state");
    let _ = fs::remove_dir_all(&root);
    let paths = RepoPaths::from_repo_root(&root);
    ensure_gutch_repo_layout(&paths, "test").unwrap();
    let mut tags = SnapshotTagSet::default();
    tags.insert("os", "windows").unwrap();
    tags.insert("arch", "x86_64").unwrap();
    let mut snapshot = SnapshotManifest::new("snapshot".into(), tags);
    snapshot.push_file(FileManifest {
        path: "Game.exe".into(),
        size: 1,
        executable: true,
        read_only: false,
        full_file_hash: test_hash("Game.exe"),
        chunks: vec![ChunkRef {
            chunk_hash: ChunkHash::parse_hex(&test_hash("Game.exe")).unwrap(),
            raw_size: 1,
        }],
    });
    snapshot.snapshot_id = compute_snapshot_id_from_entries(&snapshot.entries);
    write_snapshot_manifest_artifacts(
        &paths,
        &snapshot,
        &crate::progress::OperationControl::default(),
    )
    .unwrap();

    snapshot.launch = Some(ResolvedLaunchConfiguration {
        primary: None,
        actions: vec![ResolvedLaunchAction {
            name: "Play".into(),
            executable: "Game.exe".into(),
            arguments: vec!["--snapshot".into()],
            working_directory: None,
        }],
    });
    snapshot.snapshot_id = compute_snapshot_id_from_entries(&snapshot.entries);
    write_snapshot_manifest_artifacts(
        &paths,
        &snapshot,
        &crate::progress::OperationControl::default(),
    )
    .unwrap();
    let resolved = resolve_snapshot_state(&root, &snapshot.snapshot_id).unwrap();
    assert_eq!(
        resolved.launch.unwrap().actions[0].arguments,
        ["--snapshot"]
    );
    assert!(!resolved.snapshot_metadata_hash_b3.is_empty());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn repair_snapshot_ops_keeps_added_files_when_requested() {
    let root = temp_path("gutch_repair_keep_added_files");
    let _ = fs::remove_dir_all(&root);
    let destination_dir = root.join("destination");
    let staging_dir = root.join("staging");
    fs::create_dir_all(&destination_dir).unwrap();
    fs::write(destination_dir.join("game.dat"), b"").unwrap();
    fs::write(destination_dir.join("mod.txt"), b"user data").unwrap();
    let manifest = test_snapshot_manifest(vec![("game.dat", b"")]);
    write_repair_test_repo(&root, &manifest);

    let result = repair_installation(InstallationRepairRequest {
        repo_path: root.clone(),
        snapshot_id: manifest.snapshot_id.clone(),
        destination_dir: destination_dir.clone(),
        staging_dir: Some(staging_dir),
        remove_added_files: false,
        runtime_hints: RuntimeResourceHints::default(),
        control: OperationControl::default(),
    })
    .unwrap()
    .repair_result;

    assert_eq!(result.extra_file_count, 1);
    assert_eq!(result.removed_extra_file_count, 0);
    assert!(destination_dir.join("mod.txt").exists());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn repair_snapshot_ops_full_repair_deletes_added_files() {
    let root = temp_path("gutch_repair_delete_added_files");
    let _ = fs::remove_dir_all(&root);
    let destination_dir = root.join("destination");
    let staging_dir = root.join("staging");
    fs::create_dir_all(destination_dir.join("mods")).unwrap();
    fs::write(destination_dir.join("game.dat"), b"").unwrap();
    fs::write(destination_dir.join("mods").join("mod.txt"), b"user data").unwrap();
    let manifest = test_snapshot_manifest(vec![("game.dat", b"")]);
    write_repair_test_repo(&root, &manifest);

    let result = repair_installation(InstallationRepairRequest {
        repo_path: root.clone(),
        snapshot_id: manifest.snapshot_id.clone(),
        destination_dir: destination_dir.clone(),
        staging_dir: Some(staging_dir),
        remove_added_files: true,
        runtime_hints: RuntimeResourceHints::default(),
        control: OperationControl::default(),
    })
    .unwrap()
    .repair_result;

    assert_eq!(result.extra_file_count, 2);
    assert_eq!(result.removed_extra_file_count, 2);
    assert!(!destination_dir.join("mods").exists());
    assert!(!destination_dir.join("mods").join("mod.txt").exists());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn repair_plan_only_requires_bundles_for_affected_files() {
    let root = temp_path("gutch_repair_plan_bundle_subset");
    let _ = fs::remove_dir_all(&root);
    let paths = RepoPaths::from_repo_root(&root);
    ensure_gutch_repo_layout(&paths, "test").unwrap();

    let chunk_a = test_hash("chunk-a");
    let chunk_b = test_hash("chunk-b");
    let chunk_index = ChunkIndex {
        format_version: FORMAT_VERSION.to_string(),

        entries: chunk_entries(BTreeMap::from([
            (
                chunk_a.clone(),
                ChunkIndexEntry::new(
                    4,
                    0,
                    test_hash("bundle-a").into(),
                    4,
                    ArtifactStorageMode::Bundled,
                ),
            ),
            (
                chunk_b.clone(),
                ChunkIndexEntry::new(
                    4,
                    0,
                    test_hash("bundle-b").into(),
                    4,
                    ArtifactStorageMode::Bundled,
                ),
            ),
        ])),
    };
    write_chunk_index(&paths, &chunk_index).unwrap();

    let mut manifest = SnapshotManifest::new("snapshot".to_string(), SnapshotTagSet::default());
    manifest.set_files(vec![
        FileManifest {
            path: "game.exe".to_string(),
            size: 4,
            executable: true,
            read_only: false,
            full_file_hash: test_hash("game"),
            chunks: vec![manifest::ChunkRef {
                chunk_hash: ChunkHash::parse_hex(&chunk_a).unwrap(),
                raw_size: 4,
            }],
        },
        FileManifest {
            path: "data.bin".to_string(),
            size: 4,
            executable: false,
            read_only: false,
            full_file_hash: test_hash("data"),
            chunks: vec![manifest::ChunkRef {
                chunk_hash: ChunkHash::parse_hex(&chunk_b).unwrap(),
                raw_size: 4,
            }],
        },
    ]);
    manifest.snapshot_id = compute_snapshot_id_from_entries(&manifest.entries);
    write_snapshot_manifest_artifacts(
        &paths,
        &manifest,
        &crate::progress::OperationControl::default(),
    )
    .unwrap();

    let destination_dir = root.join("destination");
    fs::create_dir_all(&destination_dir).unwrap();
    fs::write(destination_dir.join("game.exe"), b"bad!").unwrap();
    fs::write(destination_dir.join("data.bin"), b"data").unwrap();

    let plan = plan_installation_repair(InstallationRepairPlanRequest {
        repo_path: root.clone(),
        snapshot_id: manifest.snapshot_id.clone(),
        destination_dir: destination_dir.clone(),
        remove_added_files: false,
        runtime_hints: RuntimeResourceHints::default(),
        control: OperationControl::default(),
    })
    .unwrap();

    assert_eq!(plan.missing_file_count, 0);
    assert_eq!(plan.corrupt_file_count, 1);
    assert_eq!(plan.required_artifact_ids, vec![test_hash("bundle-a")]);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn repair_restores_manifest_file_attributes() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("destination");
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("game.dat"), b"").unwrap();
    let mut manifest = test_snapshot_manifest(vec![("game.dat", b"")]);
    if let SnapshotEntry::File(file) = &mut manifest.entries[0] {
        file.read_only = true;
        file.executable = true;
    }
    manifest.snapshot_id = compute_snapshot_id_from_entries(&manifest.entries);
    write_repair_test_repo(root.path(), &manifest);
    repair_installation(InstallationRepairRequest {
        repo_path: root.path().to_path_buf(),
        snapshot_id: manifest.snapshot_id,
        destination_dir: destination.clone(),
        staging_dir: None,
        remove_added_files: true,
        runtime_hints: RuntimeResourceHints::default(),
        control: OperationControl::default(),
    })
    .unwrap();
    let permissions = fs::metadata(destination.join("game.dat"))
        .unwrap()
        .permissions();
    assert!(permissions.readonly());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(permissions.mode() & 0o111, 0);
    }
    filesystem::remove_dir_all(&destination).unwrap();
}

#[test]
fn symlink_in_implicit_directory_gets_its_parent_created() {
    let mut manifest = test_snapshot_manifest(vec![("target.txt", b"")]);
    manifest.entries.push(manifest::SnapshotEntry::Symlink(
        manifest::SymlinkManifest {
            path: "nested/link".to_string(),
            target_path: "target.txt".to_string(),
        },
    ));
    let output = tempfile::tempdir().unwrap();
    prepare_snapshot_entries(&manifest, output.path(), &OperationControl::default(), None).unwrap();
    assert!(output.path().join("nested").is_dir());
}

#[cfg(unix)]
#[test]
fn clearing_read_only_restores_only_the_owner_write_bit() {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = Permissions::from_mode(0o444);
    set_read_only(&mut permissions, false);
    assert_eq!(permissions.mode() & 0o777, 0o644);
}

#[test]
fn repair_restores_files_inside_read_only_directories() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("destination");
    let data_directory = destination.join("data");
    fs::create_dir_all(&data_directory).unwrap();
    let mut permissions = fs::metadata(&data_directory).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&data_directory, permissions).unwrap();
    let mut manifest = test_snapshot_manifest(vec![("data/game.dat", b"")]);
    manifest.entries.push(manifest::SnapshotEntry::Directory(
        manifest::DirectoryManifest {
            path: "data".to_string(),
            read_only: true,
        },
    ));
    manifest.snapshot_id = compute_snapshot_id_from_entries(&manifest.entries);
    write_repair_test_repo(root.path(), &manifest);

    repair_installation(InstallationRepairRequest {
        repo_path: root.path().to_path_buf(),
        snapshot_id: manifest.snapshot_id,
        destination_dir: destination.clone(),
        staging_dir: None,
        remove_added_files: true,
        runtime_hints: RuntimeResourceHints::default(),
        control: OperationControl::default(),
    })
    .unwrap();

    assert!(data_directory.join("game.dat").is_file());
    let permissions = fs::metadata(&data_directory).unwrap().permissions();
    assert!(permissions.readonly());
    let mut permissions = permissions;
    set_read_only(&mut permissions, false);
    fs::set_permissions(&data_directory, permissions).unwrap();
}

fn repair(root: &Path, manifest: &SnapshotManifest, remove_added_files: bool) -> crate::Result<()> {
    repair_installation(InstallationRepairRequest {
        repo_path: root.to_path_buf(),
        snapshot_id: manifest.snapshot_id.clone(),
        destination_dir: root.join("destination"),
        staging_dir: None,
        remove_added_files,
        runtime_hints: RuntimeResourceHints::default(),
        control: OperationControl::default(),
    })
    .map(|_| ())
}

/// Creates a file symlink, or returns false where the host lacks the privilege.
fn try_file_symlink(target: &str, link: &Path) -> bool {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link).is_ok();
    #[cfg(windows)]
    return std::os::windows::fs::symlink_file(target, link).is_ok();
}

#[test]
fn repair_replaces_a_dangling_link_and_keeps_extra_dangling_links() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("destination");
    fs::create_dir_all(&destination).unwrap();
    if !try_file_symlink("missing", &destination.join("game.dat"))
        || !try_file_symlink("../../outside", &destination.join("user-link"))
    {
        return;
    }
    let manifest = test_snapshot_manifest(vec![("game.dat", b"")]);
    write_repair_test_repo(root.path(), &manifest);

    repair(root.path(), &manifest, false).unwrap();
    let game = fs::symlink_metadata(destination.join("game.dat")).unwrap();
    assert!(game.file_type().is_file());
    assert!(fs::symlink_metadata(destination.join("user-link")).is_ok());

    repair(root.path(), &manifest, true).unwrap();
    assert!(fs::symlink_metadata(destination.join("user-link")).is_err());
}

#[test]
fn repaired_links_to_intact_directories_still_point_at_directories() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("destination");
    fs::create_dir_all(destination.join("data")).unwrap();
    fs::write(destination.join("data").join("game.dat"), b"").unwrap();
    let mut manifest = test_snapshot_manifest(vec![("data/game.dat", b"")]);
    manifest.entries.push(manifest::SnapshotEntry::Directory(
        manifest::DirectoryManifest {
            path: "data".to_string(),
            read_only: false,
        },
    ));
    manifest.entries.push(manifest::SnapshotEntry::Symlink(
        manifest::SymlinkManifest {
            path: "current".to_string(),
            target_path: "data".to_string(),
        },
    ));
    manifest.snapshot_id = compute_snapshot_id_from_entries(&manifest.entries);
    write_repair_test_repo(root.path(), &manifest);

    // The link is missing; its target directory is intact.
    match repair(root.path(), &manifest, true) {
        Err(error) if error.to_string().contains("symbolic-link privilege") => return,
        result => result.unwrap(),
    }
    assert!(destination.join("current").join("game.dat").is_file());
}

#[test]
fn keeping_extras_refuses_to_replace_a_directory_that_holds_them() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("destination");
    fs::create_dir_all(destination.join("game.dat")).unwrap();
    fs::write(destination.join("game.dat").join("save.txt"), b"user data").unwrap();
    let manifest = test_snapshot_manifest(vec![("game.dat", b"")]);
    write_repair_test_repo(root.path(), &manifest);

    let error = repair(root.path(), &manifest, false).unwrap_err();
    assert!(error.to_string().contains("game.dat"), "{error}");
    assert_eq!(
        fs::read(destination.join("game.dat").join("save.txt")).unwrap(),
        b"user data"
    );
    assert!(!root.path().read_dir().unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("preserve")
    }));

    repair(root.path(), &manifest, true).unwrap();
    assert!(destination.join("game.dat").is_file());
}
