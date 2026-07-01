use super::*;

fn test_rules() -> TargetTagSchema {
    TargetTagSchema::default()
}

#[test]
fn test_parse_cli_tags_supports_all_three_kinds() {
    let parsed = parse_cli_tags(&[
        "alpha".to_string(),
        "loader:fabric".to_string(),
        "os::windows".to_string(),
    ])
    .unwrap();

    assert!(parsed.plain.contains("alpha"));
    assert!(
        parsed
            .scoped
            .get("loader")
            .is_some_and(|values| values.contains("fabric"))
    );
    assert_eq!(parsed.get_exclusive("os"), Some("windows"));
}

#[test]
fn test_version_strips_v_prefix() {
    assert_eq!(normalize_version("v1.0.0").as_deref(), Some("1.0.0"));
    assert_eq!(normalize_version("1.2.3").as_deref(), Some("1.2.3"));
    assert_eq!(
        normalize_version("2025.05.30.1").as_deref(),
        Some("2025.05.30.1")
    );
    assert_eq!(normalize_version("version-1"), None);
}

#[test]
fn test_infer_tags_from_path_default_aliases() {
    let rules = test_rules();
    let tags = infer_tags_from_path(
        std::path::Path::new("game-v1.0.0-win-x64-supporter"),
        &rules,
    );

    assert_eq!(tags.get_exclusive("version"), Some("1.0.0"));
    assert_eq!(tags.get_exclusive("os"), Some("windows"));
    assert_eq!(tags.get_exclusive("arch"), Some("x86_64"));
    // access tag removed from Default template; "supporter" is not inferred
    assert_eq!(tags.get_exclusive("access"), None);
}

#[test]
fn test_infer_tags_from_path_requires_dash_prefix_segments() {
    let rules = test_rules();
    let tags = infer_tags_from_path(std::path::Path::new("game_v1.0.0_win_x64"), &rules);

    assert_eq!(tags.get_exclusive("version"), None);
    assert_eq!(tags.get_exclusive("os"), None);
    assert_eq!(tags.get_exclusive("arch"), None);
}

#[test]
fn test_infer_calver_from_prefixed_segment() {
    let rules = test_rules();
    let tags = infer_tags_from_path(std::path::Path::new("game-v2025.05.30.1-win-x64"), &rules);

    assert_eq!(tags.get_exclusive("version"), Some("2025.05.30.1"));
    assert_eq!(tags.get_exclusive("os"), Some("windows"));
    assert_eq!(tags.get_exclusive("arch"), Some("x86_64"));
}

#[test]
fn test_resolve_identity_explicit_overrides_inference() {
    let rules = test_rules();
    let mut explicit = TargetTagSet::default();
    explicit.upsert_exclusive("version".to_string(), "2.0.0".to_string());
    explicit.upsert_exclusive("os".to_string(), "linux".to_string());

    let resolved = resolve_target_identity(
        std::path::Path::new("game-v1.0.0-win-x64"),
        &explicit,
        &rules,
        "test",
    )
    .unwrap();

    assert_eq!(resolved.target_tags.get_exclusive("version"), Some("2.0.0"));
    assert_eq!(resolved.target_tags.get_exclusive("os"), Some("linux"));
    assert_eq!(resolved.target_tags.get_exclusive("arch"), Some("x86_64"));
}

#[test]
fn test_resolve_missing_version_errors() {
    let rules = test_rules();
    let result = resolve_target_identity(
        std::path::Path::new("game-win-x64"),
        &TargetTagSet::default(),
        &rules,
        "test",
    );

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("missing"));
}

#[test]
fn test_resolve_custom_scoped_tags() {
    let mut rules = test_rules();
    rules
        .target_tags
        .push(gutch::manifest::TargetTagDefinition {
            name: "loader".to_string(),
            kind: TargetTagKind::Scoped,
            required: false,
            inference_prefixes: vec!["-".to_string()],
            values: vec![
                gutch::manifest::TargetTagValueDefinition {
                    value: "fabric".to_string(),
                    aliases: Vec::new(),
                },
                gutch::manifest::TargetTagValueDefinition {
                    value: "forge".to_string(),
                    aliases: Vec::new(),
                },
            ],
            default_value: None,
        });

    let resolved = resolve_target_identity(
        std::path::Path::new("mod-v1.0.0-fabric"),
        &TargetTagSet::default(),
        &rules,
        "test",
    )
    .unwrap();

    assert_eq!(resolved.target_tags.get_exclusive("version"), Some("1.0.0"));
    assert!(
        resolved
            .target_tags
            .scoped
            .get("loader")
            .is_some_and(|values| values.contains("fabric"))
    );
}

#[test]
fn test_resolve_missing_required_tag_errors() {
    let mut rules = test_rules();
    rules.target_tags.iter_mut().for_each(|definition| {
        if definition.name == "os" {
            definition.required = true;
        }
    });

    let result = resolve_target_identity(
        std::path::Path::new("game-v1.0.0"),
        &TargetTagSet::default(),
        &rules,
        "test",
    );

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("os"));
}

#[test]
fn test_explicit_tags_dont_need_inference() {
    let rules = test_rules();
    let mut explicit = TargetTagSet::default();
    explicit.upsert_exclusive("version".to_string(), "1.0.0".to_string());
    explicit.upsert_exclusive("os".to_string(), "windows".to_string());
    explicit.upsert_exclusive("arch".to_string(), "x86_64".to_string());

    let resolved = resolve_target_identity(
        std::path::Path::new("some-random-folder"),
        &explicit,
        &rules,
        "test",
    )
    .unwrap();

    assert_eq!(resolved.target_tags.get_exclusive("version"), Some("1.0.0"));
    assert_eq!(resolved.target_tags.get_exclusive("os"), Some("windows"));
}
