use super::*;
use gutch as g;
use std::time::Duration;

#[test]
fn ratio_pct_zero_denominator() {
    assert_eq!(ratio_pct(100, 0), 0.0);
}

#[test]
fn ratio_pct_half() {
    assert!((ratio_pct(50, 100) - 50.0).abs() < f64::EPSILON);
}

#[test]
fn build_summary_separates_wall_phases_from_worker_details() {
    let timings = g::BuildTimings {
        storage_mode: g::manifest::ArtifactStorageMode::Bundled,
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
        dedupe_lookup_bytes: 20 * 1024 * 1024,
        artifact_encode_wall: Duration::from_secs(3),
        compression_input_bytes: 20 * 1024 * 1024,
        compression_output_bytes: 5 * 1024 * 1024,
        bundle_compressed_sort: Duration::from_millis(400),
        bundle_compressed_sort_bytes: 20 * 1024 * 1024,
        bundle_compressed_stream_compression: Duration::from_secs(5),
        bundle_compressed_stream_input_bytes: 20 * 1024 * 1024,
        bundle_compressed_stream_output_bytes: 5 * 1024 * 1024,
        manifest_finalize_wall: Duration::from_secs(1),
        total: Duration::from_secs(10),
        ..g::BuildTimings::default()
    };

    let summary = build_timings_summary(&timings);

    assert_eq!(summary.schema_version, 1);
    assert_eq!(summary.scan_strategy, "single_pass");
    assert!((summary.unaccounted_seconds - 1.0).abs() < f64::EPSILON);
    assert_eq!(summary.phases.scan_chunk.wall_seconds, 4.0);
    assert_eq!(
        summary
            .phases
            .scan_chunk
            .details
            .stream_chunk_worker_seconds,
        6.0
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

    let json = serde_json::to_value(&summary).expect("serialize build timing summary");
    assert!(json.get("accounted_seconds").is_none());
    assert!(json.get("scan_read_seconds").is_none());
    assert!(json["phases"]["scan_chunk"]["wall_seconds"].is_number());
}

#[test]
fn build_summary_reports_experimental_prehash_comparison_fields() {
    let timings = g::BuildTimings {
        experimental_prehash_reuse: true,
        prehash: Duration::from_secs(2),
        prehash_bytes: 8 * 1024 * 1024,
        prehash_reused_bytes: 6 * 1024 * 1024,
        prehash_candidate_file_count: 3,
        prehash_reused_file_count: 2,
        prehash_mismatch_file_count: 1,
        ..g::BuildTimings::default()
    };

    let summary = build_timings_summary(&timings);
    let details = &summary.phases.scan_chunk.details;

    assert_eq!(summary.scan_strategy, "experimental_prehash_reuse");
    assert_eq!(details.prehash_candidate_file_count, 3);
    assert_eq!(details.prehash_reused_file_count, 2);
    assert_eq!(details.prehash_mismatch_file_count, 1);
    assert_eq!(details.prehash_reused_bytes, 6 * 1024 * 1024);
    assert_eq!(details.prehash_mib_per_second, 4.0);
}

#[test]
fn reconstruct_summary_uses_null_for_disabled_verification() {
    let timings = g::ReconstructTimings {
        prepare_wall: Duration::from_secs(1),
        restore_wall: Duration::from_secs(7),
        bundle_read: Duration::from_secs(9),
        file_write: Duration::from_secs(11),
        total: Duration::from_secs(9),
        ..g::ReconstructTimings::default()
    };

    let summary = reconstruct_timings_summary(&timings);
    let json = serde_json::to_value(&summary).expect("serialize reconstruct timing summary");

    assert_eq!(summary.unaccounted_seconds, 1.0);
    assert_eq!(summary.phases.restore.wall_seconds, 7.0);
    assert!(json["phases"]["restore"]["details"]["chunk_verify_worker_seconds"].is_null());
    assert!(json["phases"]["restore"]["details"]["file_hash_worker_seconds"].is_null());
}

#[test]
fn verify_summary_reports_wall_phases_and_strict_worker_details() {
    let timings = g::VerifyTimings {
        verify_mode: g::VerifyMode::Strict,
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
        ..g::VerifyTimings::default()
    };

    let summary = verify_timings_summary(&timings);

    assert_eq!(summary.verify_mode, "strict");
    assert_eq!(summary.unaccounted_seconds, 0.5);
    assert_eq!(
        summary.phases.verify_files.details.file_read_mib_per_second,
        4.0
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
