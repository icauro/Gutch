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
    snapshots: Vec<ExpectedSnapshot>,
}

#[derive(Debug, Deserialize)]
struct GeneratorSettings {
    seed: i64,
}

#[derive(Debug, Deserialize)]
struct ExpectedSnapshot {
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

fn assert_snapshot_matches_expected(
    expected_snapshot: &ExpectedSnapshot,
    original_snapshot_root: &Path,
    reconstructed_snapshot_root: &Path,
) {
    let expected_paths: BTreeSet<String> = expected_snapshot
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect();
    assert_eq!(
        expected_paths,
        file_paths(original_snapshot_root),
        "original snapshot tree differs from its test manifest: {}",
        expected_snapshot.path
    );
    assert_eq!(
        expected_paths,
        file_paths(reconstructed_snapshot_root),
        "reconstructed snapshot tree differs from its test manifest: {}",
        expected_snapshot.path
    );

    for expected_file in &expected_snapshot.files {
        for root in [original_snapshot_root, reconstructed_snapshot_root] {
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
fn checked_in_fixtures_reconstruct_all_snapshots_and_verify_repos() {
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
        assert!(!test_manifest.snapshots.is_empty());

        let repo_status = gutch::check_gutch_repo(&repo_root).expect("fixture repo should load");
        assert_eq!(repo_status.snapshots.len(), test_manifest.snapshots.len());
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

        for snapshot in &repo_status.snapshots {
            let version = snapshot
                .snapshot_tags
                .get_exclusive("version")
                .expect("fixture snapshot version");
            let os = snapshot
                .snapshot_tags
                .get_exclusive("os")
                .expect("fixture snapshot os");
            let arch = snapshot
                .snapshot_tags
                .get_exclusive("arch")
                .expect("fixture snapshot arch");
            let access = snapshot
                .snapshot_tags
                .get_exclusive("access")
                .expect("fixture snapshot access");
            let snapshot_path = format!("{version}-{os}-{arch}-{access}");
            let expected_snapshot = test_manifest
                .snapshots
                .iter()
                .find(|expected| expected.path == snapshot_path)
                .unwrap_or_else(|| panic!("missing expected snapshot {snapshot_path}"));
            let original_snapshot_root = original_root.join(&snapshot_path);
            let reconstructed_snapshot_root = output_root.path().join(&snapshot.snapshot_id);
            let snapshot_manifest =
                gutch::load_snapshot_manifest(&repo_paths, &snapshot.snapshot_id)
                    .expect("fixture snapshot manifest should load");

            reconstruct_from_repo(
                &snapshot_manifest,
                &repo_paths,
                &repo_manifest.compression_profile(),
                &reconstructed_snapshot_root,
                &RuntimeResourceHints::default(),
                ReconstructOptions::default(),
                &GutchOperationControl::default(),
            )
            .expect("fixture snapshot should reconstruct");

            assert_snapshot_matches_expected(
                expected_snapshot,
                &original_snapshot_root,
                &reconstructed_snapshot_root,
            );
        }
    }
}
