use super::*;

#[test]
fn text_and_json_preserve_unicode_and_separators() {
    let text =
        SnapshotTagSet::from_compact_strings(&[" Label = 日本語:one=two,three ".into()]).unwrap();
    let json: SnapshotTagSet =
        serde_json::from_str(r#"{"key_value":{"Label":" 日本語:one=two,three "}}"#).unwrap();
    assert_eq!(text, json);
    assert_eq!(text.get("label"), Some("日本語:one=two,three"));
    assert_eq!(
        serde_json::to_string(&SnapshotTagSet::default()).unwrap(),
        r#"{"key_value":{}}"#
    );
}

#[test]
fn rejects_conflicts_and_old_shapes_without_silent_data_loss() {
    for input in [
        "key:value",
        "key::value",
        "key+=value",
        "key=",
        "1key=value",
        "key=early-access",
    ] {
        assert!(
            SnapshotTagSet::from_compact_strings(&[input.into()]).is_err(),
            "{input}"
        );
    }
    for input in [
        r#"{"scoped":{}}"#,
        r#"{"exclusive":{}}"#,
        r#"{"key_value":{"a":"one","a":"two"}}"#,
        r#"{"key_value":{"A":"one","a":"two"}}"#,
    ] {
        assert!(
            serde_json::from_str::<SnapshotTagSet>(input).is_err(),
            "{input}"
        );
    }
    assert!(SnapshotTagSet::from_compact_strings(&["key=one".into(), "key=one".into()]).is_ok());
    assert!(SnapshotTagSet::from_compact_strings(&["key=one".into(), "key=two".into()]).is_err());
}

#[test]
fn shared_tag_contract() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/datasets/tag_contract.json")).unwrap();
    for case in cases["assignments"].as_array().unwrap() {
        let input: Vec<String> = serde_json::from_value(case["input"].clone()).unwrap();
        let result = SnapshotTagSet::from_compact_strings(&input);
        if case["expected"].is_null() {
            assert!(result.is_err(), "{case}");
        } else {
            assert_eq!(
                serde_json::to_value(result.unwrap().key_value).unwrap(),
                case["expected"],
                "{case}"
            );
        }
    }
    for case in cases["formats"].as_array().unwrap() {
        let format: crate::manifest::constraints::SnapshotTagValueFormat =
            serde_json::from_value(case["format"].clone()).unwrap();
        assert_eq!(
            format.accepts(case["value"].as_str().unwrap()),
            case["valid"].as_bool().unwrap(),
            "{case}"
        );
    }
}

#[test]
fn compact_strings_use_one_value_per_key() {
    let parsed =
        SnapshotTagSet::from_compact_strings(&["loader=fabric".into(), "os=windows".into()])
            .unwrap();
    assert_eq!(parsed.get("loader"), Some("fabric"));
    assert_eq!(parsed.get("os"), Some("windows"));
}

#[test]
fn compact_strings_reject_bare_values() {
    let error = SnapshotTagSet::from_compact_strings(&["alpha".into()]).unwrap_err();
    assert!(error.to_string().contains("key=value"));
}

#[test]
fn normalized_keeps_explicit_tags() {
    let mut explicit = SnapshotTagSet::default();
    explicit.insert("release_channel", "nightly").unwrap();
    explicit.insert("loader", "fabric").unwrap();

    assert_eq!(explicit.normalized().unwrap(), explicit);
}

#[test]
fn normalized_allows_tagless_snapshots() {
    assert!(SnapshotTagSet::default().normalized().unwrap().is_empty());
}

#[test]
fn from_pairs_checks_each_pair_like_insert() {
    let tags = SnapshotTagSet::from_pairs([("Version", "1.0"), ("channel", "beta")]).unwrap();
    assert_eq!(tags.get("version"), Some("1.0"));
    assert_eq!(tags.get("channel"), Some("beta"));
    assert!(SnapshotTagSet::from_pairs([("version", "1.0"), ("version", "2.0")]).is_err());
    assert!(SnapshotTagSet::from_pairs([("version", "")]).is_err());
}
