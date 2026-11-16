use super::*;

#[test]
fn incoming_snapshot_is_rejected_before_metadata_is_written() {
    let root = tempfile::tempdir().unwrap();
    let paths = RepoPaths::from_repo_root(root.path());
    crate::repo::paths::ensure_repo_layout(&paths, "test").unwrap();
    crate::repo::updates::set_repo_constraints(
        root.path(),
        crate::manifest::constraints::RepoConstraints {
            targets: vec![crate::filesystem::constraints::FilesystemTarget::WindowsNtfs],
            ..Default::default()
        },
    )
    .unwrap();
    let entries = vec![crate::manifest::SnapshotEntry::Directory(
        crate::manifest::DirectoryManifest {
            path: "CON".into(),
            read_only: false,
        },
    )];
    let id = crate::snapshot::identity::snapshot_id_from_entries(&entries);
    let mut manifest = crate::manifest::SnapshotManifest::new(id.clone(), Default::default());
    manifest.entries = entries;
    let blueprint_path = root.path().join("incoming.gtcbin");
    crate::manifest::blueprint_codec::write_snapshot_blueprint_binary(
        &blueprint_path,
        &manifest.blueprint(),
    )
    .unwrap();
    let bytes = std::fs::read(blueprint_path).unwrap();
    let error = write_remote_snapshot_artifacts(
        &paths,
        &id,
        &manifest,
        None,
        &bytes,
        &OperationControl::default(),
    )
    .unwrap_err();
    assert!(matches!(error, Error::PathValidation(_)));
    assert!(!paths.snapshots_root.join(id).exists());
}

fn bundle_source() -> (String, RemoteArtifactSource) {
    let bytes = b"remote bundle payload".repeat(100);
    let hash = crate::artifact::hash::ChunkHash::of(&bytes);
    let mut builder = crate::artifact::bundle::BundleBuilder::new(Default::default());
    builder.add_chunk(hash, bytes);
    let (id, bytes) = builder.build_with_id().unwrap();
    (id, RemoteArtifactSource::Bytes(bytes))
}

#[test]
fn streamed_bundle_is_verified_before_replacing_cached_content() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("bundle");
    let client = crate::transfer::http::build_download_client().unwrap();
    let control = OperationControl::default();
    let (id, source) = bundle_source();
    download_and_publish_bundle(&source, &client, &id, &destination, &control).unwrap();
    let healthy = std::fs::read(&destination).unwrap();
    let RemoteArtifactSource::Bytes(mut damaged) = source else {
        unreachable!()
    };
    damaged[0] ^= 1;
    assert!(
        download_and_publish_bundle(
            &RemoteArtifactSource::Bytes(damaged),
            &client,
            &id,
            &destination,
            &control,
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&destination).unwrap(), healthy);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn bounded_source_rejects_oversized_bytes_before_writing() {
    let client = crate::transfer::http::build_download_client().unwrap();
    let source = RemoteArtifactSource::Bytes(vec![1, 2, 3]);
    let mut output = Vec::new();
    assert!(
        write_source(
            &source,
            &client,
            2,
            &OperationControl::default(),
            &mut output
        )
        .is_err()
    );
    assert!(output.is_empty());
    write_source(
        &source,
        &client,
        3,
        &OperationControl::default(),
        &mut output,
    )
    .unwrap();
    assert_eq!(output, [1, 2, 3]);
}

#[test]
fn cancelled_bundle_publication_keeps_the_existing_cache() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("bundle");
    std::fs::write(&destination, b"previous").unwrap();
    let client = crate::transfer::http::build_download_client().unwrap();
    let control = OperationControl::default().with_cancellation(std::sync::Arc::new(
        crate::test_util::TestCancellation::cancelled(),
    ));
    let (id, source) = bundle_source();
    assert!(download_and_publish_bundle(&source, &client, &id, &destination, &control).is_err());
    assert_eq!(std::fs::read(destination).unwrap(), b"previous");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}
