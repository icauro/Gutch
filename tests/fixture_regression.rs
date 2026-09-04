use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use gutch::GutchOperationControl;
use gutch::{
    RuntimeResourceHints, SnapshotReconstructFromRepoRequest, VerifyMode,
    reconstruct_snapshot_from_repo,
};
use serde::Deserialize;

const TEST_DATASET_PRODUCTS: &[(&str, i64)] = &[
    ("mock-compact-baseline", 20_260_820),
    ("mock-version-churn", 20_260_821),
    ("mock-filesystem-metadata", 20_260_822),
];

#[derive(Debug, Deserialize)]
struct TestManifest {
    schema_version: u64,
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
    entries: Vec<ExpectedEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "entry_type", rename_all = "snake_case")]
enum ExpectedEntry {
    File {
        path: String,
        size_bytes: u64,
        blake3: String,
        executable: bool,
        read_only: bool,
    },
    Directory {
        path: String,
        read_only: bool,
    },
    Symlink {
        path: String,
        target_path: String,
    },
}

impl ExpectedEntry {
    fn path(&self) -> &str {
        match self {
            Self::File { path, .. } | Self::Directory { path, .. } | Self::Symlink { path, .. } => {
                path
            }
        }
    }
}

fn dataset_path(relative_path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path)
}

fn load_test_manifest(repo_root: &Path) -> TestManifest {
    let manifest_path = repo_root.join(".gutch-test-manifest.json");
    serde_json::from_str(
        &std::fs::read_to_string(&manifest_path)
            .unwrap_or_else(|error| panic!("read {}: {error}", manifest_path.display())),
    )
    .unwrap_or_else(|error| panic!("parse {}: {error}", manifest_path.display()))
}

fn entry_paths(root: &Path) -> BTreeSet<String> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.depth() > 0)
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
    original_snapshot_root: Option<&Path>,
    reconstructed_snapshot_root: &Path,
) {
    let expected_paths: BTreeSet<String> = expected_snapshot
        .entries
        .iter()
        .map(|entry| entry.path().to_string())
        .collect();
    assert_eq!(
        expected_paths,
        entry_paths(reconstructed_snapshot_root),
        "reconstructed snapshot tree differs from its test manifest: {}",
        expected_snapshot.path
    );

    if let Some(original_root) = original_snapshot_root {
        assert_eq!(
            expected_paths,
            entry_paths(original_root),
            "original snapshot tree differs from its test manifest: {}",
            expected_snapshot.path
        );
    }

    for expected_entry in &expected_snapshot.entries {
        let roots = original_snapshot_root
            .into_iter()
            .chain(std::iter::once(reconstructed_snapshot_root));
        for root in roots {
            let path = root.join(expected_entry.path());
            let metadata = std::fs::symlink_metadata(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            match expected_entry {
                ExpectedEntry::File {
                    size_bytes,
                    blake3,
                    executable,
                    read_only,
                    ..
                } => {
                    assert!(metadata.file_type().is_file());
                    assert_eq!(metadata.len(), *size_bytes);
                    assert_eq!(metadata.permissions().readonly(), *read_only);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        assert_eq!(metadata.permissions().mode() & 0o111 != 0, *executable);
                    }
                    #[cfg(not(unix))]
                    let _ = executable;
                    let hash = blake3::hash(&std::fs::read(&path).unwrap())
                        .to_hex()
                        .to_string();
                    assert_eq!(&hash, blake3);
                }
                ExpectedEntry::Directory { read_only, .. } => {
                    assert!(metadata.file_type().is_dir());
                    assert_eq!(metadata.permissions().readonly(), *read_only);
                }
                ExpectedEntry::Symlink { target_path, .. } => {
                    assert!(metadata.file_type().is_symlink());
                    let resolved = path
                        .parent()
                        .unwrap()
                        .join(std::fs::read_link(&path).unwrap());
                    assert_eq!(
                        resolved.canonicalize().unwrap(),
                        root.join(target_path).canonicalize().unwrap()
                    );
                }
            }
        }
    }
}

#[test]
fn checked_in_fixtures_reconstruct_all_snapshots_and_verify_repos() {
    for (product_name, expected_seed) in TEST_DATASET_PRODUCTS {
        let source_root = dataset_path("tests/test_dataset/source").join(product_name);
        let repo_root = dataset_path("tests/test_dataset/repos/v1").join(product_name);
        let test_manifest = load_test_manifest(&repo_root);

        assert_eq!(test_manifest.generator.seed, *expected_seed);
        assert_eq!(test_manifest.schema_version, 1);
        assert!(!test_manifest.snapshots.is_empty());

        let repo_status = gutch::check_gutch_repo(&repo_root).expect("fixture repo should load");
        assert_eq!(repo_status.snapshots.len(), test_manifest.snapshots.len());
        let verification = gutch::verify_gutch_repo_artifacts(&repo_root, None, VerifyMode::Strict)
            .expect("fixture repo should verify");
        assert!(
            verification.ok,
            "fixture repo verification failed for {product_name}: {verification:?}"
        );

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
            let original_snapshot_path = source_root.join(&snapshot_path);
            let original_snapshot_root = original_snapshot_path
                .is_dir()
                .then_some(original_snapshot_path.as_path());
            let reconstructed_snapshot_root = output_root.path().join(&snapshot.snapshot_id);
            reconstruct_snapshot_from_repo(SnapshotReconstructFromRepoRequest {
                repo_path: repo_root.clone(),
                snapshot_id: snapshot.snapshot_id.clone(),
                destination_dir: reconstructed_snapshot_root.clone(),
                staging_dir: None,
                source_snapshot_id: None,
                source_dir: None,
                runtime_hints: RuntimeResourceHints::default(),
                control: GutchOperationControl::default(),
            })
            .expect("fixture snapshot should reconstruct");

            assert_snapshot_matches_expected(
                expected_snapshot,
                original_snapshot_root,
                &reconstructed_snapshot_root,
            );
        }
    }
}

#[test]
#[ignore = "official Blender snapshots are large; run this explicitly in the fixture lane"]
fn checked_in_blender_repo_is_processable() {
    let repo_root = dataset_path("tests/test_dataset/repos/v1/Blender");
    let repo_status = gutch::check_gutch_repo(&repo_root).expect("Blender repo should load");
    assert_eq!(repo_status.snapshots.len(), 4);

    let expected_targets = [
        ("5.2.0", "linux", "x86_64"),
        ("5.2.0", "windows", "x86_64"),
        ("5.2.1", "linux", "x86_64"),
        ("5.2.1", "windows", "x86_64"),
    ];
    let actual_targets: BTreeSet<(String, String, String)> = repo_status
        .snapshots
        .iter()
        .map(|snapshot| {
            (
                snapshot
                    .snapshot_tags
                    .get_exclusive("version")
                    .expect("Blender snapshot version")
                    .to_string(),
                snapshot
                    .snapshot_tags
                    .get_exclusive("os")
                    .expect("Blender snapshot OS")
                    .to_string(),
                snapshot
                    .snapshot_tags
                    .get_exclusive("arch")
                    .expect("Blender snapshot architecture")
                    .to_string(),
            )
        })
        .collect();
    let expected_targets = expected_targets
        .into_iter()
        .map(|(version, os, arch)| (version.to_string(), os.to_string(), arch.to_string()))
        .collect();
    assert_eq!(actual_targets, expected_targets);

    let verification = gutch::verify_gutch_repo_artifacts(&repo_root, None, VerifyMode::Strict)
        .expect("Blender repo should verify");
    assert!(
        verification.ok,
        "Blender repo verification failed: {verification:?}"
    );
}
