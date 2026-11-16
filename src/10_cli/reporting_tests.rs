use super::*;
use crate::artifact::profiles::ArtifactStorageMode;
use crate::capture::build::types::BuildTimings;
use crate::installation::ReconstructTimings;
use crate::installation::inspect::VerifyMode;
use crate::installation::inspect::VerifyTimings;
use crate::profiling::ProfileMode;
use std::time::Duration;

#[test]
fn typed_summaries_hide_diagnostic_values_without_hiding_counts_or_ratios() {
    use crate::profiling::ProfileMode;
    let build = BuildTimings {
        logical_bytes: 42,
        compression_input_bytes: 100,
        compression_output_bytes: 25,
        total: Duration::from_secs(1),
        ..Default::default()
    };
    for mode in [
        ProfileMode::Off,
        ProfileMode::Summary,
        ProfileMode::Detailed,
    ] {
        let detailed = mode == ProfileMode::Detailed;
        let timed = mode != ProfileMode::Off;
        let build = build_timings_summary(&build, mode);
        // Summary mode measures phase walls; only worker details need detailed mode.
        assert_eq!(build.total_seconds.is_some(), timed);
        assert_eq!(build.phases.publish.wall_seconds.is_some(), timed);
        assert_eq!(
            build
                .phases
                .scan_chunk
                .details
                .chunk_hash_worker_seconds
                .is_some(),
            detailed
        );
        assert_eq!(build.phases.scan_chunk.details.logical_bytes, 42);
        assert_eq!(
            build
                .phases
                .artifact_encode
                .details
                .compression_ratio_percent,
            25.0
        );
        let reconstruction = reconstruct_timings_summary(
            &ReconstructTimings {
                verify_files: true,
                file_verify_bytes: 42,
                ..Default::default()
            },
            mode,
        );
        assert_eq!(
            reconstruction
                .phases
                .reconstruct
                .details
                .file_hash_worker_seconds
                .is_some(),
            detailed
        );
        assert_eq!(
            reconstruction.phases.reconstruct.details.file_verify_bytes,
            42
        );
        let verification = verify_timings_summary(
            &VerifyTimings {
                verify_mode: VerifyMode::Strict,
                chunk_hash_bytes: 42,
                ..Default::default()
            },
            mode,
        );
        assert_eq!(
            verification
                .phases
                .verify_files
                .details
                .chunk_hash_mib_per_second
                .is_some(),
            detailed
        );
        assert_eq!(
            verification.phases.verify_files.details.chunk_hash_bytes,
            Some(42)
        );
    }
}

#[test]
fn ratio_pct_zero_denominator() {
    assert_eq!(ratio_pct(100, 0), 0.0);
}

#[test]
fn ratio_pct_half() {
    assert!((ratio_pct(50, 100) - 50.0).abs() < f64::EPSILON);
}

#[test]
fn table_cells_are_truncated_without_breaking_width() {
    let value = fit_table_cell("a very long throughput value", 12);
    assert_eq!(value.chars().count(), 12);
    assert!(value.ends_with("..."));
    assert_eq!(fit_table_cell("MiB/s", 12), "MiB/s");
    assert_eq!(fit_table_cell("anything", 0), "");
}

