use gutch::api::*;
use std::fs;

fn fixture() -> (tempfile::TempDir, InstallationRequest) {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::create_dir(source.join("empty")).unwrap();
    fs::write(source.join("healthy"), b"healthy content").unwrap();
    fs::write(source.join("damaged"), b"original content").unwrap();
    fs::write(source.join("missing"), b"missing content").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("healthy", source.join("link")).unwrap();
    let repo = root.path().join("repo");
    initialize_repo(&repo, RepoInitOptions::default()).unwrap();
    let capture = capture_snapshot(SnapshotCaptureRequest {
        repo_root: repo.clone(),
        source_dir: source,
        explicit_tags: SnapshotTagSet::default(),
        chunk_profile: Default::default(),
        chunker: gutch::api::Chunker::FastCdc,
        bundle_profile: Default::default(),
        compression_algorithm: CompressionKind::None,
        compression_level: None,
        storage_mode: ArtifactStorageMode::LooseChunks,
        runtime_hints: RuntimeResourceHints {
            jobs: Some(2),
            ..Default::default()
        },
        control: Default::default(),
    })
    .unwrap();
    let request = InstallationRequest {
        repo_path: repo,
        snapshot_id: capture.snapshot_id,
        destination_dir: root.path().join("installed"),
        staging_dir: None,
        source_snapshot_id: None,
        source_dir: None,
        runtime_hints: RuntimeResourceHints {
            jobs: Some(2),
            ..Default::default()
        },
        control: Default::default(),
    };
    reconstruct_snapshot(request.clone()).unwrap();
    (root, request)
}

#[test]
fn preview_streams_changes_and_selective_execution_preserves_healthy_files() {
    let (_root, request) = fixture();
    fs::write(request.destination_dir.join("damaged"), b"modified content").unwrap();
    fs::remove_file(request.destination_dir.join("missing")).unwrap();
    fs::write(request.destination_dir.join("extra"), b"user settings").unwrap();
    let healthy = request.destination_dir.join("healthy");
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(&healthy)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    let mut actions = std::collections::BTreeMap::new();
    let summary = visit_installation_plan(
        &request,
        InstallationPublication::Selective {
            extra_files: ExtraFilePolicy::Preserve,
        },
        InstallationInspection::Content,
        &mut |event| {
            if let InstallationPlanEvent::Entry { path, action, .. } = event {
                actions.insert(path, action);
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(actions["healthy"], InstallationAction::Keep);
    assert_eq!(actions["damaged"], InstallationAction::Replace);
    assert_eq!(actions["missing"], InstallationAction::Add);
    assert_eq!(actions["extra"], InstallationAction::PreserveExtra);
    assert_eq!(summary.staging_file_bytes, 31);
    assert!(!request.destination_dir.join("missing").exists());
    // Changes after a preview are reclassified during execution.
    fs::write(request.destination_dir.join("damaged"), b"different again").unwrap();
    install_snapshot_from_repo(
        request.clone(),
        InstallationPublication::Selective {
            extra_files: ExtraFilePolicy::Preserve,
        },
    )
    .unwrap();
    assert_eq!(
        fs::read(request.destination_dir.join("damaged")).unwrap(),
        b"original content"
    );
    assert_eq!(
        fs::read(request.destination_dir.join("missing")).unwrap(),
        b"missing content"
    );
    assert_eq!(
        fs::read(request.destination_dir.join("extra")).unwrap(),
        b"user settings"
    );
    assert_eq!(fs::metadata(healthy).unwrap().modified().unwrap(), modified);
}

#[test]
fn reconstruction_salvages_healthy_files_from_a_partially_damaged_source() {
    let (_root, mut request) = fixture();
    fs::write(request.destination_dir.join("damaged"), b"broken").unwrap();
    request.source_snapshot_id = Some(request.snapshot_id.clone());
    let result = reconstruct_snapshot(request.clone()).unwrap();
    assert!(result.reconstruct_result.reused_source_chunk_count > 0);
    assert_eq!(
        fs::read(request.destination_dir.join("damaged")).unwrap(),
        b"original content"
    );
    assert_eq!(
        fs::read(request.destination_dir.join("healthy")).unwrap(),
        b"healthy content"
    );
}

#[test]
fn metadata_only_preview_does_not_claim_content_integrity() {
    let (_root, request) = fixture();
    fs::write(request.destination_dir.join("damaged"), b"modified content").unwrap();
    let summary = visit_installation_plan(
        &request,
        InstallationPublication::Selective {
            extra_files: ExtraFilePolicy::Remove,
        },
        InstallationInspection::MetadataOnly,
        &mut |event| {
            if let InstallationPlanEvent::Entry {
                content_verified, ..
            } = event
            {
                assert!(!content_verified);
            }
            Ok(())
        },
    )
    .unwrap();
    assert!(!summary.content_inspected);
    assert_eq!(summary.staging_file_bytes, 0);
}

#[test]
fn permission_repair_keeps_verified_content_in_place() {
    let (_root, request) = fixture();
    let file = request.destination_dir.join("healthy");
    let before = fs::metadata(&file).unwrap().modified().unwrap();
    let mut permissions = fs::metadata(&file).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&file, permissions).unwrap();
    let mut saw_metadata = false;
    let summary = visit_installation_plan(
        &request,
        InstallationPublication::Selective {
            extra_files: ExtraFilePolicy::Preserve,
        },
        InstallationInspection::Content,
        &mut |event| {
            if let InstallationPlanEvent::Entry { path, action, .. } = event {
                if path == "healthy" {
                    saw_metadata = action == InstallationAction::UpdateMetadata;
                }
            }
            Ok(())
        },
    )
    .unwrap();
    assert!(saw_metadata);
    assert_eq!(summary.staging_file_bytes, 0);
    install_snapshot_from_repo(
        request,
        InstallationPublication::Selective {
            extra_files: ExtraFilePolicy::Preserve,
        },
    )
    .unwrap();
    assert!(!fs::metadata(&file).unwrap().permissions().readonly());
    assert_eq!(fs::metadata(&file).unwrap().modified().unwrap(), before);
}

#[cfg(unix)]
#[test]
fn repairing_a_link_does_not_replace_its_healthy_target() {
    let (_root, request) = fixture();
    let healthy = request.destination_dir.join("healthy");
    let before = fs::metadata(&healthy).unwrap().modified().unwrap();
    fs::remove_file(request.destination_dir.join("link")).unwrap();
    install_snapshot_from_repo(
        request.clone(),
        InstallationPublication::Selective {
            extra_files: ExtraFilePolicy::Preserve,
        },
    )
    .unwrap();
    assert_eq!(fs::metadata(&healthy).unwrap().modified().unwrap(), before);
    assert_eq!(
        fs::read(request.destination_dir.join("link")).unwrap(),
        b"healthy content"
    );
}

#[test]
fn removing_extra_content_preserves_expected_empty_directories() {
    let (_root, request) = fixture();
    let expected_directory = request.destination_dir.join("empty");
    fs::write(expected_directory.join("user-file"), b"extra content").unwrap();
    let extra_directory = request.destination_dir.join("extra-directory");
    fs::create_dir(&extra_directory).unwrap();
    fs::write(extra_directory.join("user-file"), b"extra content").unwrap();
    install_snapshot_from_repo(
        request,
        InstallationPublication::Selective {
            extra_files: ExtraFilePolicy::Remove,
        },
    )
    .unwrap();
    assert!(expected_directory.is_dir());
    assert!(!expected_directory.join("user-file").exists());
    assert!(!extra_directory.exists());
}
