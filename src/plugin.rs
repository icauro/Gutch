use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::manifest::{self, PluginInfo, TagConstraint, TargetTagSet};
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    pub product_id: String,
    pub content_repo_id: String,
    pub target_id: String,
    pub target_tags: TargetTagSet,
}

#[derive(Debug, Clone)]
pub struct DependencyGraph {
    pub install_order: Vec<ResolvedTarget>,
}

pub fn resolve_plugin_dependencies(
    base_target: &ResolvedTarget,
    enabled_plugins: &[(&str, &str)],
    plugin_manifests: &HashMap<(String, String), manifest::ContentRepoManifest>,
    target_resolver: &dyn Fn(
        &str,
        &str,
        &BTreeMap<String, TagConstraint>,
    ) -> Result<Option<ResolvedTarget>>,
) -> Result<DependencyGraph> {
    let mut install_order = vec![base_target.clone()];
    let mut in_order: HashSet<String> = HashSet::new();
    in_order.insert(format!(
        "{}:{}",
        base_target.product_id, base_target.content_repo_id
    ));

    let mut queue: VecDeque<(String, String)> = VecDeque::new();
    for (product_id, repo_id) in enabled_plugins {
        queue.push_back((product_id.to_string(), repo_id.to_string()));
    }

    let mut visited: HashSet<String> = HashSet::new();
    while let Some((product_id, repo_id)) = queue.pop_front() {
        let key = format!("{}:{}", product_id, repo_id);
        if !visited.insert(key.clone()) {
            continue;
        }

        let Some(plugin_manifest) = plugin_manifests.get(&(product_id.clone(), repo_id.clone()))
        else {
            return Err(Error::manifest(format!(
                "plugin '{}' (repo '{}') not found",
                product_id, repo_id
            )));
        };

        let Some(ref plugin_info) = plugin_manifest.plugin else {
            return Err(Error::manifest(format!(
                "repo '{}' for product '{}' is not a plugin",
                repo_id, product_id
            )));
        };

        for incompatible in &plugin_info.incompatible_with {
            let inc_key = format!(
                "{}:{}",
                incompatible.product_id, incompatible.content_repo_id
            );
            if in_order.contains(&inc_key) {
                return Err(Error::manifest(format!(
                    "plugin '{}' is incompatible with already-installed '{}'",
                    key, inc_key
                )));
            }
        }

        for dep in &plugin_info.dependencies {
            let dep_key = format!("{}:{}", dep.product_id, dep.content_repo_id);
            if !visited.contains(&dep_key) {
                queue.push_back((dep.product_id.clone(), dep.content_repo_id.clone()));
            }
        }

        let plugin_target =
            resolve_plugin_target(&product_id, &repo_id, plugin_info, target_resolver)?;
        in_order.insert(key);
        install_order.push(plugin_target);
    }

    detect_cycles(&install_order, plugin_manifests)?;
    reorder_by_dependencies(&mut install_order, plugin_manifests);
    apply_override_ordering(&mut install_order, plugin_manifests, enabled_plugins)?;

    Ok(DependencyGraph { install_order })
}

fn resolve_plugin_target(
    product_id: &str,
    content_repo_id: &str,
    _plugin_info: &PluginInfo,
    target_resolver: &dyn Fn(
        &str,
        &str,
        &BTreeMap<String, TagConstraint>,
    ) -> Result<Option<ResolvedTarget>>,
) -> Result<ResolvedTarget> {
    let empty_constraints = BTreeMap::new();
    target_resolver(product_id, content_repo_id, &empty_constraints)?.ok_or_else(|| {
        Error::manifest(format!(
            "no suitable target found for plugin '{}' (repo '{}')",
            product_id, content_repo_id
        ))
    })
}

