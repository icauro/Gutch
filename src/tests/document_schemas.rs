//! The JSON schemas in `schemas/current/` describe the stored documents and
//! are generated from the Rust types. The parsers stay authoritative: they also
//! enforce rules a schema cannot express, such as valid packaging pairs.
//! Set `GUTCH_UPDATE_SCHEMAS=1` to rewrite the files after a deliberate change.

use std::collections::BTreeMap;
use std::path::Path;

use crate::manifest::VersionedDocument;
use crate::manifest::repo_manifest::RepoManifest;
use crate::manifest::snapshot::{SnapshotMetadata, SnapshotProductMetadata};
use crate::repo::paths::LocalRepoConfig;

fn schema<T: schemars::JsonSchema + VersionedDocument>() -> String {
    serde_json::to_string_pretty(&schemars::schema_for!(T)).unwrap()
        + "
"
}

fn current_schemas() -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        ("repo_metadata.schema.json", schema::<RepoManifest>()),
        (
            "snapshot_metadata.schema.json",
            schema::<SnapshotMetadata>(),
        ),
        (
            "snapshot_product_metadata.schema.json",
            schema::<SnapshotProductMetadata>(),
        ),
        ("local_repo_config.schema.json", schema::<LocalRepoConfig>()),
    ])
}

#[test]
fn committed_schemas_match_the_document_types() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas/current");
    let schemas = current_schemas();
    if std::env::var_os("GUTCH_UPDATE_SCHEMAS").is_some_and(|value| value == "1") {
        if directory.exists() {
            std::fs::remove_dir_all(&directory).unwrap();
        }
        std::fs::create_dir_all(&directory).unwrap();
        for (name, schema) in &schemas {
            std::fs::write(directory.join(name), schema).unwrap();
        }
    }
    let committed: BTreeMap<String, String> = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("{}: {error}", directory.display()))
        .map(|entry| {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_str().unwrap().to_owned();
            (
                name,
                std::fs::read_to_string(&path)
                    .unwrap()
                    .replace("\r\n", "\n"),
            )
        })
        .collect();
    let expected: BTreeMap<String, String> = schemas
        .into_iter()
        .map(|(name, schema)| (name.to_owned(), schema))
        .collect();
    assert!(
        committed == expected,
        "schemas/current is out of date; rerun this test with GUTCH_UPDATE_SCHEMAS=1 \
         and review the diff"
    );
}
