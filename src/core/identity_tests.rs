use super::*;

#[test]
fn test_parse_cli_tags_supports_scoped_and_exclusive_kinds() {
    let parsed = parse_cli_tags(&["loader:fabric".to_string(), "os::windows".to_string()]).unwrap();

    assert!(
        parsed
            .scoped
            .get("loader")
            .is_some_and(|values| values.contains("fabric"))
    );
    assert_eq!(parsed.get_exclusive("os"), Some("windows"));
}

#[test]
fn test_parse_cli_tags_rejects_bare_values() {
    let error = parse_cli_tags(&["alpha".to_string()]).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("scoped:value or exclusive::value")
    );
}

#[test]
fn test_resolve_identity_uses_only_explicit_tags() {
    let mut explicit = SnapshotTagSet::default();
    explicit.upsert_exclusive("release_channel", "nightly");
    explicit.insert_scoped("loader", "fabric");

    let resolved = resolve_snapshot_identity(
        std::path::Path::new("game-v1.0.0-win-x64"),
        &explicit,
        "test",
    )
    .unwrap();

    assert_eq!(resolved.snapshot_tags, explicit);
}

#[test]
fn test_resolve_identity_allows_tagless_snapshots() {
    let resolved = resolve_snapshot_identity(
        std::path::Path::new("some-random-folder"),
        &SnapshotTagSet::default(),
        "test",
    )
    .unwrap();

    assert!(resolved.snapshot_tags.is_empty());
}
