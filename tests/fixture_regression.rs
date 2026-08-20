use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use gutch::GutchOperationControl;
use gutch::reconstruct::{ReconstructOptions, reconstruct_from_repo};
use gutch::runtime::RuntimeResourceHints;
use gutch::verify::VerifyMode;
use serde::Deserialize;

const FIXTURE_ROOTS: &[&str] = &[
    "tests/fixtures/small-product",
    "tests/fixtures/append-product",
    "tests/fixtures/filesystem-product",
];

#[derive(Debug, Deserialize)]
struct TestManifest {
    generator: GeneratorSettings,
    targets: Vec<ExpectedTarget>,
}

#[derive(Debug, Deserialize)]
struct GeneratorSettings {
    seed: i64,
}

#[derive(Debug, Deserialize)]
struct ExpectedTarget {
    path: String,
    files: Vec<ExpectedFile>,
}

#[derive(Debug, Deserialize)]
struct ExpectedFile {
    path: String,
    size_bytes: u64,
    blake3: String,
}

fn fixture_root(relative_path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path)
}

fn load_test_manifest(original_root: &Path) -> TestManifest {
    let manifest_path = original_root.join(".gutch-test-manifest.json");
    serde_json::from_str(
        &std::fs::read_to_string(&manifest_path)
            .unwrap_or_else(|error| panic!("read {}: {error}", manifest_path.display())),
    )
    .unwrap_or_else(|error| panic!("parse {}: {error}", manifest_path.display()))
}

fn file_paths(root: &Path) -> BTreeSet<String> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            entry
                .path()
                .strip_prefix(root)
                .expect("walked path should be below root")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

fn assert_target_matches_expected(
    expected_target: &ExpectedTarget,
    original_target_root: &Path,
    reconstructed_target_root: &Path,
) {
    let expected_paths: BTreeSet<String> = expected_target
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect();
    assert_eq!(
        expected_paths,
        file_paths(original_target_root),
        "original target tree differs from its test manifest: {}",
        expected_target.path
    );
    assert_eq!(
        expected_paths,
        file_paths(reconstructed_target_root),
        "reconstructed target tree differs from its test manifest: {}",
        expected_target.path
    );

    for expected_file in &expected_target.files {
        for root in [original_target_root, reconstructed_target_root] {
            let path = root.join(&expected_file.path);
            assert_eq!(
                std::fs::metadata(&path)
                    .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
                    .len(),
                expected_file.size_bytes,
                "size mismatch for {} in {}",
                expected_file.path,
                root.display()
            );
            let hash = gutch::hash::hash_hex(
                &gutch::hash::hash_file(&path)
                    .unwrap_or_else(|error| panic!("hash {}: {error}", path.display())),
            );
            assert_eq!(
                hash,
                expected_file.blake3,
                "BLAKE3 mismatch for {} in {}",
                expected_file.path,
                root.display()
            );
        }
    }
}

#[test]
fn checked_in_fixtures_reconstruct_all_targets_and_verify_repos() {
    for fixture_relative_path in FIXTURE_ROOTS {
        let fixture_root = fixture_root(fixture_relative_path);
        let original_root = fixture_root.join("original");
        let repo_root = fixture_root.join("repo");
        let test_manifest = load_test_manifest(&original_root);
        let expected_seed = match *fixture_relative_path {
            "tests/fixtures/small-product" => 20_260_820,
            "tests/fixtures/append-product" => 20_260_821,
            "tests/fixtures/filesystem-product" => 20_260_822,
            other => panic!("unregistered fixture path: {other}"),
        };

        assert_eq!(test_manifest.generator.seed, expected_seed);
        assert!(!test_manifest.targets.is_empty());

        let repo_status = gutch::check_gutch_repo(&repo_root).expect("fixture repo should load");
        assert_eq!(repo_status.targets.len(), test_manifest.targets.len());
        let verification = gutch::verify_gutch_repo(&repo_root, None, VerifyMode::Strict)
            .expect("fixture repo should verify");
        assert!(
            verification.ok,
            "fixture repo verification failed for {fixture_relative_path}: {verification:?}"
        );

        let repo_paths = gutch::GutchRepoPaths::from_repo_root(&repo_root);
        let (_, repo_manifest) =
            gutch::load_gutch_repo_manifest(&repo_root).expect("fixture repo manifest should load");
        let output_root = tempfile::tempdir().expect("reconstruction output tempdir");

        for target in &repo_status.targets {
            let version = target
                .target_tags
                .get_exclusive("version")
                .expect("fixture target version");
            let os = target
                .target_tags
                .get_exclusive("os")
                .expect("fixture target os");
            let arch = target
                .target_tags
                .get_exclusive("arch")
                .expect("fixture target arch");
            let access = target
                .target_tags
                .get_exclusive("access")
                .expect("fixture target access");
            let target_path = format!("{version}-{os}-{arch}-{access}");
            let expected_target = test_manifest
                .targets
                .iter()
                .find(|expected| expected.path == target_path)
                .unwrap_or_else(|| panic!("missing expected target {target_path}"));
            let original_target_root = original_root.join(&target_path);
            let reconstructed_target_root = output_root.path().join(&target.target_id);
            let target_manifest = gutch::load_target_manifest(&repo_paths, &target.target_id)
                .expect("fixture target manifest should load");

            reconstruct_from_repo(
                &target_manifest,
                &repo_paths,
                &repo_manifest.compression_profile(),
                &reconstructed_target_root,
                &RuntimeResourceHints::default(),
                ReconstructOptions::default(),
                &GutchOperationControl::default(),
            )
            .expect("fixture target should reconstruct");

            assert_target_matches_expected(
                expected_target,
                &original_target_root,
                &reconstructed_target_root,
            );
        }
    }
}
