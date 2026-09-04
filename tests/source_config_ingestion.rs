//! End-to-end tests for the source-root ingestion config (`.gutch_source_config.json`)
//! flowing through the `snapshot capture` CLI path.

use std::path::Path;

use gutch::GutchRepoPaths;
use gutch::cli::run_from_args;
use gutch::{RepoInitOptions, initialize_gutch_repo_with_options};

fn init_repo(root: &Path) -> GutchRepoPaths {
    let repo_paths = GutchRepoPaths::from_repo_root(&root.join("repo"));
    initialize_gutch_repo_with_options(
        repo_paths.repo_root(),
        RepoInitOptions { force: false },
    )
    .unwrap();
    repo_paths
}

fn write_source_root_files(root: &Path, relative: &str, content: &[u8]) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[test]
fn capture_many_auto_mode_skips_ignored_folders() {
    let temp = tempfile::tempdir().unwrap();
    let repo_paths = init_repo(temp.path());
    let product_dir = temp.path().join("product");
    write_source_root_files(
        &product_dir,
        "v1.0.0-windows-x86_64-2026_07_14/app.exe",
        b"win-app",
    );
    write_source_root_files(
        &product_dir,
        "v1.0.0-linux-x86_64-2026_07_14/app",
        b"linux-app",
    );
    write_source_root_files(&product_dir, ".archives/old.zip", b"ignored");
    std::fs::write(
        product_dir.join(".gutch_source_config.json"),
        r#"{"ingestion_mode":"auto","auto_ingestion_rules":{"ignore_paths":[".archives"]}}"#,
    )
    .unwrap();

    run_from_args([
        "hostapp-cli",
        "snapshot",
        "capture",
        "--source-root",
        product_dir.to_str().unwrap(),
        "--repo",
        repo_paths.repo_root().to_str().unwrap(),
        "--yes",
    ])
    .unwrap();

    let status = gutch::check_gutch_repo(repo_paths.repo_root()).unwrap();
    let snapshot_ids: Vec<&str> = status
        .snapshots
        .iter()
        .map(|snapshot| snapshot.snapshot_id.as_str())
        .collect();
    // The ignored `.archives` folder must not become a snapshot.
    assert_eq!(
        snapshot_ids.len(),
        2,
        "expected exactly two snapshots: {snapshot_ids:?}"
    );
}

#[test]
fn capture_many_explicit_mode_uses_nested_snapshots_and_declared_tags() {
    let temp = tempfile::tempdir().unwrap();
    let repo_paths = init_repo(temp.path());
    let product_dir = temp.path().join("product");
    write_source_root_files(
        &product_dir,
        "versions/4.6.0-stable/windows-x86_64/Godot_v4.6.0-stable_win64.exe",
        b"godot-win",
    );
    write_source_root_files(
        &product_dir,
        "versions/4.6.0-stable/linux-x86_64/Godot_v4.6.0-stable_linux",
        b"godot-linux",
    );
    // A nested folder that is NOT declared must be ignored by discovery.
    write_source_root_files(
        &product_dir,
        "versions/4.7-stable/macos-universal/Godot.app",
        b"x",
    );
    std::fs::write(
        product_dir.join(".gutch_source_config.json"),
        r#"{
            "ingestion_mode": "explicit",
            "explicit_ingestion_rules": {
                "snapshots": [
                    {"path": "versions/4.6.0-stable/windows-x86_64", "tags": ["version::4.6.0", "os::windows", "arch::x86_64", "released_at::2026-07-14"]},
                    {"path": "./versions/4.6.0-stable/linux-x86_64", "tags": ["version::4.6.0", "os::linux", "arch::x86_64", "released_at::2026-07-14"]}
                ]
            }
        }"#,
    )
    .unwrap();

    run_from_args([
        "hostapp-cli",
        "snapshot",
        "capture",
        "--source-root",
        product_dir.to_str().unwrap(),
        "--repo",
        repo_paths.repo_root().to_str().unwrap(),
        "--yes",
    ])
    .unwrap();

    let status = gutch::check_gutch_repo(repo_paths.repo_root()).unwrap();
    assert_eq!(status.snapshots.len(), 2);
    let windows = status
        .snapshots
        .iter()
        .find(|snapshot| snapshot.snapshot_tags.get_exclusive("os") == Some("windows"))
        .expect("windows snapshot");
    assert_eq!(
        windows.snapshot_tags.get_exclusive("version").unwrap(),
        "4.6.0"
    );
    assert_eq!(
        windows.snapshot_tags.get_exclusive("arch").unwrap(),
        "x86_64"
    );
    let linux = status
        .snapshots
        .iter()
        .find(|snapshot| snapshot.snapshot_tags.get_exclusive("os") == Some("linux"))
        .expect("linux snapshot");
    assert_eq!(linux.snapshot_tags.get_exclusive("arch").unwrap(), "x86_64");
}

