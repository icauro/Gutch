use super::*;

    #[test]
    fn test_target_manifest_new() {
        let mut tags = TargetTagSet::default();
        tags.upsert_exclusive("version", "1.0.0");
        tags.upsert_exclusive("os", "windows");
        tags.upsert_exclusive("arch", "x86_64");
        tags.upsert_exclusive("tier", "base");
        let m = TargetManifest::new(
            "base".into(),
            "abc123def".into(),
            tags.clone(),
        );
        assert_eq!(m.content_repo_id, "base");
        assert_eq!(m.target_id, "abc123def");
        assert_eq!(m.target_tags.get_exclusive("os"), Some("windows"));
        assert_eq!(m.manifest_version, "1.0.0");
        assert!(m.created_at.len() > 0);
    }

    #[test]
    fn test_manifest_serialization_roundtrip() {
        let mut tags = TargetTagSet::default();
        tags.upsert_exclusive("version", "1.0.0");
        tags.upsert_exclusive("os", "windows");
        tags.upsert_exclusive("arch", "x86_64");
        tags.upsert_exclusive("tier", "base");
        let m = TargetManifest::new(
            "base".into(),
            "abc123def".into(),
            tags.clone(),
        );
        let json = serde_json::to_string_pretty(&m).unwrap();
        let m2: TargetManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(m.content_repo_id, m2.content_repo_id);
        assert_eq!(m.target_id, m2.target_id);
        assert_eq!(m.target_tags, m2.target_tags);
    }
