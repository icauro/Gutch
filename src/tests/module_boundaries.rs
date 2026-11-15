//! Keeps the engine's top-level modules in acyclic dependency layers.
//!
//! Each module lives at `src/NN_name` and `lib.rs` loads it with a `#[path]`
//! attribute; `NN` is its layer. A module's layer is one more than the highest
//! layer its production code uses, so modules sharing a layer never use each
//! other. Test modules are skipped because fixtures may build state through
//! higher layers. `api` is the public facade over every other layer.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Crate-root re-exports declared in `lib.rs`, mapped to their owning module.
const ROOT_ITEMS: &[(&str, &str)] = &[("Error", "error"), ("Result", "error")];

#[test]
fn engine_modules_use_only_lower_layers() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let modules = declared_modules(&source_root.join("lib.rs"));
    let layers: BTreeMap<&str, usize> = modules
        .iter()
        .map(|(name, module)| (name.as_str(), module.layer))
        .collect();

    let mut violations = Vec::new();
    for (module, declared) in &modules {
        let layer = declared.layer;
        let used = used_modules(&source_root.join(&declared.path), module, &layers);
        for name in &used {
            if layers[name.as_str()] >= layer {
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
        if expected != layer {
            violations.push(format!(
                "{module} is numbered layer {layer} but its dependencies place it at {expected}: {used:?}"
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "layering violations:
{}",
        violations.join(
            "
"
        )
    );
}

struct DeclaredModule {
    /// Path from `src/` to the module's top directory or single file.
    path: String,
    layer: usize,
}

/// Production `mod name;` declarations with their `#[path = "NN_name..."]`,
/// skipping any gated by `#[cfg(test)]`.
fn declared_modules(lib: &Path) -> BTreeMap<String, DeclaredModule> {
    let source = fs::read_to_string(lib).unwrap();
    let mut test_only = false;
    let mut path = None;
    let mut modules = BTreeMap::new();
    for line in source.lines().map(str::trim) {
        if line.starts_with("#[") {
            test_only |= line == "#[cfg(test)]";
            if let Some(value) = line
                .strip_prefix("#[path = \"")
                .and_then(|rest| rest.strip_suffix("\"]"))
            {
                path = Some(value.to_string());
            }
            continue;
        }
        let declaration = line.trim_prefix("pub ");
        if let Some(name) = declaration
            .strip_prefix("mod ")
            .and_then(|rest| rest.strip_suffix(';'))
            && !test_only
        {
            let path = path
                .take()
                .unwrap_or_else(|| panic!("module {name} needs a #[path = \"NN_{name}...\"]"));
            let top = path.split('/').next().unwrap().to_string();
            let (layer, folder_name) = top
                .split_once('_')
                .unwrap_or_else(|| panic!("{top} lacks an NN_ layer prefix"));
            assert_eq!(
                folder_name.trim_end_matches(".rs"),
                name,
                "{top} must be named after module {name}"
            );
            let layer = layer
                .parse()
                .unwrap_or_else(|_| panic!("{top} lacks a numeric layer prefix"));
            modules.insert(name.to_string(), DeclaredModule { path: top, layer });
        }
        test_only = false;
        path = None;
    }
    modules
}

fn used_modules(location: &Path, module: &str, layers: &BTreeMap<&str, usize>) -> BTreeSet<String> {
    let files = if location.is_dir() {
        rust_files(location)
    } else {
        vec![location.to_path_buf()]
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
    let source = fs::read_to_string(file).unwrap().replace("\r\n", "\n");
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
