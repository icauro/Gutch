use super::*;
use std::env::temp_dir;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::process;

#[test]
fn compare_identical_files() {
    let dir = temp_dir().join(format!("gutch_path_compare_{}", process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let expected_dir = dir.join("expected");
    let actual_dir = dir.join("actual");
    fs::create_dir_all(&expected_dir).unwrap();
    fs::create_dir_all(&actual_dir).unwrap();

    let mut f = File::create(expected_dir.join("file.txt")).unwrap();
    f.write_all(b"hello world").unwrap();
    let mut f = File::create(actual_dir.join("file.txt")).unwrap();
    f.write_all(b"hello world").unwrap();

    let result = compare_paths(&expected_dir, &actual_dir).unwrap();
    assert!(result.compare_passed);
    assert_eq!(result.matched_file_count, 1);
    assert_eq!(result.mismatched_file_count, 0);
    assert_eq!(result.missing_file_count, 0);
    assert_eq!(result.extra_file_count, 0);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compare_mismatched_files() {
    let dir = temp_dir().join(format!("gutch_path_mismatch_{}", process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let expected_dir = dir.join("expected");
    let actual_dir = dir.join("actual");
    fs::create_dir_all(&expected_dir).unwrap();
    fs::create_dir_all(&actual_dir).unwrap();

    let mut f = File::create(expected_dir.join("file.txt")).unwrap();
    f.write_all(b"hello world").unwrap();
    let mut f = File::create(actual_dir.join("file.txt")).unwrap();
    f.write_all(b"goodbye world").unwrap();

    let result = compare_paths(&expected_dir, &actual_dir).unwrap();
    assert!(!result.compare_passed);
    assert_eq!(result.mismatched_file_count, 1);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compare_missing_files() {
    let dir = temp_dir().join(format!("gutch_path_missing_{}", process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let expected_dir = dir.join("expected");
    let actual_dir = dir.join("actual");
    fs::create_dir_all(&expected_dir).unwrap();
    fs::create_dir_all(&actual_dir).unwrap();

    let mut f = File::create(expected_dir.join("file.txt")).unwrap();
    f.write_all(b"hello").unwrap();

    let result = compare_paths(&expected_dir, &actual_dir).unwrap();
    assert!(!result.compare_passed);
    assert_eq!(result.missing_file_count, 1);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compare_extra_files() {
    let dir = temp_dir().join(format!("gutch_path_extra_{}", process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let expected_dir = dir.join("expected");
    let actual_dir = dir.join("actual");
    fs::create_dir_all(&expected_dir).unwrap();
    fs::create_dir_all(&actual_dir).unwrap();

    let mut f = File::create(expected_dir.join("common.txt")).unwrap();
    f.write_all(b"same").unwrap();
    let mut f = File::create(actual_dir.join("common.txt")).unwrap();
    f.write_all(b"same").unwrap();
    let mut f = File::create(actual_dir.join("extra.txt")).unwrap();
    f.write_all(b"extra").unwrap();

    let result = compare_paths(&expected_dir, &actual_dir).unwrap();
    assert!(!result.compare_passed);
    assert_eq!(result.extra_file_count, 1);
    assert_eq!(result.matched_file_count, 1);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compare_single_files_identical() {
    let dir = temp_dir().join(format!("gutch_single_file_{}", process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let expected_file = dir.join("expected.bin");
    let actual_file = dir.join("actual.bin");
    let mut f = File::create(&expected_file).unwrap();
    f.write_all(b"binary content").unwrap();
    let mut f = File::create(&actual_file).unwrap();
    f.write_all(b"binary content").unwrap();

    let result = compare_paths(&expected_file, &actual_file).unwrap();
    assert!(result.compare_passed);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn collect_comparable_files_nonexistent() {
    let result = collect_comparable_files(Path::new("/nonexistent/path/ever"));
    assert!(result.is_err());
}

#[test]
fn directory_never_matches_a_single_file_with_the_same_entry_name() {
    let root = tempfile::tempdir().unwrap();
    let expected = root.path().join("expected");
    fs::create_dir_all(&expected).unwrap();
    fs::write(expected.join("a"), b"same").unwrap();
    let actual = root.path().join("a");
    fs::write(&actual, b"same").unwrap();

    assert!(!compare_paths(&expected, &actual).unwrap().compare_passed);
}
