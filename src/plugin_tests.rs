use super::*;
use crate::manifest::{self, PluginDependency};

fn mock_target_resolver(
    _product_id: &str,
    _repo_id: &str,
    _constraints: &BTreeMap<String, TagConstraint>,
) -> Result<Option<ResolvedTarget>> {
    Ok(Some(ResolvedTarget {
        product_id: _product_id.to_string(),
        content_repo_id: _repo_id.to_string(),
        target_id: "mock-target-id".to_string(),
        target_tags: TargetTagSet::default(),
    }))
}

fn make_plugin_manifest(
    product_id: &str,
    repo_id: &str,
    plugin_type: manifest::PluginType,
    dependencies: Vec<PluginDependency>,
    overrides: Vec<manifest::PluginOverride>,
    incompatible_with: Vec<manifest::PluginOverride>,
) -> (
    (String, String),
    crate::manifest::content_repo::ContentRepoManifest,
) {
    let mut m = crate::manifest::content_repo::ContentRepoManifest::new(repo_id.to_string());
    m.plugin = Some(manifest::PluginInfo {
        plugin_type,
        display_name: None,
        dependencies,
        overrides,
        incompatible_with,
    });
    ((product_id.to_string(), repo_id.to_string()), m)
}

#[test]
fn test_resolve_single_plugin() {
    let base = ResolvedTarget {
        product_id: "game".to_string(),
        content_repo_id: "base".to_string(),
        target_id: "base-target".to_string(),
        target_tags: TargetTagSet::default(),
    };

    let (key, pm) = make_plugin_manifest(
        "mod-ae2",
        "base",
        manifest::PluginType::Mod,
        vec![],
        vec![],
        vec![],
    );
    let mut manifests = HashMap::new();
    manifests.insert(key, pm);

    let result = resolve_plugin_dependencies(
        &base,
        &[("mod-ae2", "base")],
        &manifests,
        &mock_target_resolver,
    )
    .unwrap();

    assert_eq!(result.install_order.len(), 2);
    assert_eq!(result.install_order[0].product_id, "game");
    assert_eq!(result.install_order[1].product_id, "mod-ae2");
}

#[test]
fn test_resolve_transitive_dependencies() {
    let base = ResolvedTarget {
        product_id: "game".to_string(),
        content_repo_id: "base".to_string(),
        target_id: "base-target".to_string(),
        target_tags: TargetTagSet::default(),
    };

    let mut manifests = HashMap::new();

    let (k1, dep_m) = make_plugin_manifest(
        "fabric-api",
        "base",
        manifest::PluginType::Mod,
        vec![],
        vec![],
        vec![],
    );
    manifests.insert(k1, dep_m);

    let (k2, plug_m) = make_plugin_manifest(
        "mod-ae2",
        "base",
        manifest::PluginType::Mod,
        vec![PluginDependency {
            product_id: "fabric-api".to_string(),
            content_repo_id: "base".to_string(),
            tag_constraints: BTreeMap::new(),
        }],
        vec![],
        vec![],
    );
    manifests.insert(k2, plug_m);

    let result = resolve_plugin_dependencies(
        &base,
        &[("mod-ae2", "base")],
        &manifests,
        &mock_target_resolver,
    )
    .unwrap();

    assert_eq!(result.install_order.len(), 3);
    assert_eq!(result.install_order[0].product_id, "game");
    assert_eq!(result.install_order[1].product_id, "fabric-api");
    assert_eq!(result.install_order[2].product_id, "mod-ae2");
}

#[test]
fn test_empty_plugin_list_returns_base_only() {
    let base = ResolvedTarget {
        product_id: "game".to_string(),
        content_repo_id: "base".to_string(),
        target_id: "base-target".to_string(),
        target_tags: TargetTagSet::default(),
    };
    let manifests = HashMap::new();
    let result =
        resolve_plugin_dependencies(&base, &[], &manifests, &mock_target_resolver).unwrap();
    assert_eq!(result.install_order.len(), 1);
    assert_eq!(result.install_order[0].product_id, "game");
}

