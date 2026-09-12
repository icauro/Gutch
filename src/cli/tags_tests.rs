use super::*;

#[test]
fn cli_tags_use_one_value_per_key() {
    let parsed = parse_cli_tags(&["loader=fabric".to_string(), "os=windows".to_string()]).unwrap();
    assert_eq!(parsed.get("loader"), Some("fabric"));
    assert_eq!(parsed.get("os"), Some("windows"));
}

#[test]
fn test_parse_cli_tags_rejects_bare_values() {
    let error = parse_cli_tags(&["alpha".to_string()]).unwrap_err();
    assert!(error.to_string().contains("key=value"));
}

#[test]
fn test_resolve_tags_uses_only_explicit_tags() {
    let mut explicit = SnapshotTagSet::default();
    explicit.insert("release_channel", "nightly").unwrap();
    explicit.insert("loader", "fabric").unwrap();

    let resolved = resolve_snapshot_tags(
        std::path::Path::new("game-v1.0.0-win-x64"),
        &explicit,
        "test",
    )
    .unwrap();

    assert_eq!(resolved.snapshot_tags, explicit);
}

#[test]
fn test_resolve_tags_allows_tagless_snapshots() {
    let resolved = resolve_snapshot_tags(
        std::path::Path::new("some-random-folder"),
        &SnapshotTagSet::default(),
        "test",
    )
    .unwrap();

    assert!(resolved.snapshot_tags.is_empty());
}

#[test]
fn direct_capture_and_retag_reject_hyphenated_tag_values() {
    let mut explicit = SnapshotTagSet::default();
    explicit
        .key_value
        .insert("released_at".into(), "2026-09-07".into());
    assert!(resolve_snapshot_tags(Path::new("unused"), &explicit, "test").is_err());
    let error =
        crate::api::retag_snapshot_by_id(Path::new("unused"), "unused", explicit).unwrap_err();
    assert!(error.to_string().contains("hyphens"));
}
