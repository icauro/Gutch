use super::*;

#[test]
fn snapshot_manifest_new() {
    let mut tags = SnapshotTagSet::default();
    tags.insert("version", "1.0.0").unwrap();
    tags.insert("os", "windows").unwrap();
    tags.insert("arch", "x86_64").unwrap();
    tags.insert("tier", "base").unwrap();
    let m = SnapshotManifest::new("abc123def".into(), tags.clone());
    assert_eq!(m.snapshot_id, "abc123def");
    assert_eq!(m.snapshot_tags.get("os"), Some("windows"));
    assert!(!m.captured_at.is_empty());
}

#[test]
fn manifest_serialization_roundtrip() {
    let mut tags = SnapshotTagSet::default();
    tags.insert("version", "1.0.0").unwrap();
    tags.insert("os", "windows").unwrap();
    tags.insert("arch", "x86_64").unwrap();
    tags.insert("tier", "base").unwrap();
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
    assert!(json.contains("\"tags\": {"));
    assert!(json.contains("\"arch\": \"x86_64\""));
    assert!(!json.contains("\"executable_path\": \"Game.exe\""));
    assert!(!json.contains("\"exclusive\""));
    assert!(!json.contains("\"files\""));
    let m2: SnapshotMetadata = serde_json::from_str(&json).unwrap();
    assert!(m2.snapshot_id.is_empty());
    assert_eq!(m.captured_at, m2.captured_at);
    assert_eq!(m.snapshot_tags, m2.tags);
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
        format!("os={}", host_tags.get("os").unwrap()),
        format!("arch={}", host_tags.get("arch").unwrap()),
    ];
    let metadata = SnapshotProductMetadata {
        document_type: Default::default(),
        document_version: Default::default(),
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

#[test]
fn host_variant_rejects_requirements_that_could_never_match() {
    for requirement in ["OS=windows", "os = windows", "os"] {
        let metadata = SnapshotProductMetadata {
            document_type: Default::default(),
            document_version: Default::default(),
            runtime: SnapshotRuntimeMetadata::HostVariants(SnapshotHostVariantRuntimeMetadata {
                host_variants: vec![SnapshotHostVariant {
                    variant_id: "windows".into(),
                    host_tag_requirements: vec![requirement.into()],
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
        assert!(metadata.validate_structure().is_err(), "{requirement}");
    }
}

#[test]
fn host_variants_that_can_match_one_host_are_rejected() {
    let variant = |id: &str, requirements: &[&str]| SnapshotHostVariant {
        variant_id: id.into(),
        host_tag_requirements: requirements.iter().map(|r| r.to_string()).collect(),
        launch_actions: vec![SnapshotLaunchAction {
            action_name: "Play".into(),
            executable_path: "Game.exe".into(),
            command_arguments: Vec::new(),
            working_directory_path: None,
        }],
        external_dependencies: Vec::new(),
    };
    let metadata = |variants| SnapshotProductMetadata {
        document_type: Default::default(),
        document_version: Default::default(),
        runtime: SnapshotRuntimeMetadata::HostVariants(SnapshotHostVariantRuntimeMetadata {
            host_variants: variants,
        }),
    };
    let overlapping = metadata(vec![
        variant("linux", &["os=linux"]),
        variant("x64", &["arch=x86_64"]),
    ]);
    assert!(overlapping.validate_structure().is_err());
    let subset = metadata(vec![
        variant("linux", &["os=linux"]),
        variant("linux-x64", &["os=linux", "arch=x86_64"]),
    ]);
    assert!(subset.validate_structure().is_err());
    let exclusive = metadata(vec![
        variant("linux", &["os=linux"]),
        variant("windows", &["os=windows", "arch=x86_64"]),
    ]);
    exclusive.validate_structure().unwrap();
}

#[test]
fn snapshot_metadata_rejects_an_unsupported_version() {
    let mut value = serde_json::to_value(
        SnapshotManifest::new("id".into(), SnapshotTagSet::default()).metadata(),
    )
    .unwrap();
    value["document_version"] = "9.0.0".into();
    assert!(serde_json::from_value::<SnapshotMetadata>(value).is_err());
}
