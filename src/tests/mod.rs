mod module_boundaries;

use crate::capture::pipeline::build_from_directory;
use crate::capture::reuse::add_built_bundles_to_chunk_index;
use crate::installation::InstallationRequest;
use crate::installation::inspect::RepairClassification;
use crate::installation::inspect::VerifyMode;
use crate::installation::inspect::VerifyRequest;
use crate::installation::inspect::classify_directory;
use crate::installation::reconstruct_snapshot;
use crate::manifest::FileManifest;
use crate::manifest::SnapshotTagSet;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::OperationControl;
use crate::repo::index::load_chunk_index;
use crate::repo::index::write_chunk_index;
use crate::repo::paths::RepoPaths;
use crate::repo::paths::bundle_path;
use crate::repo::paths::bundle_path_in_root;
use crate::repo::paths::ensure_gutch_repo_layout;
use crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts;
use crate::runtime::RuntimeResourceHints;
use crate::test_util;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

fn exclusive_tags(version: &str, os: &str, arch: &str, tier: &str) -> SnapshotTagSet {
    let mut tags = SnapshotTagSet::default();
    tags.insert("version", version).unwrap();
    tags.insert("os", os).unwrap();
    tags.insert("arch", arch).unwrap();
    tags.insert("tier", tier).unwrap();
    tags
}

fn verify_test_directory(manifest: &SnapshotManifest, dir: &Path) -> Vec<String> {
    classify_directory(VerifyRequest {
        snapshot_manifest: manifest,
        dir,
        mode: VerifyMode::Fast,
        runtime_hints: &RuntimeResourceHints::default(),
        control: crate::progress::OperationControl::default(),
    })
    .unwrap()
    .0
    .into_messages()
}

fn classify_test_directory(manifest: &SnapshotManifest, dir: &Path) -> RepairClassification {
    classify_directory(VerifyRequest {
        snapshot_manifest: manifest,
        dir,
        mode: VerifyMode::Fast,
        runtime_hints: &RuntimeResourceHints::default(),
        control: crate::progress::OperationControl::default(),
    })
    .unwrap()
    .0
}