#[test]
fn build_summary_separates_wall_phases_from_worker_details() {
    let timings = BuildTimings {
        storage_mode: ArtifactStorageMode::Bundled,
        scan_chunk_wall: Duration::from_secs(4),
        logical_bytes: 20 * 1024 * 1024,
        stream_chunk: Duration::from_secs(6),
        stream_chunk_bytes: 20 * 1024 * 1024,
        file_hash: Duration::from_secs(2),
        file_hash_bytes: 20 * 1024 * 1024,
        chunk_hash_judgment: Duration::from_secs(1),
        chunk_hash_judgment_bytes: 20 * 1024 * 1024,
        dedupe_plan_wall: Duration::from_secs(1),
        dedupe_lookup: Duration::from_millis(500),
        artifact_encode_wall: Duration::from_secs(3),
        compression_input_bytes: 20 * 1024 * 1024,
        compression_output_bytes: 5 * 1024 * 1024,
        bundle_compressed_sort: Duration::from_millis(400),
        bundle_compressed_frame_compression: Duration::from_secs(5),
        bundle_compressed_frame_input_bytes: 20 * 1024 * 1024,
        bundle_compressed_frame_output_bytes: 5 * 1024 * 1024,
        manifest_finalize_wall: Duration::from_secs(1),
        prepare_wall: Duration::from_millis(250),
        publish_wall: Duration::from_millis(500),
        total: Duration::from_secs(11),
        ..BuildTimings::default()
    };

    let summary = build_timings_summary(&timings, ProfileMode::Detailed);

    assert_eq!(summary.scan_strategy, "single_pass");
    assert!((summary.unaccounted_seconds.unwrap() - 1.25).abs() < f64::EPSILON);
    assert_eq!(summary.phases.prepare.wall_seconds, Some(0.25));
    assert_eq!(summary.phases.publish.wall_seconds, Some(0.5));
    assert_eq!(summary.phases.scan_chunk.wall_seconds, Some(4.0));
    assert_eq!(
        summary
            .phases
            .scan_chunk
            .details
            .stream_chunk_worker_seconds,
        Some(6.0)
    );
    assert_eq!(
        summary
            .phases
            .artifact_encode
            .details
            .compression_ratio_percent,
        25.0
    );
    assert_eq!(
        summary
            .phases
            .artifact_encode
            .details
            .bundle_sort_worker_seconds,
        Some(0.4)
    );

    let json = serde_json::to_value(&summary).expect("serialize capture timing summary");
    assert!(json.get("accounted_seconds").is_none());
    assert!(json.get("scan_read_seconds").is_none());
    assert!(json["phases"]["scan_chunk"]["wall_seconds"].is_number());
}

#[test]
fn reconstruct_summary_uses_null_for_disabled_verification() {
    let timings = ReconstructTimings {
        prepare_wall: Duration::from_secs(1),
        reconstruct_wall: Duration::from_secs(7),
        artifact_read: Duration::from_secs(9),
        chunk_verify: Duration::from_secs(1),
        chunk_verify_bytes: 4 * 1024 * 1024,
        file_write: Duration::from_secs(2),
        file_write_bytes: 4 * 1024 * 1024,
        total: Duration::from_secs(9),
        ..ReconstructTimings::default()
    };

    let summary = reconstruct_timings_summary(&timings, ProfileMode::Detailed);
    let json = serde_json::to_value(&summary).expect("serialize reconstruct timing summary");

    assert_eq!(summary.unaccounted_seconds, Some(1.0));
    assert_eq!(summary.phases.reconstruct.wall_seconds, Some(7.0));
    let details = &json["phases"]["reconstruct"]["details"];
    // Chunk hashes are always checked; only whole-file hashing is optional.
    assert_eq!(details["chunk_verify_mib_per_second"], 4.0);
    assert!(details["file_hash_worker_seconds"].is_null());
    assert_eq!(details["file_write_bytes"], 4 * 1024 * 1024);
    assert_eq!(details["file_write_mib_per_second"], 2.0);
    assert!(details.get("file_rebuild_bytes").is_none());
}

#[test]
fn verify_summary_reports_wall_phases_and_strict_worker_details() {
    let timings = VerifyTimings {
        verify_mode: VerifyMode::Strict,
        prepare_wall: Duration::from_secs(1),
        verify_files_wall: Duration::from_secs(4),
        extra_file_scan_wall: Duration::from_millis(500),
        stat: Duration::from_secs(2),
        file_read: Duration::from_secs(3),
        file_read_bytes: 12 * 1024 * 1024,
        file_hash: Duration::from_secs(2),
        file_hash_bytes: 12 * 1024 * 1024,
        chunk_hash: Duration::from_secs(1),
        chunk_hash_bytes: 12 * 1024 * 1024,
        total: Duration::from_secs(6),
        ..VerifyTimings::default()
    };

    let summary = verify_timings_summary(&timings, ProfileMode::Detailed);

    assert_eq!(summary.verify_mode, "strict");
    assert_eq!(summary.unaccounted_seconds, Some(0.5));
    assert_eq!(
        summary.phases.verify_files.details.file_read_mib_per_second,
        Some(4.0)
    );
    assert_eq!(
        summary
            .phases
            .verify_files
            .details
            .chunk_hash_worker_seconds,
        Some(1.0)
    );
}
