//! Keeps the engine's top-level modules in acyclic dependency layers.
//!
//! A module's layer is one more than the highest layer its production code
//! uses, so modules sharing a layer never use each other. Test modules are
//! skipped because fixtures may build state through higher layers. `api` is the
//! public facade over every other layer.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const SOURCE_LAYERS: &[(&str, usize)] = &[
    ("error", 0),
    ("profile", 1),
    ("progress", 2),
    ("filesystem", 3),
    ("runtime", 3),
    ("artifact", 4),
    ("config", 4),
    ("manifest", 5),
    ("path_compare", 5),
    ("transfer", 5),
    ("snapshot", 6),
    ("repo", 7),
    ("capture", 8),
    ("sync", 8),
    ("installation", 9),
    ("cli", 10),
    ("api", 11),
];

/// Crate-root re-exports declared in `lib.rs`, mapped to their owning module.
const ROOT_ITEMS: &[(&str, &str)] = &[("Error", "error"), ("Result", "error")];

#[test]
fn engine_modules_use_only_lower_layers() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let layers: BTreeMap<&str, usize> = SOURCE_LAYERS.iter().copied().collect();
    let declared = declared_modules(&source_root.join("lib.rs"));
    assert_eq!(
        declared,
        layers.keys().map(|name| name.to_string()).collect(),
        "declare every top-level module in SOURCE_LAYERS"
    );

    let mut violations = Vec::new();
    for (module, layer) in &layers {
        let used = used_modules(&source_root, module, &layers);
        for name in &used {
            if layers[name.as_str()] >= *layer {
                violations.push(format!(
                    "{module} (layer {layer}) uses {name} (layer {})",
                    layers[name.as_str()]
                ));
            }
        }
        let expected = used
            .iter()
            .map(|name| layers[name.as_str()] + 1)
            .max()
            .unwrap_or(0);
        if expected != *layer {
            violations.push(format!(
                "{module} is declared layer {layer} but its dependencies place it at {expected}: {used:?}"
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "layering violations:\n{}",
        violations.join("\n")
    );
}

/// Production `mod name;` declarations, skipping any gated by `#[cfg(test)]`.
fn declared_modules(lib: &Path) -> BTreeSet<String> {
    let source = std::fs::read_to_string(lib).unwrap();
    let mut test_only = false;
    let mut modules = BTreeSet::new();
    for line in source.lines().map(str::trim) {
        if line.starts_with("#[") {
            test_only |= line == "#[cfg(test)]";
            continue;
        }
        let declaration = line.strip_prefix("pub ").unwrap_or(line);
        if let Some(name) = declaration
            .strip_prefix("mod ")
            .and_then(|rest| rest.strip_suffix(';'))
            && !test_only
        {
            modules.insert(name.to_string());
        }
        test_only = false;
    }
    modules
}

fn used_modules(
    source_root: &Path,
    module: &str,
    layers: &BTreeMap<&str, usize>,
) -> BTreeSet<String> {
    let directory = source_root.join(module);
    let files = if directory.is_dir() {
        rust_files(&directory)
    } else {
        vec![source_root.join(format!("{module}.rs"))]
    };
    files
        .iter()
        .flat_map(|file| crate_paths(&production_source(file)))
        .map(|name| {
            ROOT_ITEMS
                .iter()
                .find(|(item, _)| *item == name)
                .map_or(name.clone(), |(_, owner)| owner.to_string())
        })
        .filter(|name| name != module && layers.contains_key(name.as_str()))
        .collect()
}

fn rust_files(directory: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(directory) {
        let path = entry.unwrap().into_path();
        let is_test_file = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains("test"));
        if path.extension().is_some_and(|extension| extension == "rs") && !is_test_file {
            files.push(path);
        }
    }
    files
}

/// Source text before the file's first inline test module.
fn production_source(file: &Path) -> String {
    let source = std::fs::read_to_string(file).unwrap().replace("\r\n", "\n");
    match source.find("#[cfg(test)]\nmod ") {
        Some(test_module_start) => source[..test_module_start].to_string(),
        None => source,
    }
}

/// First path segment after each `crate::`, including each `crate::{...}` group member.
fn crate_paths(source: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for rest in source.split("crate::").skip(1) {
        match rest.strip_prefix('{') {
            Some(group) => {
                let group = &group[..group.find('}').unwrap_or(group.len())];
                names.extend(group.split(',').map(identifier));
            }
            None => {
                names.insert(identifier(rest));
            }
        }
    }
    names.remove("");
    names
}

fn identifier(text: &str) -> String {
    text.trim_start()
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
        .collect()
}
