use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::manifest::{PluginInfo, TagConstraint, TargetTagSet};
use crate::{Error, Result};

type PluginManifestMap = HashMap<(String, String), crate::manifest::gutch_repo::GutchRepoManifest>;
type TargetResolver =
    dyn Fn(&str, &str, &BTreeMap<String, TagConstraint>) -> Result<Option<ResolvedTarget>>;

#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    pub product_id: String,
    pub gutch_repo_id: String,
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
    plugin_manifests: &PluginManifestMap,
    target_resolver: &TargetResolver,
) -> Result<DependencyGraph> {
    let mut install_order = vec![base_target.clone()];
    let mut in_order: HashSet<String> = HashSet::new();
    in_order.insert(format!(
        "{}:{}",
        base_target.product_id, base_target.gutch_repo_id
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
            let inc_key = format!("{}:{}", incompatible.product_id, incompatible.gutch_repo_id);
            if in_order.contains(&inc_key) {
                return Err(Error::manifest(format!(
                    "plugin '{}' is incompatible with already-installed '{}'",
                    key, inc_key
                )));
            }
        }

        for dep in &plugin_info.dependencies {
            let dep_key = format!("{}:{}", dep.product_id, dep.gutch_repo_id);
            if !visited.contains(&dep_key) {
                queue.push_back((dep.product_id.clone(), dep.gutch_repo_id.clone()));
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
    gutch_repo_id: &str,
    _plugin_info: &PluginInfo,
    target_resolver: &TargetResolver,
) -> Result<ResolvedTarget> {
    let empty_constraints = BTreeMap::new();
    target_resolver(product_id, gutch_repo_id, &empty_constraints)?.ok_or_else(|| {
        Error::manifest(format!(
            "no suitable target found for plugin '{}' (repo '{}')",
            product_id, gutch_repo_id
        ))
    })
}

fn detect_cycles(
    install_order: &[ResolvedTarget],
    plugin_manifests: &PluginManifestMap,
) -> Result<()> {
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    let mut in_degree: HashMap<String, usize> = HashMap::new();

    for target in install_order {
        let key = format!("{}:{}", target.product_id, target.gutch_repo_id);
        adj.entry(key.clone()).or_default();
        in_degree.entry(key.clone()).or_insert(0);
    }

    for target in install_order {
        let tkey = format!("{}:{}", target.product_id, target.gutch_repo_id);
        if let Some(manifest) =
            plugin_manifests.get(&(target.product_id.clone(), target.gutch_repo_id.clone()))
            && let Some(ref info) = manifest.plugin
        {
            for dep in &info.dependencies {
                let dkey = format!("{}:{}", dep.product_id, dep.gutch_repo_id);
                if in_degree.contains_key(&dkey) {
                    adj.entry(dkey.clone()).or_default().push(tkey.clone());
                    *in_degree.entry(tkey.clone()).or_insert(0) += 1;
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
    plugin_manifests: &PluginManifestMap,
) {
    let mut i = 1;
    while i < install_order.len() {
        let _target_key = format!(
            "{}:{}",
            install_order[i].product_id, install_order[i].gutch_repo_id
        );
        if let Some(manifest) = plugin_manifests.get(&(
            install_order[i].product_id.clone(),
            install_order[i].gutch_repo_id.clone(),
        )) && let Some(ref info) = manifest.plugin
        {
            for dep in &info.dependencies {
                let dep_key = format!("{}:{}", dep.product_id, dep.gutch_repo_id);
                if let Some(dep_pos) = install_order[i + 1..]
                    .iter()
                    .position(|t| format!("{}:{}", t.product_id, t.gutch_repo_id) == dep_key)
                {
                    let actual_pos = i + 1 + dep_pos;
                    let target = install_order.remove(actual_pos);
                    install_order.insert(i, target);
                }
            }
        }
        i += 1;
    }
}

fn apply_override_ordering(
    install_order: &mut Vec<ResolvedTarget>,
    plugin_manifests: &PluginManifestMap,
    enabled_plugins: &[(&str, &str)],
) -> Result<()> {
    let mut order_map: HashMap<String, usize> = HashMap::new();
    for (i, target) in install_order.iter().enumerate() {
        let key = format!("{}:{}", target.product_id, target.gutch_repo_id);
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
                override_target.product_id, override_target.gutch_repo_id
            );
            if let Some(&override_idx) = order_map.get(&override_key)
                && plugin_idx <= override_idx
            {
                install_order.swap(plugin_idx, override_idx);
                let k1 = format!(
                    "{}:{}",
                    install_order[plugin_idx].product_id, install_order[plugin_idx].gutch_repo_id
                );
                let k2 = format!(
                    "{}:{}",
                    install_order[override_idx].product_id,
                    install_order[override_idx].gutch_repo_id
                );
                order_map.insert(k1, plugin_idx);
                order_map.insert(k2, override_idx);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "../plugin_tests.rs"]
mod tests;
