use super::*;
use std::fs;

#[test]
fn rejects_cross_platform_escape_forms() {
    for path in [
        "../outside",
        "bin/../outside",
        "C:/outside",
        "\\\\server\\share",
        "/outside",
    ] {
        assert!(
            validate_manifest_relative_path(path, false).is_err(),
            "accepted {path}"
        );
    }
}
#[test]
fn disjoint_paths_reject_nesting_in_either_direction() {
    let base = tempfile::tempdir().unwrap();
    let parent = base.path().join("parent");
    let child = parent.join("missing").join("child");
    let sibling = base.path().join("sibling");
    assert!(require_disjoint_paths(&parent, &child).is_err());
    assert!(require_disjoint_paths(&child, &parent).is_err());
    assert!(require_disjoint_paths(&parent, &parent.join(".")).is_err());
    assert!(require_disjoint_paths(&parent, &sibling).is_ok());
    assert!(require_disjoint_paths(&child, &sibling.join("..").join("parent")).is_err());
}
#[test]
fn allows_install_root_only_when_requested() {
    assert!(validate_manifest_relative_path(".", false).is_err());
    assert!(validate_manifest_relative_path(".", true).is_ok());
}
#[test]
fn entry_resolution_accepts_a_missing_leaf() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    let resolver = ManifestPathResolver::new(&root).unwrap();
    assert_eq!(
        resolver.resolve_entry("missing.txt").unwrap(),
        root.join("missing.txt")
    );
}
#[cfg(any(unix, windows))]
#[test]
fn resolver_checks_new_links_beneath_a_cached_parent() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("root");
    let parent = root.join("parent");
    let outside = base.path().join("outside");
    fs::create_dir_all(&parent).unwrap();
    fs::create_dir(&outside).unwrap();
    let resolver = ManifestPathResolver::new(&root).unwrap();
    // Cache the existing parent, but never the missing child.
    assert!(resolver.resolve("parent/child/file").is_ok());
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, parent.join("child")).unwrap();
    #[cfg(windows)]
    if std::os::windows::fs::symlink_dir(&outside, parent.join("child")).is_err() {
        return; // Windows may lack the privilege to create symbolic links.
    }
    assert!(resolver.resolve("parent/child/file").is_err());
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_parent_that_escapes_root() {
    use std::os::unix::fs::symlink;
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("root");
    let outside = base.path().join("outside");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, root.join("escape")).unwrap();
    assert!(resolve_manifest_relative_path(&root, "escape/game").is_err());
}
#[cfg(windows)]
#[test]
fn rejects_directory_link_that_escapes_root_when_links_are_available() {
    use std::os::windows::fs::symlink_dir;
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("root");
    let outside = base.path().join("outside");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();
    if symlink_dir(&outside, root.join("escape")).is_ok() {
        assert!(resolve_manifest_relative_path(&root, "escape/game").is_err());
    }
}

#[test]
fn rejects_dangling_link_that_would_create_outside_root() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("root");
    fs::create_dir_all(&root).unwrap();
    let missing_outside = base.path().join("missing");
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&missing_outside, root.join("leaf")).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&missing_outside, root.join("leaf")).is_ok();
    if linked {
        assert!(resolve_manifest_relative_path(&root, "leaf").is_err());
    }
}

#[test]
fn normalizes_targets_without_allowing_root_escape() {
    assert_eq!(
        normalize_link_target("bin/link", Path::new("../lib/./target")).unwrap(),
        "lib/target"
    );
    assert!(normalize_link_target("link", Path::new("../outside")).is_err());
    assert!(normalize_link_target("bin/link", Path::new("../../outside")).is_err());
    assert!(normalize_link_target("bin/link", &std::env::current_dir().unwrap()).is_err());
}
#[cfg(unix)]
#[test]
fn keeps_unix_backslashes_inside_target_names() {
    assert_eq!(
        normalize_link_target("bin/link", Path::new("a\\b")).unwrap(),
        "bin/a\\b"
    );
}

#[test]
fn path_limits_count_utf8_bytes_and_components() {
    let allowed = "é".repeat(MAX_MANIFEST_PATH_BYTES / 2);
    assert!(validate_manifest_relative_path(&allowed, false).is_ok());
    assert!(validate_manifest_relative_path(&(allowed + "x"), false).is_err());
    let allowed = vec!["a"; MAX_MANIFEST_PATH_COMPONENTS].join("/");
    assert!(validate_manifest_relative_path(&allowed, false).is_ok());
    assert!(validate_manifest_relative_path(&(allowed + "/a"), false).is_err());
    assert!(validate_manifest_relative_path("a/./b", false).is_err());
}
#[test]
fn destination_rejects_long_components_before_creating_output() {
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("output");
    assert!(
        validate_destination_paths(&destination, ["a".repeat(255)].iter().map(String::as_str))
            .is_ok()
    );
    assert!(
        validate_destination_paths(&destination, ["a".repeat(256)].iter().map(String::as_str))
            .is_err()
    );
    assert!(!destination.exists());
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}
#[cfg(windows)]
#[test]
fn windows_aliases_and_reserved_names_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    for name in ["CON", "nul.txt", "COM¹", "a.", "a ", "a:b", "x?y"] {
        assert!(
            validate_destination_paths(temp.path(), [name].into_iter()).is_err(),
            "{name}"
        );
    }
    assert!(validate_destination_paths(temp.path(), ["dir/a", "DIR/b"].into_iter()).is_err());
}
#[test]
fn distinct_parents_do_not_make_equal_children_aliases() {
    // Uppercasing folds sharp-s to SS, but common filesystems keep them distinct.
    let temp = tempfile::tempdir().unwrap();
    validate_destination_paths(temp.path(), ["ß/x", "SS/x", "ß/y"].into_iter()).unwrap();
}
#[cfg(unix)]
#[test]
fn capture_rejects_non_unicode_and_literal_backslash_names() {
    use std::os::unix::ffi::OsStrExt;
    assert!(manifest_path_from_native(Path::new(std::ffi::OsStr::from_bytes(b"bad\xff"))).is_err());
    assert!(manifest_path_from_native(Path::new("literal\\name")).is_err());
    assert_eq!(
        manifest_path_from_native(Path::new("é/e\u{301}")).unwrap(),
        "é/e\u{301}"
    );
}