#[test]
fn test_circular_dependency_detected() {
    let base = ResolvedTarget {
        product_id: "game".to_string(),
        content_repo_id: "base".to_string(),
        target_id: "base-target".to_string(),
        target_tags: TargetTagSet::default(),
    };
    let mut manifests = HashMap::new();
    let (k1, m1) = make_plugin_manifest(
        "mod-a",
        "base",
        manifest::PluginType::Mod,
        vec![PluginDependency {
            product_id: "mod-b".to_string(),
            content_repo_id: "base".to_string(),
            tag_constraints: BTreeMap::new(),
        }],
        vec![],
        vec![],
    );
    manifests.insert(k1, m1);
    let (k2, m2) = make_plugin_manifest(
        "mod-b",
        "base",
        manifest::PluginType::Mod,
        vec![PluginDependency {
            product_id: "mod-a".to_string(),
            content_repo_id: "base".to_string(),
            tag_constraints: BTreeMap::new(),
        }],
        vec![],
        vec![],
    );
    manifests.insert(k2, m2);
    let result = resolve_plugin_dependencies(
        &base,
        &[("mod-a", "base")],
        &manifests,
        &mock_target_resolver,
    );
    assert!(result.is_err());
}

#[test]
fn test_missing_dependency_rejected() {
    let base = ResolvedTarget {
        product_id: "game".to_string(),
        content_repo_id: "base".to_string(),
        target_id: "base-target".to_string(),
        target_tags: TargetTagSet::default(),
    };
    let mut manifests = HashMap::new();
    let (k1, m1) = make_plugin_manifest(
        "mod-ae2",
        "base",
        manifest::PluginType::Mod,
        vec![PluginDependency {
            product_id: "nonexistent-dep".to_string(),
            content_repo_id: "base".to_string(),
            tag_constraints: BTreeMap::new(),
        }],
        vec![],
        vec![],
    );
    manifests.insert(k1, m1);
    let result = resolve_plugin_dependencies(
        &base,
        &[("mod-ae2", "base")],
        &manifests,
        &mock_target_resolver,
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

#[test]
fn test_override_ensures_ordering() {
    let base = ResolvedTarget {
        product_id: "game".to_string(),
        content_repo_id: "base".to_string(),
        target_id: "base-target".to_string(),
        target_tags: TargetTagSet::default(),
    };
    let mut manifests = HashMap::new();
    let (k1, m1) = make_plugin_manifest(
        "base-enhancer",
        "base",
        manifest::PluginType::Mod,
        vec![],
        vec![],
        vec![],
    );
    manifests.insert(k1, m1);
    let (k2, m2) = make_plugin_manifest(
        "mod-ae2",
        "base",
        manifest::PluginType::Mod,
        vec![],
        vec![manifest::PluginOverride {
            product_id: "base-enhancer".to_string(),
            content_repo_id: "base".to_string(),
        }],
        vec![],
    );
    manifests.insert(k2, m2);
    let result = resolve_plugin_dependencies(
        &base,
        &[("mod-ae2", "base"), ("base-enhancer", "base")],
        &manifests,
        &mock_target_resolver,
    )
    .unwrap();
    assert_eq!(result.install_order.len(), 3);
    assert_eq!(result.install_order[1].product_id, "base-enhancer");
    assert_eq!(result.install_order[2].product_id, "mod-ae2");
}

#[test]
fn test_self_dependency_detected_as_cycle() {
    let base = ResolvedTarget {
        product_id: "game".to_string(),
        content_repo_id: "base".to_string(),
        target_id: "base-target".to_string(),
        target_tags: TargetTagSet::default(),
    };
    let mut manifests = HashMap::new();
    let (k1, m1) = make_plugin_manifest(
        "mod-self",
        "base",
        manifest::PluginType::Mod,
        vec![PluginDependency {
            product_id: "mod-self".to_string(),
            content_repo_id: "base".to_string(),
            tag_constraints: BTreeMap::new(),
        }],
        vec![],
        vec![],
    );
    manifests.insert(k1, m1);
    let result = resolve_plugin_dependencies(
        &base,
        &[("mod-self", "base")],
        &manifests,
        &mock_target_resolver,
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("circular"));
}

#[test]
fn test_incompatible_plugin_rejected() {
    let base = ResolvedTarget {
        product_id: "game".to_string(),
        content_repo_id: "base".to_string(),
        target_id: "base-target".to_string(),
        target_tags: TargetTagSet::default(),
    };

    let mut manifests = HashMap::new();

    let (k1, m1) = make_plugin_manifest(
        "mod-old",
        "base",
        manifest::PluginType::Mod,
        vec![],
        vec![],
        vec![],
    );
    manifests.insert(k1, m1);

    let (k2, m2) = make_plugin_manifest(
        "mod-ae2",
        "base",
        manifest::PluginType::Mod,
        vec![],
        vec![],
        vec![manifest::PluginOverride {
            product_id: "mod-old".to_string(),
            content_repo_id: "base".to_string(),
        }],
    );
    manifests.insert(k2, m2);

    let result = resolve_plugin_dependencies(
        &base,
        &[("mod-old", "base"), ("mod-ae2", "base")],
        &manifests,
        &mock_target_resolver,
    );

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("incompatible"));
}
