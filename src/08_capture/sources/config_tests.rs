use super::*;
use crate::filesystem;

fn write_config(discover_from: &Path, json: &str) {
    filesystem::write(&discover_from.join(SOURCE_CONFIG_FILE_NAME), json).unwrap();
}

#[test]
fn default_skeleton_is_auto_mode() {
    let config = SourceConfig::default_skeleton();
    assert_eq!(config.ingestion_mode, IngestionMode::Auto);
    assert!(config.auto_ingestion_rules.ignore_folders.is_empty());
    assert!(config.explicit_ingestion_rules.snapshots.is_empty());
}

#[test]
fn load_missing_returns_none() {
    let temp = tempfile::tempdir().unwrap();
    assert!(SourceConfig::load(temp.path()).unwrap().is_none());
}

#[test]
fn load_or_create_writes_skeleton() {
    let temp = tempfile::tempdir().unwrap();
    let config = SourceConfig::load_or_create(temp.path()).unwrap();
    assert_eq!(config.ingestion_mode, IngestionMode::Auto);
    assert!(temp.path().join(SOURCE_CONFIG_FILE_NAME).is_file());
    let reloaded = SourceConfig::load(temp.path()).unwrap().unwrap();
    assert_eq!(reloaded.ingestion_mode, IngestionMode::Auto);
}

#[test]
fn normalize_relative_path_rules() {
    assert_eq!(normalize_relative_path("./a/b").unwrap(), "a/b");
    assert_eq!(normalize_relative_path("a\\b").unwrap(), "a/b");
    assert_eq!(normalize_relative_path("a//b/./c").unwrap(), "a/b/c");
    assert!(normalize_relative_path("../escape").is_err());
    assert!(normalize_relative_path("a/../b").is_err());
    assert!(normalize_relative_path("/abs").is_err());
    assert!(normalize_relative_path("C:\\abs").is_err());
    assert!(normalize_relative_path("").is_err());
    assert!(normalize_relative_path(".").is_err());
}

#[test]
fn fnmatch_supports_star_and_question() {
    assert!(fnmatch_name(".archives", ".archives"));
    assert!(fnmatch_name(".archives", ".*"));
    assert!(fnmatch_name("foo.part", "*.part"));
    assert!(fnmatch_name("a.b", "a?b"));
    assert!(!fnmatch_name("ab", "a?b"));
    assert!(!fnmatch_name("foo", "*.part"));
    assert!(fnmatch_name("anything", "*"));
}

#[test]
fn auto_discovery_filters_ignore_paths_and_skips_files() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("game-win")).unwrap();
    std::fs::create_dir(temp.path().join(".archives")).unwrap();
    std::fs::write(temp.path().join("notes.txt"), b"x").unwrap();
    write_config(
        temp.path(),
        r#"{"ingestion_mode":"auto","auto_ingestion_rules":{"ignore_folders":[".archives"]}}"#,
    );
    let config = SourceConfig::load(temp.path()).unwrap().unwrap();

    let snapshots = config.discover_snapshot_sources(temp.path()).unwrap();

    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].path, temp.path().join("game-win"));
    assert!(snapshots[0].config_tags.is_none());
}

#[test]
fn explicit_discovery_resolves_nested_paths_with_tags() {
    let temp = tempfile::tempdir().unwrap();
    let nested = temp
        .path()
        .join("versions")
        .join("4.6-stable")
        .join("linux-x86_64");
    std::fs::create_dir_all(&nested).unwrap();
    write_config(
        temp.path(),
        r#"{"ingestion_mode":"explicit","explicit_ingestion_rules":{"snapshots":[{"path":"./versions/4.6-stable/linux-x86_64","tags":["version=4.6_stable","os=linux","arch=x86_64"]}]}}"#,
    );
    let config = SourceConfig::load(temp.path()).unwrap().unwrap();

    let snapshots = config.discover_snapshot_sources(temp.path()).unwrap();

    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].path, nested);
    let tags = snapshots[0].config_tags.as_ref().unwrap();
    assert_eq!(tags.get("version").unwrap(), "4.6_stable");
    assert_eq!(tags.get("os").unwrap(), "linux");
}

#[test]
fn explicit_discovery_rejects_missing_snapshots_and_duplicates() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("game-win")).unwrap();
    write_config(
        temp.path(),
        r#"{"ingestion_mode":"explicit","explicit_ingestion_rules":{"snapshots":[{"path":"game-win"},{"path":"./game-win"}]}}"#,
    );
    let config = SourceConfig::load(temp.path()).unwrap().unwrap();
    assert!(
        config
            .discover_snapshot_sources(temp.path())
            .unwrap_err()
            .to_string()
            .contains("duplicate")
    );

    write_config(
        temp.path(),
        r#"{"ingestion_mode":"explicit","explicit_ingestion_rules":{"snapshots":[{"path":"missing-dir"}]}}"#,
    );
    let config = SourceConfig::load(temp.path()).unwrap().unwrap();
    assert!(
        config
            .discover_snapshot_sources(temp.path())
            .unwrap_err()
            .to_string()
            .contains("does not exist")
    );

    write_config(
        temp.path(),
        r#"{"ingestion_mode":"explicit","explicit_ingestion_rules":{"snapshots":[]}}"#,
    );
    let config = SourceConfig::load(temp.path()).unwrap().unwrap();
    assert!(
        config
            .discover_snapshot_sources(temp.path())
            .unwrap_err()
            .to_string()
            .contains("requires at least one")
    );
}

#[test]
fn auto_discovery_rejects_separators_in_ignore_paths() {
    let temp = tempfile::tempdir().unwrap();
    write_config(
        temp.path(),
        r#"{"ingestion_mode":"auto","auto_ingestion_rules":{"ignore_folders":["deep/nested"]}}"#,
    );
    let config = SourceConfig::load(temp.path()).unwrap().unwrap();
    assert!(
        config
            .discover_snapshot_sources(temp.path())
            .unwrap_err()
            .to_string()
            .contains("plain folder names")
    );
}

#[test]
fn dormant_mode_object_is_not_validated() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("game-win")).unwrap();
    // Auto mode active; the explicit block references a missing dir and
    // empty snapshots, which must be ignored because it is dormant.
    write_config(
        temp.path(),
        r#"{"ingestion_mode":"auto","auto_ingestion_rules":{"ignore_folders":[]},"explicit_ingestion_rules":{"snapshots":[]}}"#,
    );
    let config = SourceConfig::load(temp.path()).unwrap().unwrap();
    let snapshots = config.discover_snapshot_sources(temp.path()).unwrap();
    assert_eq!(snapshots.len(), 1);
}
