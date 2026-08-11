use super::*;

#[test]
fn test_target_manifest_new() {
    let mut tags = TargetTagSet::default();
    tags.upsert_exclusive("version", "1.0.0");
    tags.upsert_exclusive("os", "windows");
    tags.upsert_exclusive("arch", "x86_64");
    tags.upsert_exclusive("tier", "base");
    let m = TargetManifest::new("abc123def".into(), tags.clone());
    assert_eq!(m.target_id, "abc123def");
    assert_eq!(m.target_tags.get_exclusive("os"), Some("windows"));
    assert_eq!(m.manifest_version, "1.0.0");
    assert!(m.inserted_at.len() > 0);
}

#[test]
fn test_manifest_serialization_roundtrip() {
    let mut tags = TargetTagSet::default();
    tags.upsert_exclusive("version", "1.0.0");
    tags.upsert_exclusive("os", "windows");
    tags.upsert_exclusive("arch", "x86_64");
    tags.upsert_exclusive("tier", "base");
    let mut m = TargetManifest::new("abc123def".into(), tags.clone());
    m.launch = Some(launch::ResolvedLaunchConfiguration {
        primary: None,
        actions: vec![launch::ResolvedLaunchAction {
            name: "Play".into(),
            executable: "Game.exe".into(),
            arguments: vec!["--safe".into()],
            working_directory: None,
        }],
    });
    let json = serde_json::to_string_pretty(&m).unwrap();
    assert!(json.contains("\"target_tags\": ["));
    assert!(json.contains("\"arch::x86_64\""));
    assert!(json.contains("\"executable\": \"Game.exe\""));
    assert!(!json.contains("\"exclusive\""));
    let m2: TargetManifest = serde_json::from_str(&json).unwrap();
    assert!(m2.target_id.is_empty());
    assert_eq!(m.inserted_at, m2.inserted_at);
    assert_eq!(m.target_tags, m2.target_tags);
    assert_eq!(m.launch, m2.launch);
}

#[test]
fn target_metadata_requires_source_modified_at() {
    let manifest = TargetManifest::new("abc123def".into(), TargetTagSet::default());
    let mut value = serde_json::to_value(manifest).unwrap();
    value.as_object_mut().unwrap().remove("source_modified_at");

    assert!(serde_json::from_value::<TargetManifest>(value).is_err());
}
