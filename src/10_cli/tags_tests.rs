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
fn test_normalize_tags_uses_only_explicit_tags() {
    let mut explicit = SnapshotTagSet::default();
    explicit.insert("release_channel", "nightly").unwrap();
    explicit.insert("loader", "fabric").unwrap();

    let resolved = explicit.normalized().unwrap();

    assert_eq!(resolved, explicit);
}

#[test]
fn test_normalize_tags_allows_tagless_snapshots() {
    let resolved = SnapshotTagSet::default().normalized().unwrap();

    assert!(resolved.is_empty());
}

#[test]
fn direct_capture_and_retag_reject_hyphenated_tag_values() {
    let mut explicit = SnapshotTagSet::default();
    explicit
        .key_value
        .insert("released_at".into(), "2026-09-07".into());
    assert!(explicit.normalized().is_err());
    let error = crate::repo::snapshots::retag_snapshot_by_id(
        std::path::Path::new("unused"),
        "unused",
        explicit,
    )
    .unwrap_err();
    assert!(error.to_string().contains("hyphens"));
}