#[test]
fn test_apply_snapshot_ops_reuses_verified_source_destination() {
    let root = test_util::temp_path("gutch_test_apply_snapshot_ops");
    let v1_dir = root.join("v1");
    let v2_dir = root.join("v2");
    let repo_dir = root.join("repo");
    let destination_dir = root.join("destination");
    let staging_dir = root.join(".staging");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&v1_dir).unwrap();
    std::fs::create_dir_all(&v2_dir).unwrap();

    std::fs::write(v1_dir.join("shared.txt"), b"same content".repeat(200)).unwrap();
    std::fs::write(v1_dir.join("old.txt"), b"old content").unwrap();
    std::fs::write(v2_dir.join("shared.txt"), b"same content".repeat(200)).unwrap();
    std::fs::write(v2_dir.join("new.txt"), b"new content").unwrap();

    let tags = exclusive_tags("1.0.0", "windows", "x86_64", "base");
    let v1 = build_from_directory(&v1_dir, &tags).unwrap();
    let tags_v2 = exclusive_tags("2.0.0", "windows", "x86_64", "base");
    let v2 = build_from_directory(&v2_dir, &tags_v2).unwrap();

    std::fs::create_dir_all(&destination_dir).unwrap();
    std::fs::copy(
        v1_dir.join("shared.txt"),
        destination_dir.join("shared.txt"),
    )
    .unwrap();
    std::fs::copy(v1_dir.join("old.txt"), destination_dir.join("old.txt")).unwrap();
    std::fs::write(destination_dir.join("player.log"), b"local runtime log").unwrap();

    let repo_paths = RepoPaths::from_repo_root(&repo_dir);
    ensure_gutch_repo_layout(&repo_paths, "test").unwrap();
    write_snapshot_manifest_artifacts(&repo_paths, &v1.snapshot_manifest).unwrap();
    write_snapshot_manifest_artifacts(&repo_paths, &v2.snapshot_manifest).unwrap();
    for bundle in &v2.bundles {
        let bundle_path = bundle_path(&repo_paths, &bundle.bundle_id).unwrap();
        std::fs::create_dir_all(bundle_path.parent().unwrap()).unwrap();
        std::fs::copy(bundle.staged_file.path(), bundle_path).unwrap();
    }
    let mut chunk_index = load_chunk_index(&repo_paths).unwrap();
    add_built_bundles_to_chunk_index(&mut chunk_index, &v2.bundles).unwrap();
    write_chunk_index(&repo_paths, &chunk_index).unwrap();

    let result = reconstruct_snapshot(InstallationRequest {
        repo_path: repo_dir,
        snapshot_id: v2.snapshot_manifest.snapshot_id.clone(),
        destination_dir: destination_dir.clone(),
        staging_dir: Some(staging_dir.clone()),
        source_snapshot_id: Some(v1.snapshot_manifest.snapshot_id.clone()),
        source_dir: Some(destination_dir.clone()),
        runtime_hints: RuntimeResourceHints::default(),
        control: OperationControl::default(),
    })
    .unwrap()
    .reconstruct_result;

    assert!(
        result.reused_source_chunk_count > 0,
        "expected at least one source chunk to be reused"
    );
    assert!(destination_dir.join("shared.txt").exists());
    assert!(destination_dir.join("new.txt").exists());
    assert!(!destination_dir.join("old.txt").exists());
    assert!(!staging_dir.exists());

    let verify_errors = verify_test_directory(&v2.snapshot_manifest, &destination_dir);
    assert!(
        verify_errors.is_empty(),
        "verification errors: {:?}",
        verify_errors
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn test_gutch_repo_paths_from_store_root() {
    let paths = RepoPaths::from_store_root(Path::new("D:\\store"), "base").unwrap();

    assert_eq!(
        paths.repo_root,
        PathBuf::from("D:\\store")
            .join("v1")
            .join("repos")
            .join("base")
    );
    assert_eq!(
        paths.repo_metadata_path,
        paths.repo_root.join("repo_metadata.json")
    );
    assert_eq!(paths.snapshots_root, paths.repo_root.join("snapshots"));
    assert_eq!(paths.bundles_root, paths.repo_root.join("bundles"));
}

#[test]
fn test_ensure_gutch_repo_layout_writes_repo_manifest() {
    let repo_root = test_util::temp_path("gutch_test_gutch_repo_layout");
    let _ = std::fs::remove_dir_all(&repo_root);
    let paths = RepoPaths::from_repo_root(&repo_root);

    let repo_manifest = ensure_gutch_repo_layout(&paths, "base").unwrap();

    assert_eq!(repo_manifest.name, "base");
    assert!(paths.repo_metadata_path.exists());
    assert!(paths.snapshots_root.exists());
    assert!(paths.bundles_root.exists());

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn test_verify_directory_missing_file() {
    let dir = test_util::temp_path("gutch_test_verify");
    test_util::recreate_dir(&dir);

    let fm = FileManifest {
        path: "nonexistent.txt".to_string(),
        size: 10,
        executable: false,
        read_only: false,
        full_file_hash: "abc".to_string(),
        chunks: vec![],
    };

    let tags = exclusive_tags("v", "win", "x64", "base");
    let mut vm = SnapshotManifest::new("1".into(), tags);
    vm.push_file(fm);
    let errors = verify_test_directory(&vm, &dir);
    assert!(!errors.is_empty());
    assert!(errors[0].contains("missing"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_verify_directory_reports_extra_file() {
    let dir = test_util::temp_path("gutch_test_verify_extra");
    test_util::recreate_dir(&dir);

    std::fs::write(dir.join("expected.txt"), b"hello world").unwrap();
    std::fs::write(dir.join("extra.log"), b"left behind").unwrap();

    let tags = exclusive_tags("1.0.0", "windows", "x86_64", "base");
    let result = build_from_directory(&dir, &tags).unwrap();

    let mut manifest = result.snapshot_manifest.clone();
    manifest
        .entries
        .retain(|entry| entry.path() == "expected.txt");
    manifest.total_logical_size = b"hello world".len() as u64;

    let errors = verify_test_directory(&manifest, &dir);
    assert!(
        errors
            .iter()
            .any(|error| error == "extra filesystem entry: extra.log"),
        "expected extra file error, got {:?}",
        errors
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_classify_directory_separates_missing_corrupt_and_extra() {
    let dir = test_util::temp_path("gutch_test_classify_directory");
    test_util::recreate_dir(&dir);

    std::fs::write(dir.join("healthy.txt"), b"healthy file").unwrap();
    std::fs::write(dir.join("corrupt.txt"), b"original contents").unwrap();

    let tags = exclusive_tags("1.0.0", "windows", "x86_64", "base");
    let result = build_from_directory(&dir, &tags).unwrap();

    std::fs::remove_file(dir.join("healthy.txt")).unwrap();
    std::fs::write(dir.join("corrupt.txt"), b"modified contents").unwrap();
    std::fs::write(dir.join("notes.tmp"), b"user scratch").unwrap();

    let classification = classify_test_directory(&result.snapshot_manifest, &dir);

    assert_eq!(classification.missing_files.len(), 1);
    assert_eq!(classification.missing_files[0].path, "healthy.txt");
    assert!(!classification.corrupt_files.is_empty());
    assert!(
        classification
            .corrupt_files
            .iter()
            .any(|issue| issue.path == "corrupt.txt")
    );
    assert_eq!(classification.extra_files.len(), 1);
    assert_eq!(classification.extra_files[0].path, "notes.tmp");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_large_game_build_200mb() {
    let root = test_util::temp_path("gutch_test_200mb");
    let artifacts_dir = test_util::temp_path("gutch_test_200mb_artifacts");
    test_util::recreate_dir(&root);
    test_util::recreate_dir(&artifacts_dir);

    for sub in &["audio", "textures", "data"] {
        std::fs::create_dir_all(root.join(sub)).unwrap();
    }

    let t0 = Instant::now();

    test_util::make_file(
        root.join("game.exe"),
        42 * 1024 * 1024,
        test_util::Seed::Binary,
    );
    test_util::make_file(
        root.join("audio/eng.pak"),
        48 * 1024 * 1024,
        test_util::Seed::Audio,
    );
    test_util::make_file(
        root.join("audio/fr.pak"),
        32 * 1024 * 1024,
        test_util::Seed::Audio,
    );
    test_util::make_file(
        root.join("textures/characters.tex"),
        22 * 1024 * 1024,
        test_util::Seed::Texture,
    );
    test_util::make_file(
        root.join("textures/environment.tex"),
        22 * 1024 * 1024,
        test_util::Seed::Texture,
    );
    test_util::make_file(
        root.join("data/assets.pak"),
        38 * 1024 * 1024,
        test_util::Seed::Archive,
    );
    test_util::make_file(
        root.join("data/config.json"),
        2 * 1024 * 1024,
        test_util::Seed::Config,
    );
    test_util::make_file(root.join("readme.txt"), 500 * 1024, test_util::Seed::Text);

    let gen_ms = t0.elapsed().as_millis();

    let total_original = test_util::dir_size(&root);
    assert!(
        total_original >= 190 * 1024 * 1024,
        "expected >=190 MB, got {} MB",
        total_original / (1024 * 1024)
    );

    let tags = exclusive_tags("1.0.0", "windows", "x86_64", "base");
    let t1 = Instant::now();
    let result = build_from_directory(&root, &tags).unwrap();
    let build_ms = t1.elapsed().as_millis();

    let bundle_total: u64 = result.bundles.iter().map(|b| b.size).sum();
    let compression_pct = if result.snapshot_manifest.total_logical_size > 0 {
        100.0 * bundle_total as f64 / result.snapshot_manifest.total_logical_size as f64
    } else {
        0.0
    };

    let manifest_path = artifacts_dir.join("manifest.json");
    let manifest_json = serde_json::to_string_pretty(&result.snapshot_manifest.metadata()).unwrap();
    std::fs::write(&manifest_path, manifest_json).unwrap();
    for bundle in &result.bundles {
        let bundle_dir = artifacts_dir.join("bundles");
        std::fs::create_dir_all(&bundle_dir).unwrap();
        let bundle_path = bundle_path_in_root(&bundle_dir, &bundle.bundle_id).unwrap();
        std::fs::create_dir_all(bundle_path.parent().unwrap()).unwrap();
        std::fs::copy(bundle.staged_file.path(), bundle_path).unwrap();
    }
    let artifacts_total = test_util::dir_size(&artifacts_dir);

    let repo_root = test_util::temp_path("gutch_test_200mb_repo");
    let repo_paths = RepoPaths::from_repo_root(&repo_root);
    ensure_gutch_repo_layout(&repo_paths, "200mb-test").unwrap();
    write_snapshot_manifest_artifacts(&repo_paths, &result.snapshot_manifest).unwrap();
    for bundle in &result.bundles {
        let bundle_path = bundle_path(&repo_paths, &bundle.bundle_id).unwrap();
        std::fs::create_dir_all(bundle_path.parent().unwrap()).unwrap();
        std::fs::copy(bundle.staged_file.path(), bundle_path).unwrap();
    }
    let mut chunk_index = load_chunk_index(&repo_paths).unwrap();
    add_built_bundles_to_chunk_index(&mut chunk_index, &result.bundles).unwrap();
    write_chunk_index(&repo_paths, &chunk_index).unwrap();

    let t2 = Instant::now();
    let out_dir = test_util::temp_path("gutch_test_200mb_reconstructed");
    let _ = std::fs::remove_dir_all(&out_dir);
    reconstruct_snapshot(InstallationRequest {
        repo_path: repo_root.clone(),
        snapshot_id: result.snapshot_manifest.snapshot_id.clone(),
        destination_dir: out_dir.clone(),
        staging_dir: None,
        source_snapshot_id: None,
        source_dir: None,
        runtime_hints: RuntimeResourceHints::default(),
        control: OperationControl::default(),
    })
    .unwrap();
    let recon_ms = t2.elapsed().as_millis();

    let t3 = Instant::now();
    let errors = verify_test_directory(&result.snapshot_manifest, &out_dir);
    let verify_ms = t3.elapsed().as_millis();
    let pipeline_without_verify_ms = gen_ms + build_ms + recon_ms;

    assert!(errors.is_empty(), "verification errors: {:?}", errors);

    println!();
    println!("=== 200 MB mock game capture ===");
    println!(
        "  files:           {}",
        result.snapshot_manifest.file_count()
    );
    println!(
        "  total logical:   {:.1} MB",
        result.snapshot_manifest.total_logical_size as f64 / (1024.0 * 1024.0)
    );
    println!(
        "  total bundled:   {:.1} MB ({:.1}% of original)",
        bundle_total as f64 / (1024.0 * 1024.0),
        compression_pct
    );
    println!(
        "  artifacts dir:   {:.1} MB",
        artifacts_total as f64 / (1024.0 * 1024.0)
    );
    println!(
        "  unique chunks:   {}",
        result
            .snapshot_manifest
            .files()
            .flat_map(|f| f.chunks.iter())
            .count()
    );
    println!("  bundles:         {}", result.bundles.len());
    println!();
    println!("  gen files:       {} ms", gen_ms);
    println!("  capture:           {} ms", build_ms);
    println!("  reconstruct:     {} ms", recon_ms);
    println!("  total no verify: {} ms", pipeline_without_verify_ms);
    println!("  verify:          {} ms", verify_ms);
    println!(
        "  total pipeline:  {} ms",
        gen_ms + build_ms + recon_ms + verify_ms
    );
    println!();
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&artifacts_dir);
    let _ = std::fs::remove_dir_all(&repo_root);
    let _ = std::fs::remove_dir_all(&out_dir);
}
