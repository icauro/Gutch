use super::*;

#[test]
fn formats_check_numeric_bounds_and_real_calendar_dates() {
    for value in ["0.0.0.0", "4294967295.1.2.3"] {
        assert!(SnapshotTagValueFormat::FourPartVersion.accepts(value));
    }
    for value in [
        "1.2.3",
        "v1.2.3.4",
        "01.2.3.4",
        "4294967296.0.0.0",
        "+1.2.3.4",
    ] {
        assert!(
            !SnapshotTagValueFormat::FourPartVersion.accepts(value),
            "{value}"
        );
    }
    for value in ["0001_01_01", "2000_02_29", "9999_12_31"] {
        assert!(SnapshotTagValueFormat::CalendarDate.accepts(value));
    }
    for value in [
        "0000_01_01",
        "1900_02_29",
        "2026_04_31",
        "2026_1_01",
        "2026_00_01",
        "2026_01_00",
        "日本語",
    ] {
        assert!(
            !SnapshotTagValueFormat::CalendarDate.accepts(value),
            "{value}"
        );
    }
}

#[test]
fn optional_rules_validate_only_present_values_and_reject_unknown_formats() {
    let constraints: RepoConstraints = serde_json::from_str(
        r#"{"snapshot_tags":{"key_value":{"build":{"format":"u32.u32.u32.u32"}}}}"#,
    )
    .unwrap();
    assert!(
        constraints
            .validate_snapshot_tags(&super::super::SnapshotTagSet::default(), "empty")
            .is_ok()
    );
    assert!(
        serde_json::from_str::<RepoConstraints>(
            r#"{"snapshot_tags":{"key_value":{"build":{"format":"regex"}}}}"#
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<RepoConstraints>(
            r#"{"snapshot_tags":{"key_value":{"build":{"enabled":true}}}}"#
        )
        .is_err()
    );
    let error =
        serde_json::from_str::<RepoConstraints>(r#"{"snapshot_tags":{"key_value":{"Build":{}}}}"#)
            .unwrap_err();
    assert!(error.to_string().contains("lower snake case"), "{error}");
}

#[test]
fn targets_compare_as_sets_and_unknown_targets_fail_closed() {
    let a = RepoConstraints {
        targets: vec![FilesystemTarget::LinuxExt4, FilesystemTarget::WindowsNtfs],
        ..Default::default()
    };
    let b = RepoConstraints {
        targets: vec![
            FilesystemTarget::WindowsNtfs,
            FilesystemTarget::LinuxExt4,
            FilesystemTarget::LinuxExt4,
        ],
        ..Default::default()
    };
    assert_eq!(a, b);
    assert!(serde_json::from_str::<RepoConstraints>(r#"{"targets":["windwos"]}"#).is_err());
}
