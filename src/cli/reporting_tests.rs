use super::*;
use gutch as g;
use std::time::Duration;

#[test]
fn format_bytes_zero() {
    assert_eq!(format_bytes(0), "0 B");
}

#[test]
fn format_bytes_bytes_exact() {
    assert_eq!(format_bytes(1), "1 B");
    assert_eq!(format_bytes(1023), "1023 B");
}

#[test]
fn format_bytes_kib() {
    assert_eq!(format_bytes(1024), "1.00 KiB");
    assert_eq!(format_bytes(2048), "2.00 KiB");
    assert_eq!(format_bytes(1536), "1.50 KiB");
}

#[test]
fn format_bytes_mib() {
    assert_eq!(format_bytes(1048576), "1.00 MiB");
    assert_eq!(format_bytes(2097152), "2.00 MiB");
}

#[test]
fn format_bytes_gib() {
    assert_eq!(format_bytes(1073741824), "1.00 GiB");
}

#[test]
fn format_bytes_tib() {
    let tib = 1099511627776u64;
    assert_eq!(format_bytes(tib), "1.00 TiB");
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
fn ratio_pct_full() {
    assert!((ratio_pct(100, 100) - 100.0).abs() < f64::EPSILON);
}

#[test]
fn ratio_pct_zero() {
    assert!((ratio_pct(0, 100) - 0.0).abs() < f64::EPSILON);
}

#[test]
fn build_timings_summary_reports_accounted_and_ratio_fields() {
    let timings = g::BuildTimings {
        scan_read: Duration::from_secs(2),
        scan_read_bytes: 20 * 1024 * 1024,
        file_hash: Duration::from_secs(1),
        file_hash_bytes: 10 * 1024 * 1024,
        chunking: Duration::from_secs(1),
        chunking_bytes: 10 * 1024 * 1024,
        chunk_hash_judgment: Duration::from_secs(1),
        chunk_hash_judgment_bytes: 10 * 1024 * 1024,
        metadata: Duration::from_secs(1),
        dedupe_lookup: Duration::from_secs(1),
        dedupe_lookup_bytes: 10 * 1024 * 1024,
        compression: Duration::from_secs(2),
        compression_input_bytes: 20 * 1024 * 1024,
        compression_output_bytes: 5 * 1024 * 1024,
        total_compressed_bytes: 5 * 1024 * 1024,
        bundle_compressed_sort: Duration::from_millis(400),
        bundle_compressed_sort_bytes: 20 * 1024 * 1024,
        bundle_compressed_ratio_probe: Duration::from_millis(200),
        bundle_compressed_ratio_probe_bytes: 8 * 1024 * 1024,
        bundle_compressed_payload_load: Duration::from_millis(500),
        bundle_compressed_payload_load_bytes: 10 * 1024 * 1024,
        bundle_compressed_payload_assembly: Duration::from_millis(250),
        bundle_compressed_payload_assembly_bytes: 10 * 1024 * 1024,
        bundle_compressed_stream_compression: Duration::from_secs(2),
        bundle_compressed_stream_input_bytes: 20 * 1024 * 1024,
        bundle_compressed_stream_output_bytes: 5 * 1024 * 1024,
        bundle_compressed_serialization: Duration::from_millis(200),
        bundle_compressed_serialization_bytes: 6 * 1024 * 1024,
        bundle_build: Duration::from_secs(1),
        bundle_build_bytes: 5 * 1024 * 1024,
        target_manifest_finalize: Duration::from_secs(1),
        total: Duration::from_secs(12),
        runtime_plan: None,
    };

    let summary = build_timings_summary(&timings);
    assert!((summary.compression_ratio_percent - 25.0).abs() < f64::EPSILON);
    assert!((summary.accounted_seconds - 11.0).abs() < f64::EPSILON);
    assert!((summary.unaccounted_seconds - 1.0).abs() < f64::EPSILON);
    assert!((summary.accounted_percent_of_total - (11.0 / 12.0 * 100.0)).abs() < 1e-9);
    assert!((summary.scan_read_mib_per_second - 10.0).abs() < f64::EPSILON);
    assert!((summary.bundle_compressed_sort_mib_per_second - 50.0).abs() < f64::EPSILON);
    assert!((summary.bundle_compressed_ratio_probe_mib_per_second - 40.0).abs() < f64::EPSILON);
    assert!((summary.bundle_compressed_payload_load_mib_per_second - 20.0).abs() < f64::EPSILON);
    assert!((summary.bundle_compressed_stream_ratio_percent - 25.0).abs() < f64::EPSILON);
}

#[test]
fn verify_timings_summary_reports_accounted_fields() {
    let timings = g::VerifyTimings {
        verify_mode: g::VerifyMode::Strict,
        runtime_plan: None,
        stat: Duration::from_secs(1),
        file_read: Duration::from_secs(2),
        file_read_bytes: 8 * 1024 * 1024,
        file_hash: Duration::from_secs(1),
        file_hash_bytes: 8 * 1024 * 1024,
        chunk_hash: Duration::from_secs(1),
        chunk_hash_bytes: 8 * 1024 * 1024,
        total: Duration::from_secs(6),
    };

    let summary = verify_timings_summary(&timings);
    assert_eq!(summary.verify_mode, "strict");
    assert!((summary.file_read_mib_per_second - 4.0).abs() < f64::EPSILON);
    assert!((summary.accounted_seconds - 5.0).abs() < f64::EPSILON);
    assert!((summary.unaccounted_seconds - 1.0).abs() < f64::EPSILON);
}
