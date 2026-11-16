use super::*;
use crate::manifest::repo_manifest::RepoManifest;

fn repo_metadata() -> serde_json::Value {
    serde_json::to_value(RepoManifest::new("base".into())).unwrap()
}

fn parse(value: &serde_json::Value) -> Result<RepoManifest> {
    RepoManifest::from_json_bytes(REPO_METADATA_FILE, &serde_json::to_vec(value).unwrap())
}

fn unsupported(result: Result<RepoManifest>) -> (String, bool) {
    match result {
        Err(Error::UnsupportedFormat {
            found,
            needs_newer_gutch,
            ..
        }) => (found, needs_newer_gutch),
        other => panic!("expected an unsupported format, got {other:?}"),
    }
}

#[test]
fn markers_serialize_as_their_fixed_strings_and_reject_others() {
    let value = repo_metadata();
    assert_eq!(value["document_type"], "gutch_repo");
    assert_eq!(value["document_version"], "1.0.0");
    assert_eq!(value["repository_format"]["version"], 1);
    assert!(parse(&value).is_ok());
    let mut wrong_type = value.clone();
    wrong_type["document_type"] = "gutch_snapshot".into();
    assert!(matches!(parse(&wrong_type), Err(Error::Manifest(_))));
    // Direct serde use cannot bypass the markers either.
    assert!(serde_json::from_value::<RepoManifest>(wrong_type).is_err());
}

#[test]
fn a_newer_document_version_is_refused_before_its_unknown_fields() {
    let mut newer = repo_metadata();
    newer["document_version"] = "2.0.0".into();
    newer["field_added_later"] = true.into();
    let (found, needs_newer_gutch) = unsupported(parse(&newer));
    assert_eq!(found, "document_version '2.0.0'");
    assert!(needs_newer_gutch);
    let mut older = repo_metadata();
    older["document_version"] = "0.9.0".into();
    assert!(!unsupported(parse(&older)).1);
    let mut missing = repo_metadata();
    missing.as_object_mut().unwrap().remove("document_version");
    assert_eq!(unsupported(parse(&missing)).0, "document_version missing");
}

#[test]
fn version_order_compares_numeric_parts() {
    assert!(is_newer_version("1.10.0", "1.9.0"));
    assert!(is_newer_version("2.0.0", "1.0.0"));
    assert!(!is_newer_version("1.0.0", "1.0.0"));
    assert!(!is_newer_version("1.0", "1.0.0"));
    assert!(!is_newer_version("1.0.0.0", "1.0.0"));
    assert!(!is_newer_version("v2", "1.0.0"));
}

#[test]
fn repository_format_and_features_gate_reading_and_writing() {
    let mut newer = repo_metadata();
    newer["repository_format"]["version"] = 2.into();
    newer["repository_format"]["field_added_later"] = true.into();
    let (found, needs_newer_gutch) = unsupported(parse(&newer));
    assert_eq!(found, "format 2");
    assert!(needs_newer_gutch);

    let mut read_feature = repo_metadata();
    read_feature["repository_format"]["features_required_to_read"] = serde_json::json!(["later"]);
    assert_eq!(
        unsupported(parse(&read_feature)).0,
        "features later for reading"
    );

    let format = |required_to_write: &[&str], optional: &[&str]| RepoFormat {
        features_required_to_write: required_to_write.iter().map(|f| f.to_string()).collect(),
        optional_features: optional.iter().map(|f| f.to_string()).collect(),
        ..RepoFormat::default()
    };
    let write_only = format(&["later"], &[]);
    write_only.check(RepositoryAccess::Read).unwrap();
    assert!(write_only.check(RepositoryAccess::Write).is_err());
    let optional = format(&[], &["later"]);
    optional.check(RepositoryAccess::Write).unwrap();
}

#[test]
fn repository_format_probe_ignores_unrelated_fields() {
    let mut value = repo_metadata();
    value["field_added_later"] = true.into();
    let format = RepoFormat::from_repo_metadata(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(format, RepoFormat::default());
    value.as_object_mut().unwrap().remove("repository_format");
    assert!(RepoFormat::from_repo_metadata(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn feature_names_must_be_unique_lowercase_identifiers() {
    for features in [vec!["Upper"], vec![""], vec!["a-b"], vec!["same", "same"]] {
        let format = RepoFormat {
            optional_features: features.iter().map(|f| f.to_string()).collect(),
            ..RepoFormat::default()
        };
        assert!(format.validate().is_err(), "{features:?}");
    }
    let format = RepoFormat {
        optional_features: vec!["sparse_index_v2".into()],
        ..RepoFormat::default()
    };
    format.validate().unwrap();
}

/// Every frozen corpus repository must stay readable by this release.
#[test]
fn frozen_corpus_documents_parse_with_the_current_readers() {
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/datasets/corpus");
    let mut repositories = 0;
    for case in std::fs::read_dir(&corpus).unwrap() {
        let repository = case.unwrap().path().join("repository/format-v1");
        if !repository.is_dir() {
            continue;
        }
        let metadata = std::fs::read(repository.join(REPO_METADATA_FILE)).unwrap();
        RepoManifest::from_json_bytes(REPO_METADATA_FILE, &metadata)
            .unwrap_or_else(|error| panic!("{}: {error}", repository.display()));
        for snapshot in std::fs::read_dir(repository.join("snapshots")).unwrap() {
            let path = snapshot.unwrap().path().join("snapshot_metadata.json");
            parse_document::<crate::manifest::snapshot::SnapshotMetadata>(
                "snapshot_metadata.json",
                &std::fs::read(&path).unwrap(),
            )
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        }
        repositories += 1;
    }
    assert_eq!(repositories, 10);
}
