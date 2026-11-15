use super::*;

fn scan_dir(root: &std::path::Path) -> SourceInventory {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    scan_source_inventory(
        root,
        &crate::progress::OperationControl::default(),
        1,
        &pool,
    )
    .unwrap()
}

fn write_source(root: &std::path::Path, name: &str, content: &[u8]) -> std::path::PathBuf {
    let path = root.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, content).unwrap();
    path
}

#[test]
fn parallel_scan_preserves_serial_manifest_and_chunk_order() {
    let temp = tempfile::tempdir().unwrap();
    for index in 0..32 {
        let size = if index < 4 { 256 * 1024 } else { index * 37 };
        write_source(
            temp.path(),
            &format!("{index:02}.bin"),
            &vec![index as u8; size],
        );
    }
    let inventory = scan_dir(temp.path());
    let scan = |workers| {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .unwrap();
        scan_directory_into_file_entries(
            &inventory,
            ScanDirectoryOptions {
                chunking_profile: &crate::artifact::profiles::ChunkSizingProfile::default(),
                available_chunk_sizes: &HashMap::new(),
                worker_count: workers,
                worker_pool: &pool,
                control: crate::progress::OperationControl::default(),
            },
        )
        .unwrap()
    };
    let serial = scan(1);
    let parallel = scan(4);
    for (expected, actual) in serial.iter().zip(&parallel) {
        assert_eq!(
            serde_json::to_vec(&expected.manifest).unwrap(),
            serde_json::to_vec(&actual.manifest).unwrap()
        );
        assert_eq!(expected.new_chunk_offsets, actual.new_chunk_offsets);
    }
    assert_eq!(serial.len(), parallel.len());
}

#[test]
fn stable_source_verifies() {
    let temp = tempfile::tempdir().unwrap();
    write_source(temp.path(), "game.exe", b"content");
    write_source(temp.path(), "sub/readme.txt", b"readme");
    let before = scan_dir(temp.path());
    let after = scan_dir(temp.path());
    verify_source_inventory_stable(&before, &after).unwrap();
}

#[test]
fn modified_truncated_added_and_removed_files_are_unstable() {
    use std::time::Duration;
    use std::time::SystemTime;

    fn set_mtime(path: &std::path::Path, secs: u64) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    let temp = tempfile::tempdir().unwrap();
    write_source(temp.path(), "game.exe", b"content-1234");
    write_source(temp.path(), "keep.txt", b"keep");
    set_mtime(&temp.path().join("game.exe"), 1_700_000_000);
    set_mtime(&temp.path().join("keep.txt"), 1_700_000_000);
    let before = scan_dir(temp.path());

    // Same-size modification with a visibly newer timestamp.
    write_source(temp.path(), "game.exe", b"content-5678");
    set_mtime(&temp.path().join("game.exe"), 1_700_000_100);
    let after = scan_dir(temp.path());
    assert!(verify_source_inventory_stable(&before, &after).is_err());

    // Truncation.
    write_source(temp.path(), "game.exe", b"content-1234");
    std::fs::File::options()
        .write(true)
        .open(temp.path().join("game.exe"))
        .unwrap()
        .set_len(0)
        .unwrap();
    let after = scan_dir(temp.path());
    assert!(verify_source_inventory_stable(&before, &after).is_err());

    // Addition.
    write_source(temp.path(), "game.exe", b"content-1234");
    write_source(temp.path(), "extra.dll", b"extra");
    let after = scan_dir(temp.path());
    assert!(verify_source_inventory_stable(&before, &after).is_err());

    // Removal.
    std::fs::remove_file(temp.path().join("extra.dll")).unwrap();
    std::fs::remove_file(temp.path().join("keep.txt")).unwrap();
    let after = scan_dir(temp.path());
    assert!(verify_source_inventory_stable(&before, &after).is_err());
}

#[test]
fn replaced_files_and_permission_flips_are_unstable() {
    let temp = tempfile::tempdir().unwrap();
    let path = write_source(temp.path(), "game.exe", b"content");
    let before = scan_dir(temp.path());

    // Permission flip without touching content or timestamps.
    let original_permissions = std::fs::metadata(&path).unwrap().permissions();
    let mut permissions = original_permissions.clone();
    permissions.set_readonly(!permissions.readonly());
    std::fs::set_permissions(&path, permissions).unwrap();
    let after = scan_dir(temp.path());
    assert!(verify_source_inventory_stable(&before, &after).is_err());

    // Replace with different content under the same name.
    std::fs::set_permissions(&path, original_permissions).unwrap();
    std::fs::remove_file(&path).unwrap();
    write_source(temp.path(), "game.exe", b"replacement-bytes!!");
    let after = scan_dir(temp.path());
    assert!(verify_source_inventory_stable(&before, &after).is_err());
}
