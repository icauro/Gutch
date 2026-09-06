use super::*;
use std::io::Write;

#[test]
fn compare_identical_files() {
    let dir = std::env::temp_dir().join(format!("gutch_path_compare_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let expected_dir = dir.join("expected");
    let actual_dir = dir.join("actual");
    std::fs::create_dir_all(&expected_dir).unwrap();
    std::fs::create_dir_all(&actual_dir).unwrap();

    let mut f = std::fs::File::create(expected_dir.join("file.txt")).unwrap();
    f.write_all(b"hello world").unwrap();
    let mut f = std::fs::File::create(actual_dir.join("file.txt")).unwrap();
    f.write_all(b"hello world").unwrap();

    let result = compare_paths(&expected_dir, &actual_dir).unwrap();
    assert!(result.compare_passed);
    assert_eq!(result.matched_file_count, 1);
    assert_eq!(result.mismatched_file_count, 0);
    assert_eq!(result.missing_file_count, 0);
    assert_eq!(result.extra_file_count, 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn compare_mismatched_files() {
    let dir = std::env::temp_dir().join(format!("gutch_path_mismatch_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let expected_dir = dir.join("expected");
    let actual_dir = dir.join("actual");
    std::fs::create_dir_all(&expected_dir).unwrap();
    std::fs::create_dir_all(&actual_dir).unwrap();

    let mut f = std::fs::File::create(expected_dir.join("file.txt")).unwrap();
    f.write_all(b"hello world").unwrap();
    let mut f = std::fs::File::create(actual_dir.join("file.txt")).unwrap();
    f.write_all(b"goodbye world").unwrap();

    let result = compare_paths(&expected_dir, &actual_dir).unwrap();
    assert!(!result.compare_passed);
    assert_eq!(result.mismatched_file_count, 1);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn compare_missing_files() {
    let dir = std::env::temp_dir().join(format!("gutch_path_missing_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let expected_dir = dir.join("expected");
    let actual_dir = dir.join("actual");
    std::fs::create_dir_all(&expected_dir).unwrap();
    std::fs::create_dir_all(&actual_dir).unwrap();

    let mut f = std::fs::File::create(expected_dir.join("file.txt")).unwrap();
    f.write_all(b"hello").unwrap();

    let result = compare_paths(&expected_dir, &actual_dir).unwrap();
    assert!(!result.compare_passed);
    assert_eq!(result.missing_file_count, 1);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn compare_extra_files() {
    let dir = std::env::temp_dir().join(format!("gutch_path_extra_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let expected_dir = dir.join("expected");
    let actual_dir = dir.join("actual");
    std::fs::create_dir_all(&expected_dir).unwrap();
    std::fs::create_dir_all(&actual_dir).unwrap();

    let mut f = std::fs::File::create(expected_dir.join("common.txt")).unwrap();
    f.write_all(b"same").unwrap();
    let mut f = std::fs::File::create(actual_dir.join("common.txt")).unwrap();
    f.write_all(b"same").unwrap();
    let mut f = std::fs::File::create(actual_dir.join("extra.txt")).unwrap();
    f.write_all(b"extra").unwrap();

    let result = compare_paths(&expected_dir, &actual_dir).unwrap();
    assert!(!result.compare_passed);
    assert_eq!(result.extra_file_count, 1);
    assert_eq!(result.matched_file_count, 1);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn compare_single_files_identical() {
    let dir = std::env::temp_dir().join(format!("gutch_single_file_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let expected_file = dir.join("expected.bin");
    let actual_file = dir.join("actual.bin");
    let mut f = std::fs::File::create(&expected_file).unwrap();
    f.write_all(b"binary content").unwrap();
    let mut f = std::fs::File::create(&actual_file).unwrap();
    f.write_all(b"binary content").unwrap();

    let result = compare_paths(&expected_file, &actual_file).unwrap();
    assert!(result.compare_passed);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn collect_comparable_files_nonexistent() {
    let result = collect_comparable_files(Path::new("/nonexistent/path/ever"));
    assert!(result.is_err());
}
