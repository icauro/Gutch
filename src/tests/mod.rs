use super::*;
use crate::manifest::{FileManifest, TargetTagSet, target::TargetManifest};
use crate::test_util;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn exclusive_tags(version: &str, os: &str, arch: &str, tier: &str) -> TargetTagSet {
    let mut tags = TargetTagSet::default();
    tags.upsert_exclusive("version", version);
    tags.upsert_exclusive("os", os);
    tags.upsert_exclusive("arch", arch);
    tags.upsert_exclusive("tier", tier);
    tags
}

#[test]
fn test_build_and_reconstruct_roundtrip() {
    let dir = test_util::temp_path("gutch_test_roundtrip");
    test_util::recreate_dir(&dir);

    let sub = dir.join("subdir");
    std::fs::create_dir_all(&sub).unwrap();

    std::fs::write(
        dir.join("file1.txt"),
        b"hello world, this is file one ".repeat(50),
    )
    .unwrap();
    std::fs::write(
        dir.join("file2.txt"),
        b"goodbye world, this is file two ".repeat(80),
    )
    .unwrap();
    std::fs::write(sub.join("nested.txt"), b"nested content here ".repeat(30)).unwrap();

    let tags = exclusive_tags("1.0.0", "windows", "x86_64", "base");
    let result = build_from_directory(&dir, "base", "1", &tags).unwrap();

    assert!(!result.target_manifest.files.is_empty());

    let out_dir = test_util::temp_path("gutch_test_output");
    let _ = std::fs::remove_dir_all(&out_dir);

    let bundle_data: Vec<Vec<u8>> = result.bundles.iter().map(|b| b.data.clone()).collect();
    reconstruct_from_target_manifest(&result.target_manifest, &bundle_data, &out_dir).unwrap();

    let verify_errors = verify_directory(&result.target_manifest, &out_dir).unwrap();

    assert!(
        verify_errors.is_empty(),
        "verification errors: {:?}",
        verify_errors
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn test_apply_target_install_reuses_verified_source_install() {
    let root = test_util::temp_path("gutch_test_apply_target_install");
    let v1_dir = root.join("v1");
    let v2_dir = root.join("v2");
    let bundles_dir = root.join("bundles");
    let install_dir = root.join("install");
    let staging_dir = root.join(".staging");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&v1_dir).unwrap();
    std::fs::create_dir_all(&v2_dir).unwrap();
    std::fs::create_dir_all(&bundles_dir).unwrap();

    std::fs::write(v1_dir.join("shared.txt"), b"same content".repeat(200)).unwrap();
    std::fs::write(v1_dir.join("old.txt"), b"old content").unwrap();
    std::fs::write(v2_dir.join("shared.txt"), b"same content".repeat(200)).unwrap();
    std::fs::write(v2_dir.join("new.txt"), b"new content").unwrap();

    let tags = exclusive_tags("1.0.0", "windows", "x86_64", "base");
    let v1 = build_from_directory(&v1_dir, "base", "1", &tags).unwrap();
    let tags_v2 = exclusive_tags("2.0.0", "windows", "x86_64", "base");
    let v2 = build_from_directory(&v2_dir, "base", "2", &tags_v2).unwrap();

    let v1_bundle_data: Vec<Vec<u8>> = v1
        .bundles
        .iter()
        .map(|bundle| bundle.data.clone())
        .collect();
    reconstruct_from_target_manifest(&v1.target_manifest, &v1_bundle_data, &install_dir).unwrap();
    std::fs::write(install_dir.join("player.log"), b"local runtime log").unwrap();

    for bundle in &v2.bundles {
        std::fs::write(
            bundle_path_in_root(&bundles_dir, &bundle.bundle_id),
            &bundle.data,
        )
        .unwrap();
    }
    let bundle_paths: Vec<PathBuf> = v2
        .bundles
        .iter()
        .map(|bundle| bundle_path_in_root(&bundles_dir, &bundle.bundle_id))
        .collect();

    let result = apply_target_install_with_timings(
        &v2.target_manifest,
        &bundle_paths,
        &install_dir,
        &staging_dir,
        Some(TargetInstallApplySource {
            target_manifest: &v1.target_manifest,
            install_dir: &install_dir,
        }),
    )
    .unwrap();

    assert!(
        result.reused_source_chunk_count > 0,
        "expected at least one source chunk to be reused"
    );
    assert_eq!(
        result.backup_dir, None,
        "successful promotion should not report a deleted backup directory"
    );
    assert!(install_dir.join("shared.txt").exists());
    assert!(install_dir.join("new.txt").exists());
    assert!(!install_dir.join("old.txt").exists());
    assert!(!staging_dir.exists());

    let verify_errors = verify_directory(&v2.target_manifest, &install_dir).unwrap();
    assert!(
        verify_errors.is_empty(),
        "verification errors: {:?}",
        verify_errors
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn test_reconstruct_bundle_compressed_with_runtime_hints() {
    let root = test_util::temp_path("gutch_test_bundle_compressed_reconstruct");
    let input_dir = root.join("input");
    let bundles_dir = root.join("bundles");
    let out_dir = root.join("out");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&input_dir).unwrap();
    std::fs::create_dir_all(&bundles_dir).unwrap();

    std::fs::write(
        input_dir.join("shared.bin"),
        b"bundle compressed".repeat(512),
    )
    .unwrap();
    std::fs::write(input_dir.join("config.json"), br#"{"mode":"bundle"}"#).unwrap();

    let tags = exclusive_tags("3.0.0", "windows", "x86_64", "base");
    let mut config = BuildConfig::with_ids("base", "300", &tags);
    config.compression_profile =
        manifest::CompressionProfile::for_preset(manifest::CompressionPreset::Zstd3);
    let result = build_with_config(&input_dir, &config).unwrap().0;

    for bundle in &result.bundles {
        std::fs::write(
            bundle_path_in_root(&bundles_dir, &bundle.bundle_id),
            &bundle.data,
        )
        .unwrap();
    }
    let bundle_paths: Vec<PathBuf> = result
        .bundles
        .iter()
        .map(|bundle| bundle_path_in_root(&bundles_dir, &bundle.bundle_id))
        .collect();

    let (_, timings) = reconstruct_from_bundle_paths_with_timings_and_hints(
        &result.target_manifest,
        &bundle_paths,
        &out_dir,
        &crate::runtime::RuntimeResourceHints {
            memory_mode: crate::runtime::MemoryMode::Low,
            max_memory_mib: Some(1024),
            jobs: Some(1),
        },
    )
    .unwrap();

    let verify_errors = verify_directory(&result.target_manifest, &out_dir).unwrap();
    assert!(
        verify_errors.is_empty(),
        "verification errors: {:?}",
        verify_errors
    );
    assert_eq!(
        timings.runtime_plan.as_ref().map(|plan| plan.worker_count),
        Some(1)
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn test_high_memory_no_verify_reconstruct_roundtrip() {
    let root = test_util::temp_path("gutch_test_high_memory_no_verify_reconstruct");
    let input_dir = root.join("input");
    let bundles_dir = root.join("bundles");
    let out_dir = root.join("out");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&input_dir).unwrap();
    std::fs::create_dir_all(&bundles_dir).unwrap();

    std::fs::write(
        input_dir.join("a.bin"),
        b"direct bundle restore".repeat(1024),
    )
    .unwrap();
    std::fs::write(input_dir.join("b.bin"), b"second output file".repeat(512)).unwrap();

    let tags = exclusive_tags("3.1.0", "windows", "x86_64", "base");
    let mut config = BuildConfig::with_ids("base", "301", &tags);
    config.compression_profile =
        manifest::CompressionProfile::for_preset(manifest::CompressionPreset::Zstd3);
    let result = build_with_config(&input_dir, &config).unwrap().0;

    for bundle in &result.bundles {
        std::fs::write(
            bundle_path_in_root(&bundles_dir, &bundle.bundle_id),
            &bundle.data,
        )
        .unwrap();
    }
    let bundle_paths: Vec<PathBuf> = result
        .bundles
        .iter()
        .map(|bundle| bundle_path_in_root(&bundles_dir, &bundle.bundle_id))
        .collect();

    reconstruct_from_bundle_paths_and_verified_source_with_timings_and_hints_with_options(
        &result.target_manifest,
        &bundle_paths,
        None,
        &out_dir,
        &crate::runtime::RuntimeResourceHints {
            memory_mode: crate::runtime::MemoryMode::High,
            max_memory_mib: Some(1024),
            jobs: Some(2),
        },
        ReconstructOptions {
            verify_chunks: false,
            verify_files: false,
        },
    )
    .unwrap();

    let verify_errors = verify_directory(&result.target_manifest, &out_dir).unwrap();
    assert!(
        verify_errors.is_empty(),
        "verification errors: {:?}",
        verify_errors
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn test_gutch_repo_paths_from_store_root() {
    let paths = GutchRepoPaths::from_store_root(Path::new("D:\\store"), "base");

    assert_eq!(
        paths.repo_root,
        PathBuf::from("D:\\store")
            .join("v1")
            .join("repos")
            .join("base")
    );
    assert_eq!(
        paths.repo_manifest_path,
        paths.repo_root.join("repo_manifest.json")
    );
    assert_eq!(paths.targets_root, paths.repo_root.join("targets"));
    assert_eq!(paths.bundles_root, paths.repo_root.join("bundles"));
}

#[test]
fn test_ensure_gutch_repo_layout_writes_repo_manifest() {
    let repo_root = test_util::temp_path("gutch_test_gutch_repo_layout");
    let _ = std::fs::remove_dir_all(&repo_root);
    let paths = GutchRepoPaths::from_repo_root(&repo_root);

    let repo_manifest = ensure_gutch_repo_layout(&paths, "base").unwrap();

    assert_eq!(repo_manifest.name, "base");
    assert!(paths.repo_manifest_path.exists());
    assert!(paths.targets_root.exists());
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
        full_file_hash: "abc".to_string(),
        chunks: vec![],
    };

    let tags = exclusive_tags("v", "win", "x64", "base");
    let mut vm = TargetManifest::new("1".into(), tags);
    vm.files.push(fm);
    let errors = verify_directory(&vm, &dir).unwrap();
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
    let result = build_from_directory(&dir, "base", "1", &tags).unwrap();

    let mut manifest = result.target_manifest.clone();
    manifest.files.retain(|file| file.path == "expected.txt");
    manifest.total_logical_size = b"hello world".len() as u64;

    let errors = verify_directory(&manifest, &dir).unwrap();
    assert!(
        errors.iter().any(|error| error == "extra file: extra.log"),
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
    let result = build_from_directory(&dir, "base", "1", &tags).unwrap();

    std::fs::remove_file(dir.join("healthy.txt")).unwrap();
    std::fs::write(dir.join("corrupt.txt"), b"modified contents").unwrap();
    std::fs::write(dir.join("notes.tmp"), b"user scratch").unwrap();

    let classification = classify_directory(&result.target_manifest, &dir).unwrap();

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
    let result = build_from_directory(&root, "base", "1", &tags).unwrap();
    let build_ms = t1.elapsed().as_millis();

    let bundle_total: u64 = result.bundles.iter().map(|b| b.data.len() as u64).sum();
    let compression_pct = if result.target_manifest.total_logical_size > 0 {
        100.0 * bundle_total as f64 / result.target_manifest.total_logical_size as f64
    } else {
        0.0
    };

    let manifest_path = artifacts_dir.join("manifest.json");
    let manifest_json = serde_json::to_string_pretty(&result.target_manifest).unwrap();
    std::fs::write(&manifest_path, manifest_json).unwrap();
    for bundle in &result.bundles {
        let bundle_dir = artifacts_dir.join("bundles");
        std::fs::create_dir_all(&bundle_dir).unwrap();
        let bundle_path = bundle_path_in_root(&bundle_dir, &bundle.bundle_id);
        std::fs::write(bundle_path, &bundle.data).unwrap();
    }
    let artifacts_total = test_util::dir_size(&artifacts_dir);

    let t2 = Instant::now();
    let out_dir = test_util::temp_path("gutch_test_200mb_reconstructed");
    let _ = std::fs::remove_dir_all(&out_dir);
    let bundle_data: Vec<Vec<u8>> = result.bundles.iter().map(|b| b.data.clone()).collect();
    reconstruct_from_target_manifest(&result.target_manifest, &bundle_data, &out_dir).unwrap();
    let recon_ms = t2.elapsed().as_millis();

    let t3 = Instant::now();
    let errors = verify_directory(&result.target_manifest, &out_dir).unwrap();
    let verify_ms = t3.elapsed().as_millis();
    let pipeline_without_verify_ms = gen_ms + build_ms + recon_ms;

    assert!(errors.is_empty(), "verification errors: {:?}", errors);

    let est = estimate_build(
        &result.target_manifest,
        &Default::default(),
        1024 * 1024 * 1024,
    )
    .unwrap();

    println!();
    println!("=== 200 MB mock game build ===");
    println!("  files:           {}", result.target_manifest.files.len());
    println!(
        "  total logical:   {:.1} MB",
        result.target_manifest.total_logical_size as f64 / (1024.0 * 1024.0)
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
            .target_manifest
            .files
            .iter()
            .flat_map(|f| f.chunks.iter())
            .count()
    );
    println!("  bundles:         {}", result.bundles.len());
    println!();
    println!("  gen files:       {} ms", gen_ms);
    println!("  build:           {} ms", build_ms);
    println!("  reconstruct:     {} ms", recon_ms);
    println!("  total no verify: {} ms", pipeline_without_verify_ms);
    println!("  verify:          {} ms", verify_ms);
    println!(
        "  total pipeline:  {} ms",
        gen_ms + build_ms + recon_ms + verify_ms
    );
    println!();
    println!("  fresh install estimate:");
    println!("    missing chunks:      {}", est.missing_chunk_count);
    println!(
        "    missing logical:     {:.1} MB",
        est.missing_logical_bytes as f64 / (1024.0 * 1024.0)
    );
    println!(
        "    missing compressed:  {:.1} MB",
        est.missing_compressed_bytes as f64 / (1024.0 * 1024.0)
    );
    println!("    estimated bundles:   {}", est.required_bundle_count);

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&artifacts_dir);
    let _ = std::fs::remove_dir_all(&out_dir);
}