fn detect_cycles(
    install_order: &[ResolvedTarget],
    plugin_manifests: &HashMap<(String, String), manifest::ContentRepoManifest>,
) -> Result<()> {
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    let mut in_degree: HashMap<String, usize> = HashMap::new();

    for target in install_order {
        let key = format!("{}:{}", target.product_id, target.content_repo_id);
        adj.entry(key.clone()).or_default();
        in_degree.entry(key.clone()).or_insert(0);
    }

    for target in install_order {
        let tkey = format!("{}:{}", target.product_id, target.content_repo_id);
        if let Some(manifest) =
            plugin_manifests.get(&(target.product_id.clone(), target.content_repo_id.clone()))
        {
            if let Some(ref info) = manifest.plugin {
                for dep in &info.dependencies {
                    let dkey = format!("{}:{}", dep.product_id, dep.content_repo_id);
                    if in_degree.contains_key(&dkey) {
                        adj.entry(dkey.clone()).or_default().push(tkey.clone());
                        *in_degree.entry(tkey.clone()).or_insert(0) += 1;
                    }
                }
            }
        }
    }

    let mut queue: VecDeque<String> = VecDeque::new();
    let mut sorted_count = 0usize;
    for (key, degree) in in_degree.iter() {
        if *degree == 0 {
            queue.push_back(key.clone());
        }
    }

    while let Some(key) = queue.pop_front() {
        sorted_count += 1;
        if let Some(dependents) = adj.get(&key) {
            for dependent in dependents {
                if let Some(degree) = in_degree.get_mut(dependent) {
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(dependent.clone());
                    }
                }
            }
        }
    }

    if sorted_count != in_degree.len() {
        return Err(Error::manifest(
            "circular dependency detected among plugins",
        ));
    }

    Ok(())
}

fn reorder_by_dependencies(
    install_order: &mut Vec<ResolvedTarget>,
    plugin_manifests: &HashMap<(String, String), manifest::ContentRepoManifest>,
) {
    let mut i = 1;
    while i < install_order.len() {
        let _target_key = format!(
            "{}:{}",
            install_order[i].product_id, install_order[i].content_repo_id
        );
        if let Some(manifest) = plugin_manifests.get(&(
            install_order[i].product_id.clone(),
            install_order[i].content_repo_id.clone(),
        )) {
            if let Some(ref info) = manifest.plugin {
                for dep in &info.dependencies {
                    let dep_key = format!("{}:{}", dep.product_id, dep.content_repo_id);
                    if let Some(dep_pos) = install_order[i + 1..]
                        .iter()
                        .position(|t| format!("{}:{}", t.product_id, t.content_repo_id) == dep_key)
                    {
                        let actual_pos = i + 1 + dep_pos;
                        let target = install_order.remove(actual_pos);
                        install_order.insert(i, target);
                    }
                }
            }
        }
        i += 1;
    }
}

fn apply_override_ordering(
    install_order: &mut Vec<ResolvedTarget>,
    plugin_manifests: &HashMap<(String, String), manifest::ContentRepoManifest>,
    enabled_plugins: &[(&str, &str)],
) -> Result<()> {
    let mut order_map: HashMap<String, usize> = HashMap::new();
    for (i, target) in install_order.iter().enumerate() {
        let key = format!("{}:{}", target.product_id, target.content_repo_id);
        order_map.insert(key, i);
    }

    for (product_id, repo_id) in enabled_plugins {
        let key = format!("{}:{}", product_id, repo_id);
        let Some(&plugin_idx) = order_map.get(&key) else {
            continue;
        };

        let Some(plugin_manifest) =
            plugin_manifests.get(&(product_id.to_string(), repo_id.to_string()))
        else {
            continue;
        };
        let Some(ref plugin_info) = plugin_manifest.plugin else {
            continue;
        };

        for override_target in &plugin_info.overrides {
            let override_key = format!(
                "{}:{}",
                override_target.product_id, override_target.content_repo_id
            );
            if let Some(&override_idx) = order_map.get(&override_key) {
                if plugin_idx <= override_idx {
                    install_order.swap(plugin_idx, override_idx);
                    let k1 = format!(
                        "{}:{}",
                        install_order[plugin_idx].product_id,
                        install_order[plugin_idx].content_repo_id
                    );
                    let k2 = format!(
                        "{}:{}",
                        install_order[override_idx].product_id,
                        install_order[override_idx].content_repo_id
                    );
                    order_map.insert(k1, plugin_idx);
                    order_map.insert(k2, override_idx);
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::PluginDependency;

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
    ) -> ((String, String), manifest::ContentRepoManifest) {
        let mut m = manifest::ContentRepoManifest::new(product_id.to_string(), repo_id.to_string());
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
}
