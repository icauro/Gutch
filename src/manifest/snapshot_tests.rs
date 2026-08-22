use super::*;

#[test]
fn test_snapshot_manifest_new() {
    let mut tags = SnapshotTagSet::default();
    tags.upsert_exclusive("version", "1.0.0");
    tags.upsert_exclusive("os", "windows");
    tags.upsert_exclusive("arch", "x86_64");
    tags.upsert_exclusive("tier", "base");
    let m = SnapshotManifest::new("abc123def".into(), tags.clone());
    assert_eq!(m.snapshot_id, "abc123def");
    assert_eq!(m.snapshot_tags.get_exclusive("os"), Some("windows"));
    assert_eq!(m.metadata_version, "1.0.0");
    assert!(m.inserted_at.len() > 0);
}

#[test]
fn test_manifest_serialization_roundtrip() {
    let mut tags = SnapshotTagSet::default();
    tags.upsert_exclusive("version", "1.0.0");
    tags.upsert_exclusive("os", "windows");
    tags.upsert_exclusive("arch", "x86_64");
    tags.upsert_exclusive("tier", "base");
    let mut m = SnapshotManifest::new("abc123def".into(), tags.clone());
    m.launch = Some(launch::ResolvedLaunchConfiguration {
        primary: None,
        actions: vec![launch::ResolvedLaunchAction {
            name: "Play".into(),
            executable: "Game.exe".into(),
            arguments: vec!["--safe".into()],
            working_directory: None,
        }],
    });
    m.sync_product_metadata_from_launch();
    let json = serde_json::to_string_pretty(&m.metadata()).unwrap();
    assert!(json.contains("\"snapshot_tags\": ["));
    assert!(json.contains("\"arch::x86_64\""));
    assert!(!json.contains("\"executable_path\": \"Game.exe\""));
    assert!(!json.contains("\"exclusive\""));
    assert!(!json.contains("\"files\""));
    let m2: SnapshotMetadata = serde_json::from_str(&json).unwrap();
    assert!(m2.snapshot_id.is_empty());
    assert_eq!(m.inserted_at, m2.inserted_at);
    assert_eq!(m.snapshot_tags, m2.snapshot_tags);
}

#[test]
fn snapshot_metadata_does_not_store_source_or_blueprint_details() {
    let manifest = SnapshotManifest::new("abc123def".into(), SnapshotTagSet::default());
    let mut value = serde_json::to_value(manifest.metadata()).unwrap();
    assert!(
        !value
            .as_object_mut()
            .unwrap()
            .contains_key("source_modified_at")
    );
    assert!(
        !value
            .as_object_mut()
            .unwrap()
            .contains_key("total_logical_size")
    );
    assert!(serde_json::from_value::<SnapshotMetadata>(value).is_ok());
}

#[test]
fn host_variant_runtime_uses_launcher_host_tags() {
    let host_tags = current_host_tags();
    let requirements = vec![
        format!("os::{}", host_tags.get_exclusive("os").unwrap()),
        format!("arch::{}", host_tags.get_exclusive("arch").unwrap()),
    ];
    let metadata = SnapshotProductMetadata {
        document_version: "1.0.0".into(),
        runtime: SnapshotRuntimeMetadata::HostVariants(SnapshotHostVariantRuntimeMetadata {
            host_variants: vec![SnapshotHostVariant {
                variant_id: "current-host".into(),
                host_tag_requirements: requirements,
                launch_actions: vec![SnapshotLaunchAction {
                    action_name: "Play".into(),
                    executable_path: "Game.exe".into(),
                    command_arguments: Vec::new(),
                    working_directory_path: None,
                }],
                external_dependencies: Vec::new(),
            }],
        }),
    };

    let resolved = metadata.launch_for_host_tags(&host_tags).unwrap();
    assert_eq!(resolved.actions[0].executable, "Game.exe");
    assert!(metadata.validate_structure().is_ok());
}
