//! End-to-end tests for the source-root ingestion config (`.gutch_source_config.json`)
//! flowing through the `target add-many` CLI path.

use std::path::Path;

use gutch::cli::run_from_args;
use gutch::repo::ensure_gutch_repo_layout;
use gutch::GutchRepoPaths;

fn init_repo(root: &Path) -> GutchRepoPaths {
    let repo_paths = GutchRepoPaths::from_repo_root(&root.join("repo"));
    ensure_gutch_repo_layout(&repo_paths, "base").unwrap();
    repo_paths
}

fn write_source_root_files(root: &Path, relative: &str, content: &[u8]) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[test]
fn insert_many_auto_mode_skips_ignored_folders() {
    let temp = tempfile::tempdir().unwrap();
    let repo_paths = init_repo(temp.path());
    let product_dir = temp.path().join("product");
    write_source_root_files(&product_dir, "v1.0.0-windows-x86_64-2026_07_14/app.exe", b"win-app");
    write_source_root_files(&product_dir, "v1.0.0-linux-x86_64-2026_07_14/app", b"linux-app");
    write_source_root_files(&product_dir, ".archives/old.zip", b"ignored");
    std::fs::write(
        product_dir.join(".gutch_source_config.json"),
        r#"{"ingestion_mode":"auto","auto_rules":{"ignore_paths":[".archives"]}}"#,
    )
    .unwrap();

    run_from_args([
        "pl-cli",
        "target",
        "add-many",
        "--product-dir",
        product_dir.to_str().unwrap(),
        "--repo",
        repo_paths.repo_root.to_str().unwrap(),
        "--yes",
    ])
    .unwrap();

    let status = gutch::check_gutch_repo(&repo_paths.repo_root).unwrap();
    let target_ids: Vec<&str> = status
        .targets
        .iter()
        .map(|target| target.target_id.as_str())
        .collect();
    // The ignored `.archives` folder must not become a target.
    assert_eq!(target_ids.len(), 2, "expected exactly two targets: {target_ids:?}");
}

#[test]
fn insert_many_explicit_mode_uses_nested_targets_and_declared_tags() {
    let temp = tempfile::tempdir().unwrap();
    let repo_paths = init_repo(temp.path());
    let product_dir = temp.path().join("product");
    write_source_root_files(
        &product_dir,
        "versions/4.6-stable/windows-x86_64/Godot_v4.6-stable_win64.exe",
        b"godot-win",
    );
    write_source_root_files(
        &product_dir,
        "versions/4.6-stable/linux-x86_64/Godot_v4.6-stable_linux",
        b"godot-linux",
    );
    // A nested folder that is NOT declared must be ignored by discovery.
    write_source_root_files(&product_dir, "versions/4.7-stable/macos-universal/Godot.app", b"x");
    std::fs::write(
        product_dir.join(".gutch_source_config.json"),
        r#"{
            "ingestion_mode": "explicit",
            "explicit_rules": {
                "targets": [
                    {"path": "versions/4.6-stable/windows-x86_64", "tags": ["version::4.6-stable", "os::windows", "arch::x86_64", "released_at::2026_07_14"]},
                    {"path": "./versions/4.6-stable/linux-x86_64", "tags": ["version::4.6-stable", "os::linux", "arch::x86_64", "released_at::2026_07_14"]}
                ]
            }
        }"#,
    )
    .unwrap();

    run_from_args([
        "pl-cli",
        "target",
        "add-many",
        "--product-dir",
        product_dir.to_str().unwrap(),
        "--repo",
        repo_paths.repo_root.to_str().unwrap(),
        "--yes",
    ])
    .unwrap();

    let status = gutch::check_gutch_repo(&repo_paths.repo_root).unwrap();
    assert_eq!(status.targets.len(), 2);
    let windows = status
        .targets
        .iter()
        .find(|target| target.target_tags.get_exclusive("os").as_deref() == Some("windows"))
        .expect("windows target");
    assert_eq!(windows.target_tags.get_exclusive("version").unwrap(), "4.6-stable");
    assert_eq!(windows.target_tags.get_exclusive("arch").unwrap(), "x86_64");
    let linux = status
        .targets
        .iter()
        .find(|target| target.target_tags.get_exclusive("os").as_deref() == Some("linux"))
        .expect("linux target");
    assert_eq!(linux.target_tags.get_exclusive("arch").unwrap(), "x86_64");
}

#[test]
fn insert_many_explicit_mode_errors_when_declared_version_is_missing() {
    let temp = tempfile::tempdir().unwrap();
    let repo_paths = init_repo(temp.path());
    let product_dir = temp.path().join("product");
    write_source_root_files(&product_dir, "some-folder/app.exe", b"app");
    std::fs::write(
        product_dir.join(".gutch_source_config.json"),
        r#"{
            "ingestion_mode": "explicit",
            "explicit_rules": {
                "targets": [
                    {"path": "some-folder", "tags": ["os::windows", "arch::x86_64"]}
                ]
            }
        }"#,
    )
    .unwrap();

    let result = run_from_args([
        "pl-cli",
        "target",
        "add-many",
        "--product-dir",
        product_dir.to_str().unwrap(),
        "--repo",
        repo_paths.repo_root.to_str().unwrap(),
        "--yes",
    ]);
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("missing a recognizable version tag"),
        "unexpected error: {message}"
    );
}

#[test]
fn insert_many_auto_creates_skeleton_config_on_first_use() {
    let temp = tempfile::tempdir().unwrap();
    let repo_paths = init_repo(temp.path());
    let product_dir = temp.path().join("product");
    write_source_root_files(&product_dir, "v1.0.0-windows-x86_64-2026_07_14/app.exe", b"app");
    assert!(!product_dir.join(".gutch_source_config.json").exists());

    run_from_args([
        "pl-cli",
        "target",
        "add-many",
        "--product-dir",
        product_dir.to_str().unwrap(),
        "--repo",
        repo_paths.repo_root.to_str().unwrap(),
        "--yes",
    ])
    .unwrap();

    assert!(product_dir.join(".gutch_source_config.json").is_file());
    let status = gutch::check_gutch_repo(&repo_paths.repo_root).unwrap();
    assert_eq!(status.targets.len(), 1);
}
