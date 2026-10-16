//! Keeps the engine's internal module layering acyclic.
//!
//! Each engine module may import only the layers listed for it. Cross-cutting
//! support modules (`error`, `profile`, `progress`, `runtime`, `test_util`) are
//! available everywhere. Test modules are skipped because fixtures may build
//! state through higher layers.

use std::collections::BTreeSet;
use std::path::Path;

const LAYER_DEPENDENCIES: &[(&str, &[&str])] = &[
    ("filesystem", &[]),
    ("artifact", &["filesystem"]),
    ("manifest", &["artifact", "filesystem"]),
    ("snapshot", &["manifest", "artifact", "filesystem"]),
    ("repo", &["snapshot", "manifest", "artifact", "filesystem"]),
    ("transfer", &["artifact", "filesystem"]),
    (
        "sync",
        &[
            "repo",
            "transfer",
            "snapshot",
            "manifest",
            "artifact",
            "filesystem",
        ],
    ),
    (
        "capture",
        &["repo", "snapshot", "manifest", "artifact", "filesystem"],
    ),
    (
        "installation",
        &[
            "sync",
            "repo",
            "transfer",
            "snapshot",
            "manifest",
            "artifact",
            "filesystem",
        ],
    ),
];

#[test]
fn engine_modules_import_only_lower_layers() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let layered_modules: BTreeSet<&str> =
        LAYER_DEPENDENCIES.iter().map(|(name, _)| *name).collect();
    let mut violations = Vec::new();
    for (module, allowed) in LAYER_DEPENDENCIES {
        for file in rust_files(&source_root.join(module)) {
            for imported in imported_modules(&production_source(&file)) {
                let is_layered = layered_modules.contains(imported.as_str());
                if is_layered && imported != *module && !allowed.contains(&imported.as_str()) {
                    violations.push(format!("{} imports crate::{imported}", file.display()));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "layering violations:\n{}",
        violations.join("\n")
    );
}

fn rust_files(directory: &Path) -> Vec<std::path::PathBuf> {
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

fn imported_modules(source: &str) -> BTreeSet<String> {
    source
        .split("crate::")
        .skip(1)
        .map(|rest| {
            rest.chars()
                .take_while(|character| character.is_ascii_lowercase() || *character == '_')
                .collect()
        })
        .collect()
}
