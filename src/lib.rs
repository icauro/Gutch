pub mod build;
pub mod build_scan;
pub mod build_types;
pub mod bundle;
pub mod chunk;
pub mod compress;
pub mod error;
pub mod hash;
pub mod install;
pub mod manifest;
pub mod reconstruct;
pub mod repo;
#[cfg(test)]
mod test_util;
pub mod verify;

pub use build::{
    build_from_directory, build_from_directory_with_timings,
    build_from_directory_with_timings_against_existing_bundles,
    build_from_directory_with_timings_against_existing_store,
    build_from_directory_with_timings_against_existing_store_and_manifests,
    build_from_directory_with_timings_against_existing_store_and_manifests_and_profile,
    build_from_directory_with_timings_against_existing_store_and_manifests_and_profiles,
    build_repo_init_batch_with_timings,
};
pub use build_types::{
    BuildResult, BuildTimings, BuiltBundle, ExistingChunkLocation, RepoInitResult,
    RepoInitTargetInput,
};
pub use error::{Error, Result};
pub use install::{
    GutchBundleDownload, TargetInstallApplyResult, TargetInstallApplySource,
    TargetInstallFetchRequest, TargetInstallFetchResult, apply_target_install_with_timings,
    fetch_target_install_content,
};
pub use reconstruct::{
    ReconstructTimings, VerifiedSourceInstall,
    reconstruct_from_bundle_paths_and_verified_source_with_timings,
    reconstruct_from_bundle_paths_with_timings, reconstruct_from_target_manifest,
    reconstruct_from_target_manifest_with_timings,
};
pub use repo::{
    ContentRepoPaths, RepoInspectStatus, RepoInspectTargetStatus, SourceTargetInstallCandidate,
    canonical_target_tags, compute_target_id, discover_source_target_installs,
    ensure_content_repo_layout, inspect_content_repo, load_content_repo_manifest,
    target_manifest_path,
};
pub use verify::{
    RepairClassification, RepairIssue, RepairIssueKind, VerifyTimings, classify_directory,
    classify_directory_with_timings, estimate_build, verify_directory,
    verify_directory_with_timings,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{FileManifest, TargetManifest};
    use crate::test_util;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    #[test]
    fn test_build_and_reconstruct_roundtrip() {
        let dir = std::env::temp_dir().join("gutch_test_roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

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

        let result = build_from_directory(
            &dir,
            "test-product",
            "base",
            "1.0.0",
            "windows",
            "x86_64",
            "base",
        )
        .unwrap();

        assert!(!result.target_manifest.files.is_empty());
        assert!(!result.target_manifest.required_chunk_set.is_empty());

        let out_dir = std::env::temp_dir().join("gutch_test_output");
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
        let root = std::env::temp_dir().join("gutch_test_apply_target_install");
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

        let v1 = build_from_directory(
            &v1_dir,
            "test-product",
            "base",
            "1.0.0",
            "windows",
            "x86_64",
            "base",
        )
        .unwrap();
        let v2 = build_from_directory(
            &v2_dir,
            "test-product",
            "base",
            "2.0.0",
            "windows",
            "x86_64",
            "base",
        )
        .unwrap();

        let v1_bundle_data: Vec<Vec<u8>> = v1
            .bundles
            .iter()
            .map(|bundle| bundle.data.clone())
            .collect();
        reconstruct_from_target_manifest(&v1.target_manifest, &v1_bundle_data, &install_dir)
            .unwrap();
        std::fs::write(install_dir.join("player.log"), b"local runtime log").unwrap();

        for bundle in &v2.bundles {
            std::fs::write(
                bundles_dir.join(format!("{}.bundle", bundle.bundle_id)),
                &bundle.data,
            )
            .unwrap();
        }
        let bundle_paths: Vec<PathBuf> = v2
            .bundles
            .iter()
            .map(|bundle| bundles_dir.join(format!("{}.bundle", bundle.bundle_id)))
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
    fn test_repo_init_batch_builds_multiple_targets() {
        let root = std::env::temp_dir().join("gutch_test_repo_init_batch");
        let dir_a = root.join("a");
        let dir_b = root.join("b");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();

        std::fs::write(dir_a.join("shared.bin"), b"shared bytes".repeat(100)).unwrap();
        std::fs::write(dir_a.join("only-a.txt"), b"alpha").unwrap();
        std::fs::write(dir_b.join("shared.bin"), b"shared bytes".repeat(100)).unwrap();
        std::fs::write(dir_b.join("only-b.txt"), b"beta").unwrap();

        let inputs = vec![
            RepoInitTargetInput {
                dir: dir_a.clone(),
                version_id: "1.0.0".to_string(),
                platform: "windows".to_string(),
                architecture: "x86_64".to_string(),
                tier_id: "base".to_string(),
                tags: canonical_target_tags("1.0.0", "windows", "x86_64", "base"),
            },
            RepoInitTargetInput {
                dir: dir_b.clone(),
                version_id: "1.0.0".to_string(),
                platform: "windows".to_string(),
                architecture: "x86_64".to_string(),
                tier_id: "tr".to_string(),
                tags: canonical_target_tags("1.0.0", "windows", "x86_64", "tr"),
            },
        ];

        let (result, _) = build_repo_init_batch_with_timings(
            "test-product",
            "base",
            &inputs,
            &manifest::ChunkSizingProfile::default(),
            &manifest::BundleSizingProfile::default(),
            &manifest::CompressionProfile::default(),
        )
        .unwrap();

        assert_eq!(result.target_manifests.len(), 2);
        assert!(!result.bundles.is_empty());
        assert_eq!(result.target_manifests[0].content_repo_id, "base");
        assert!(
            result
                .target_manifests
                .iter()
                .all(|manifest| !manifest.required_bundle_set.is_empty())
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_content_repo_paths_from_store_root() {
        let paths = ContentRepoPaths::from_store_root(Path::new("D:\\store"), "prod-1", "base");

        assert_eq!(
            paths.repo_root,
            PathBuf::from("D:\\store")
                .join("v1")
                .join("products")
                .join("prod-1")
                .join("repos")
                .join("base")
        );
        assert_eq!(paths.repo_manifest_path, paths.repo_root.join("repo.json"));
        assert_eq!(paths.targets_root, paths.repo_root.join("targets"));
        assert_eq!(paths.bundles_root, paths.repo_root.join("bundles"));
    }

    #[test]
    fn test_ensure_content_repo_layout_writes_repo_manifest() {
        let repo_root = std::env::temp_dir().join("gutch_test_content_repo_layout");
        let _ = std::fs::remove_dir_all(&repo_root);
        let paths = ContentRepoPaths::from_repo_root(&repo_root);

        let repo_manifest = ensure_content_repo_layout(&paths, "prod-1", "base").unwrap();

        assert_eq!(repo_manifest.product_id, "prod-1");
        assert!(paths.repo_manifest_path.exists());
        assert!(paths.targets_root.exists());
        assert!(paths.bundles_root.exists());

        let _ = std::fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn test_verify_directory_missing_file() {
        let dir = std::env::temp_dir().join("gutch_test_verify");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let fm = FileManifest {
            path: "nonexistent.txt".to_string(),
            size: 10,
            mode: 0o644,
            executable: false,
            symlink_target: None,
            full_file_hash: "abc".to_string(),
            chunks: vec![],
        };

        let vm = TargetManifest::new(
            "p".into(),
            "base".into(),
            compute_target_id("v", "win", "x64", "base"),
            "v".into(),
            "win".into(),
            "x64".into(),
            "base".into(),
            canonical_target_tags("v", "win", "x64", "base"),
        );
        let mut vm = vm;
        vm.files.push(fm);
        let errors = verify_directory(&vm, &dir).unwrap();
        assert!(!errors.is_empty());
        assert!(errors[0].contains("missing"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_verify_directory_reports_extra_file() {
        let dir = std::env::temp_dir().join("gutch_test_verify_extra");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(dir.join("expected.txt"), b"hello world").unwrap();
        std::fs::write(dir.join("extra.log"), b"left behind").unwrap();

        let result = build_from_directory(
            &dir,
            "test-product",
            "base",
            "1.0.0",
            "windows",
            "x86_64",
            "base",
        )
        .unwrap();

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
        let dir = std::env::temp_dir().join("gutch_test_classify_directory");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(dir.join("healthy.txt"), b"healthy file").unwrap();
        std::fs::write(dir.join("corrupt.txt"), b"original contents").unwrap();

        let result = build_from_directory(
            &dir,
            "test-product",
            "base",
            "1.0.0",
            "windows",
            "x86_64",
            "base",
        )
        .unwrap();

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
        let root = std::env::temp_dir().join("gutch_test_200mb");
        let artifacts_dir = std::env::temp_dir().join("gutch_test_200mb_artifacts");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&artifacts_dir);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&artifacts_dir).unwrap();

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

        let t1 = Instant::now();
        let result = build_from_directory(
            &root,
            "mock-game",
            "base",
            "1.0.0",
            "windows",
            "x86_64",
            "base",
        )
        .unwrap();
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
            let bundle_path = bundle_dir.join(format!("{}.bundle", bundle.bundle_id));
            std::fs::write(bundle_path, &bundle.data).unwrap();
        }
        let artifacts_total = test_util::dir_size(&artifacts_dir);

        let t2 = Instant::now();
        let out_dir = std::env::temp_dir().join("gutch_test_200mb_reconstructed");
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
            result.target_manifest.required_chunk_set.len()
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
}
