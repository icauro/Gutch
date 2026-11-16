use super::*;
use crate::artifact::hash::ChunkHash;

fn save_test_snapshot(paths: &RepoPaths, path: &str, bytes: &[u8]) -> SnapshotManifest {
    let hash = crate::artifact::hash::ChunkHash::of(bytes);
    let mut manifest = SnapshotManifest::new(String::new(), Default::default());
    manifest.push_file(crate::manifest::FileManifest {
        path: path.into(),
        size: bytes.len() as u64,
        executable: false,
        read_only: false,
        full_file_hash: hash,
        chunks: if bytes.is_empty() {
            Vec::new()
        } else {
            vec![crate::manifest::ChunkRef {
                chunk_hash: hash,
                raw_size: bytes.len() as u32,
            }]
        },
    });
    manifest.snapshot_id = crate::snapshot::identity::snapshot_id_from_entries(&manifest.entries);
    crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts(
        paths,
        &manifest,
        &crate::progress::OperationControl::default(),
    )
    .unwrap();
    manifest
}

#[test]
fn selection_limits_payload_checks_but_validates_unselected_chunk_references() {
    let root = tempfile::tempdir().unwrap();
    let paths = RepoPaths::from_repo_root(root.path());
    crate::repo::paths::ensure_repo_layout(&paths, "selection").unwrap();
    let selected = save_test_snapshot(&paths, "empty", b"");
    let unselected = save_test_snapshot(&paths, "missing", b"unavailable payload");
    crate::test_util::index_manifest(&paths, &unselected);

    for mode in [RepoVerifyMode::Fast, RepoVerifyMode::Strict] {
        let result = verify_repo(&crate::repo::verify::RepoVerifyRequest {
            repo_root: root.path().to_path_buf(),
            snapshot_id: Some(selected.snapshot_id.clone()),
            mode,
            control: Default::default(),
        })
        .unwrap();
        assert!(result.ok);
        assert_eq!(result.snapshot_count, 1);
        assert_eq!(result.snapshots[0].snapshot_id, selected.snapshot_id);
        assert_eq!(result.snapshots[0].required_artifact_count, 0);
        let all = verify_repo(&crate::repo::verify::RepoVerifyRequest {
            repo_root: root.path().to_path_buf(),
            snapshot_id: None,
            mode,
            control: Default::default(),
        })
        .unwrap();
        assert!(!all.ok);
        assert_eq!(all.snapshot_count, 2);
        assert_eq!(all.snapshots_ok, 1);
        let missing = all
            .snapshots
            .iter()
            .find(|item| item.snapshot_id == unselected.snapshot_id)
            .unwrap();
        assert_eq!(missing.missing_bundle_count, 1);
        assert_eq!(missing.missing_artifact_count, 1);
    }

    let mut index = load_chunk_index(&paths).unwrap();
    let chunk = unselected.files().next().unwrap().chunks[0].chunk_hash;
    let mut entry = index.entries.get(&chunk).unwrap().clone();
    entry.raw_size += 1;
    index.entries.insert(chunk, entry);
    crate::repo::index::write_chunk_index(&paths, &index).unwrap();
    let error = verify_repo(&crate::repo::verify::RepoVerifyRequest {
        repo_root: root.path().to_path_buf(),
        snapshot_id: Some(selected.snapshot_id.clone()),
        mode: RepoVerifyMode::Fast,
        control: Default::default(),
    })
    .unwrap_err();
    assert!(error.to_string().contains("chunk index size disagrees"));

    crate::repo::index::write_chunk_index(&paths, &crate::repo::index::empty_chunk_index())
        .unwrap();
    let error = verify_repo(&crate::repo::verify::RepoVerifyRequest {
        repo_root: root.path().to_path_buf(),
        snapshot_id: Some(selected.snapshot_id.clone()),
        mode: RepoVerifyMode::Fast,
        control: Default::default(),
    })
    .unwrap_err();
    assert!(error.to_string().contains("missing chunk index entry"));

    crate::test_util::index_manifest(&paths, &unselected);
    let mut invalid = unselected;
    let crate::manifest::SnapshotEntry::File(file) = &mut invalid.entries[0] else {
        panic!("expected the fixture's file entry");
    };
    file.full_file_hash = ChunkHash::of(b"different content");
    crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts(
        &paths,
        &invalid,
        &crate::progress::OperationControl::default(),
    )
    .unwrap();
    let error = verify_repo(&crate::repo::verify::RepoVerifyRequest {
        repo_root: root.path().to_path_buf(),
        snapshot_id: Some(selected.snapshot_id.clone()),
        mode: RepoVerifyMode::Fast,
        control: Default::default(),
    })
    .unwrap_err();
    assert!(matches!(error, Error::SnapshotIntegrityMismatch { .. }));
}

#[test]
fn empty_repository_succeeds_but_unknown_snapshot_is_an_error() {
    let root = tempfile::tempdir().unwrap();
    let paths = RepoPaths::from_repo_root(root.path());
    crate::repo::paths::ensure_repo_layout(&paths, "empty").unwrap();
    let result = verify_repo(&crate::repo::verify::RepoVerifyRequest {
        repo_root: root.path().to_path_buf(),
        snapshot_id: None,
        mode: RepoVerifyMode::Fast,
        control: Default::default(),
    })
    .unwrap();
    assert!(result.ok);
    assert_eq!(result.snapshot_count, 0);
    let unknown = "a".repeat(64);
    let error = verify_repo(&crate::repo::verify::RepoVerifyRequest {
        repo_root: root.path().to_path_buf(),
        snapshot_id: Some(unknown.clone()),
        mode: RepoVerifyMode::Fast,
        control: Default::default(),
    })
    .unwrap_err();
    assert!(error.to_string().contains("snapshot not found in repo"));
}

#[test]
fn structural_verification_does_not_claim_content_addressed_identity() {
    let profile = crate::artifact::profiles::CompressionProfile::default();
    let mut builder = bundle::BundleBuilder::new(profile.clone());
    let bytes = b"verified bundle payload";
    builder.add_chunk(crate::artifact::hash::ChunkHash::of(bytes), bytes.to_vec());
    let (bundle_id, encoded) = builder.build_with_id().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bundle");
    std::fs::write(&path, encoded).unwrap();
    verify_bundle_file(&path, &bundle_id, RepoVerifyMode::Strict).unwrap();
    let incorrect_id = "0".repeat(64);
    verify_bundle_file(&path, &incorrect_id, RepoVerifyMode::Fast).unwrap();
    assert!(verify_bundle_file(&path, &incorrect_id, RepoVerifyMode::Strict).is_err());
}