#[test]
fn capture_many_explicit_mode_accepts_missing_version_tags() {
    let temp = tempfile::tempdir().unwrap();
    let repo_paths = init_repo(temp.path());
    let product_dir = temp.path().join("product");
    write_source_root_files(&product_dir, "some-folder/app.exe", b"app");
    std::fs::write(
        product_dir.join(".gutch_source_config.json"),
        r#"{
            "ingestion_mode": "explicit",
            "explicit_ingestion_rules": {
                "snapshots": [
                    {"path": "some-folder", "tags": ["os::windows", "arch::x86_64"]}
                ]
            }
        }"#,
    )
    .unwrap();

    run_from_args([
        "hostapp-cli",
        "snapshot",
        "capture",
        "--source-root",
        product_dir.to_str().unwrap(),
        "--repo",
        repo_paths.repo_root().to_str().unwrap(),
        "--yes",
    ])
    .unwrap();

    let status = gutch::check_gutch_repo(repo_paths.repo_root()).unwrap();
    assert_eq!(status.snapshots.len(), 1);
    assert_eq!(
        status.snapshots[0].snapshot_tags.get_exclusive("os"),
        Some("windows")
    );
    assert_eq!(status.snapshots[0].snapshot_tags.get_exclusive("version"), None);
}

#[test]
fn capture_many_auto_captures_skeleton_config_on_first_use() {
    let temp = tempfile::tempdir().unwrap();
    let repo_paths = init_repo(temp.path());
    let product_dir = temp.path().join("product");
    write_source_root_files(
        &product_dir,
        "v1.0.0-windows-x86_64-2026_07_14/app.exe",
        b"app",
    );
    assert!(!product_dir.join(".gutch_source_config.json").exists());

    run_from_args([
        "hostapp-cli",
        "snapshot",
        "capture",
        "--source-root",
        product_dir.to_str().unwrap(),
        "--repo",
        repo_paths.repo_root().to_str().unwrap(),
        "--yes",
    ])
    .unwrap();

    assert!(product_dir.join(".gutch_source_config.json").is_file());
    let status = gutch::check_gutch_repo(repo_paths.repo_root()).unwrap();
    assert_eq!(status.snapshots.len(), 1);
}

#[test]
fn capture_single_dir_uses_manual_tags() {
    let temp = tempfile::tempdir().unwrap();
    let repo_paths = init_repo(temp.path());
    let source_dir = temp.path().join("v2.0.0-windows-x86_64-2026_08_01");
    write_source_root_files(&source_dir, "app.exe", b"app-v2");

    run_from_args([
        "hostapp-cli",
        "snapshot",
        "capture",
        "--source-dir",
        source_dir.to_str().unwrap(),
        "--repo",
        repo_paths.repo_root().to_str().unwrap(),
        "--tag",
        "version::2.0.0",
        "--tag",
        "os::windows",
        "--tag",
        "arch::x86_64",
        "--yes",
    ])
    .unwrap();

    let status = gutch::check_gutch_repo(repo_paths.repo_root()).unwrap();
    assert_eq!(status.snapshots.len(), 1);
    assert_eq!(
        status.snapshots[0]
            .snapshot_tags
            .get_exclusive("version")
            .unwrap(),
        "2.0.0"
    );
}
