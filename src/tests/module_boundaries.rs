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

#[test]
fn engine_modules_use_only_lower_layers() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let lib = fs::read_to_string(source_root.join("lib.rs")).unwrap();
    let modules = declared_modules(&lib);
    let root_items = root_items(&lib);
    let layers: BTreeMap<&str, usize> = modules
        .iter()
        .map(|(name, module)| (name.as_str(), module.layer))
        .collect();

    let mut violations = Vec::new();
    for (module, declared) in &modules {
        let layer = declared.layer;
        let used = used_modules(
            &source_root.join(&declared.path),
            module,
            &layers,
            &root_items,
        );
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
fn declared_modules(source: &str) -> BTreeMap<String, DeclaredModule> {
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
        if let Some(name) = without_visibility(line)
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

/// The item after any `pub`, `pub(crate)` or `pub(in path)` visibility.
fn without_visibility(line: &str) -> &str {
    let Some(rest) = line.strip_prefix("pub") else {
        return line;
    };
    let rest = match rest.strip_prefix('(') {
        Some(scope) => scope.split_once(')').map_or("", |(_, after)| after),
        None => rest,
    };
    rest.trim_start()
}

/// Crate-root `use module::...::Item;` re-exports in `lib.rs`, mapped to the owning module.
fn root_items(lib: &str) -> BTreeMap<String, String> {
    lib.lines()
        .filter_map(|line| without_visibility(line.trim()).strip_prefix("use "))
        .filter_map(|path| path.strip_suffix(';'))
        .filter_map(|path| {
            let path = path.trim_start_matches("crate::");
            let (owner, _) = path.split_once("::")?;
            let item = path.rsplit("::").next()?;
            Some((item.to_string(), owner.to_string()))
        })
        .collect()
}

fn used_modules(
    location: &Path,
    module: &str,
    layers: &BTreeMap<&str, usize>,
    root_items: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    let files = if location.is_dir() {
        rust_files(location)
    } else {
        vec![location.to_path_buf()]
    };
    files
        .iter()
        .flat_map(|file| crate_paths(&production_source(file)))
        .map(|name| root_items.get(&name).cloned().unwrap_or(name))
        .filter(|name| name != module && layers.contains_key(name.as_str()))
        .collect()
}

fn rust_files(directory: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(directory) {
        let path = entry.unwrap().into_path();
        let is_test_file = path.components().any(|part| part.as_os_str() == "tests")
            || path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == "tests.rs" || name.ends_with("_tests.rs"));
        if path.extension().is_some_and(|extension| extension == "rs") && !is_test_file {
            files.push(path);
        }
    }
    files
}

/// Source text without its `#[cfg(test)]` modules.
fn production_source(file: &Path) -> String {
    let source = fs::read_to_string(file).unwrap().replace("\r\n", "\n");
    without_test_modules(&source)
}

/// Removes each `#[cfg(test)] mod` declaration or brace-matched body and keeps
/// the production code around it. Test bodies are assumed to balance braces.
fn without_test_modules(source: &str) -> String {
    const MARKER: &str = "#[cfg(test)]\nmod ";
    let mut production = String::new();
    let mut rest = source;
    while let Some(start) = rest.find(MARKER) {
        production.push_str(&rest[..start]);
        let module = &rest[start..];
        let body_start = module.find(['{', ';']).unwrap_or(module.len());
        let end = if module[body_start..].starts_with('{') {
            let mut depth = 0usize;
            module[body_start..]
                .char_indices()
                .find_map(|(offset, character)| {
                    match character {
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        _ => {}
                    }
                    (depth == 0).then_some(body_start + offset + 1)
                })
                .unwrap_or(module.len())
        } else {
            (body_start + 1).min(module.len())
        };
        rest = &module[end..];
    }
    production.push_str(rest);
    production
}

#[test]
fn production_source_keeps_code_after_inline_test_modules() {
    let source = "use crate::a;\n#[cfg(test)]\nmod tests {\n    fn f() { let _ = crate::t; }\n}\nuse crate::b;\n#[cfg(test)]\nmod more;\nuse crate::c;\n";
    let production = without_test_modules(source);
    assert_eq!(
        crate_paths(&production),
        ["a", "b", "c"].map(String::from).into()
    );
}

/// First path segment after each `crate::`, including each top-level member
/// of a `crate::{...}` group, however deeply the members nest.
fn crate_paths(source: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for rest in source.split("crate::").skip(1) {
        match rest.strip_prefix('{') {
            Some(group) => names.extend(top_level_members(group).map(identifier)),
            None => {
                names.insert(identifier(rest));
            }
        }
    }
    names.remove("");
    names
}

/// Splits a group body (after its `{`) at top-level commas, up to its closing `}`.
fn top_level_members(group: &str) -> impl Iterator<Item = &str> {
    let mut depth = 0usize;
    let mut start = 0;
    let mut members = Vec::new();
    for (offset, character) in group.char_indices() {
        match character {
            '{' => depth += 1,
            '}' if depth == 0 => {
                members.push(&group[start..offset]);
                return members.into_iter();
            }
            '}' => depth -= 1,
            ',' if depth == 0 => {
                members.push(&group[start..offset]);
                start = offset + 1;
            }
            _ => {}
        }
    }
    members.push(&group[start..]);
    members.into_iter()
}

fn identifier(text: &str) -> String {
    text.trim_start()
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
        .collect()
}

#[test]
fn crate_paths_reads_every_member_of_nested_groups() {
    let source = "use crate::{a::{b, c::{d}}, cli, e::f};
use crate::g;";
    assert_eq!(
        crate_paths(source),
        ["a", "cli", "e", "g"].map(String::from).into()
    );
}

#[test]
fn declared_modules_accepts_any_visibility_and_root_items_follow_lib_rs() {
    let lib = "#[path = \"01_a/mod.rs\"]
pub(crate) mod a;
#[path = \"02_b.rs\"]
pub mod b;
#[cfg(test)]
mod tests;
pub(crate) use a::Error;
pub use b::nested::Thing;
";
    let modules = declared_modules(lib);
    assert_eq!(modules.keys().collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(modules["a"].layer, 1);
    assert_eq!(
        root_items(lib),
        BTreeMap::from([
            ("Error".to_string(), "a".to_string()),
            ("Thing".to_string(), "b".to_string()),
        ])
    );
}
