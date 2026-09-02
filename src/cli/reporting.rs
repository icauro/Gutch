use std::path::PathBuf;

use gutch as g;
use serde::Serialize;

fn secs(d: std::time::Duration) -> f64 {
    d.as_secs_f64()
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn mib_per_s(bytes: u64, d: std::time::Duration) -> f64 {
    let secs = d.as_secs_f64();
    if secs <= f64::EPSILON {
        0.0
    } else {
        mib(bytes) / secs
    }
}

fn duration_sum(values: &[std::time::Duration]) -> std::time::Duration {
    values.iter().copied().sum()
}

fn print_section_title(title: &str) {
    println!("== {} ==", title);
}

fn print_metric_line(label: &str, value: impl std::fmt::Display) {
    println!("  {label:<22} {value}");
}

fn print_blank_line() {
    println!();
}

const TABLE_PHASE_WIDTH: usize = 24;
const TABLE_SECONDS_WIDTH: usize = 14;
const TABLE_BYTES_WIDTH: usize = 32;
const TABLE_THROUGHPUT_WIDTH: usize = 20;

fn fit_table_cell(value: &str, width: usize) -> String {
    let character_count = value.chars().count();
    if character_count <= width {
        return value.to_string();
    }
    if width <= 3 {
        return ".".repeat(width);
    }

    let mut shortened = value.chars().take(width - 3).collect::<String>();
    shortened.push_str("...");
    shortened
}

fn runtime_plan_detail(plan: &g::RuntimePlan) -> String {
    format!("workers={}", plan.worker_count)
}

fn print_runtime_plan(plan: Option<&g::RuntimePlan>) {
    if let Some(plan) = plan {
        print_metric_line("runtime plan", runtime_plan_detail(plan));
    }
}

fn print_table_border() {
    println!(
        "  +{}+{}+{}+{}+",
        "-".repeat(TABLE_PHASE_WIDTH + 2),
        "-".repeat(TABLE_SECONDS_WIDTH + 2),
        "-".repeat(TABLE_BYTES_WIDTH + 2),
        "-".repeat(TABLE_THROUGHPUT_WIDTH + 2),
    );
}

fn print_table_header(seconds_label: &str) {
    print_table_border();
    let phase = fit_table_cell("phase", TABLE_PHASE_WIDTH);
    let seconds = fit_table_cell(seconds_label, TABLE_SECONDS_WIDTH);
    let bytes = fit_table_cell("bytes", TABLE_BYTES_WIDTH);
    let throughput = fit_table_cell("throughput", TABLE_THROUGHPUT_WIDTH);
    println!(
        "  | {phase:<phase_width$} | {seconds:>seconds_width$} | {bytes:<bytes_width$} | {throughput:>throughput_width$} |",
        phase_width = TABLE_PHASE_WIDTH,
        seconds_width = TABLE_SECONDS_WIDTH,
        bytes_width = TABLE_BYTES_WIDTH,
        throughput_width = TABLE_THROUGHPUT_WIDTH,
    );
    print_table_border();
}

fn print_table_row(
    phase: &str,
    level: usize,
    seconds: Option<f64>,
    bytes: Option<String>,
    throughput: Option<String>,
) {
    let phase = fit_table_cell(
        &format!("{}{}", "  ".repeat(level), phase),
        TABLE_PHASE_WIDTH,
    );
    let seconds = seconds
        .map(|value| format!("{value:.2} s"))
        .unwrap_or_else(|| "-".to_string());
    let bytes = bytes.unwrap_or_else(|| "-".to_string());
    let throughput = throughput.unwrap_or_else(|| "-".to_string());
    let seconds = fit_table_cell(&seconds, TABLE_SECONDS_WIDTH);
    let bytes = fit_table_cell(&bytes, TABLE_BYTES_WIDTH);
    let throughput = fit_table_cell(&throughput, TABLE_THROUGHPUT_WIDTH);
    println!(
        "  | {phase:<phase_width$} | {seconds:>seconds_width$} | {bytes:<bytes_width$} | {throughput:>throughput_width$} |",
        phase_width = TABLE_PHASE_WIDTH,
        seconds_width = TABLE_SECONDS_WIDTH,
        bytes_width = TABLE_BYTES_WIDTH,
        throughput_width = TABLE_THROUGHPUT_WIDTH,
    );
    print_table_border();
}

fn print_throughput_row(phase: &str, level: usize, duration: std::time::Duration, bytes: u64) {
    print_table_row(
        phase,
        level,
        Some(secs(duration)),
        Some(format!("{:.2} MiB", mib(bytes))),
        Some(format!("{:.2} MiB/s", mib_per_s(bytes, duration))),
    );
}

fn print_in_out_throughput_row(
    phase: &str,
    level: usize,
    duration: std::time::Duration,
    input_bytes: u64,
    output_bytes: u64,
) {
    print_in_out_throughput_row_with_rate(
        phase,
        level,
        duration,
        input_bytes,
        output_bytes,
        output_bytes,
    );
}

fn print_in_out_throughput_row_with_rate(
    phase: &str,
    level: usize,
    duration: std::time::Duration,
    input_bytes: u64,
    output_bytes: u64,
    rate_bytes: u64,
) {
    print_table_row(
        phase,
        level,
        Some(secs(duration)),
        Some(format!(
            "in {:.2} / out {:.2} MiB",
            mib(input_bytes),
            mib(output_bytes)
        )),
        Some(format!("{:.2} MiB/s", mib_per_s(rate_bytes, duration))),
    );
}

pub(crate) fn print_profile_summary(profile: Option<&g::GutchProfiler>) {
    let Some(profile) = profile else {
        return;
    };
    let summary = profile.summary();
    let spans = summary
        .spans
        .iter()
        .filter(|span| span.name != "gutch.cli.command")
        .collect::<Vec<_>>();
    print_section_title("Perfetto Profile Summary");
    print_metric_line(
        "completed spans",
        spans.iter().map(|span| span.count).sum::<usize>(),
    );
    print_metric_line(
        "sum of spans",
        format!(
            "{:.3} s",
            spans.iter().map(|span| span.total_seconds).sum::<f64>()
        ),
    );
    println!("  +--------------------------------------+----------+------------+------------+");
    println!(
        "  | {:<36} | {:>8} | {:>10} | {:>10} |",
        "span", "count", "sum", "max"
    );
    println!("  +--------------------------------------+----------+------------+------------+");
    for span in spans.into_iter().take(24) {
        let name = fit_table_cell(&span.name, 36);
        println!(
            "  | {:<36} | {:>8} | {:>9.3} s | {:>9.3} s |",
            name, span.count, span.total_seconds, span.maximum_seconds
        );
    }
    println!("  +--------------------------------------+----------+------------+------------+");
    print_metric_line("trace format", "Chrome JSON (Perfetto)");
}

pub(crate) fn ratio_pct(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        (numerator as f64 * 100.0) / denominator as f64
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct RuntimePlanSummary {
    worker_count: Option<usize>,
}

fn runtime_plan_summary(plan: Option<&g::RuntimePlan>) -> RuntimePlanSummary {
    RuntimePlanSummary {
        worker_count: plan.map(|plan| plan.worker_count),
    }
}

fn wall_timing_fields(total: std::time::Duration, parts: &[std::time::Duration]) -> (f64, f64) {
    let total_seconds = secs(total);
    let unaccounted_seconds = total_seconds - secs(duration_sum(parts));
    debug_assert!(unaccounted_seconds >= -1e-9);
    (total_seconds, unaccounted_seconds)
}

#[derive(Debug, Serialize)]
pub(crate) struct OptionalOutputPaths {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stats_json_out: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct BuildTimingsSummary {
    pub(crate) schema_version: u32,
    pub(crate) scan_strategy: String,
    pub(crate) runtime_plan: RuntimePlanSummary,
    pub(crate) total_seconds: f64,
    pub(crate) unaccounted_seconds: f64,
    pub(crate) phases: BuildTimingPhasesSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct BuildTimingPhasesSummary {
    pub(crate) scan_chunk: BuildScanChunkPhaseSummary,
    pub(crate) dedupe_plan: BuildDedupePlanPhaseSummary,
    pub(crate) artifact_encode: BuildArtifactEncodePhaseSummary,
    pub(crate) manifest_finalize: WallTimingPhaseSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct BuildScanChunkPhaseSummary {
    pub(crate) wall_seconds: f64,
    pub(crate) details: BuildScanChunkDetailsSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct BuildScanChunkDetailsSummary {
    pub(crate) logical_bytes: u64,
    pub(crate) stream_chunk_worker_seconds: f64,
    pub(crate) stream_chunk_bytes: u64,
    pub(crate) stream_chunk_mib_per_second: f64,
    pub(crate) file_hash_worker_seconds: f64,
    pub(crate) file_hash_bytes: u64,
    pub(crate) file_hash_mib_per_second: f64,
    pub(crate) chunk_hash_worker_seconds: f64,
    pub(crate) chunk_hash_bytes: u64,
    pub(crate) chunk_hash_mib_per_second: f64,
    pub(crate) metadata_worker_seconds: f64,
}

#[derive(Debug, Serialize)]
pub(crate) struct BuildDedupePlanPhaseSummary {
    pub(crate) wall_seconds: f64,
    pub(crate) details: BuildDedupePlanDetailsSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct BuildDedupePlanDetailsSummary {
    pub(crate) lookup_worker_seconds: f64,
    pub(crate) dedupe_lookup_bytes: u64,
    pub(crate) dedupe_lookup_mib_per_second: f64,
}

#[derive(Debug, Serialize)]
pub(crate) struct BuildArtifactEncodePhaseSummary {
    pub(crate) wall_seconds: f64,
    pub(crate) details: BuildArtifactEncodeDetailsSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct BuildArtifactEncodeDetailsSummary {
    pub(crate) compression_input_bytes: u64,
    pub(crate) compression_output_bytes: u64,
    pub(crate) compression_ratio_percent: f64,
    pub(crate) total_compressed_bytes: u64,
    pub(crate) artifact_output_bytes: u64,
    pub(crate) payload_load_worker_seconds: f64,
    pub(crate) payload_load_bytes: u64,
    pub(crate) payload_load_mib_per_second: f64,
    pub(crate) bundle_sort_worker_seconds: Option<f64>,
    pub(crate) bundle_compressed_sort_bytes: u64,
    pub(crate) bundle_compressed_sort_mib_per_second: Option<f64>,
    pub(crate) ratio_probe_worker_seconds: Option<f64>,
    pub(crate) bundle_compressed_ratio_probe_bytes: u64,
    pub(crate) bundle_compressed_ratio_probe_mib_per_second: Option<f64>,
    pub(crate) frame_compression_worker_seconds: Option<f64>,
    pub(crate) bundle_compressed_frame_input_bytes: u64,
    pub(crate) bundle_compressed_frame_output_bytes: u64,
    pub(crate) bundle_compressed_frame_input_mib_per_second: Option<f64>,
    pub(crate) bundle_compressed_frame_ratio_percent: Option<f64>,
    pub(crate) footer_serialization_worker_seconds: Option<f64>,
    pub(crate) bundle_compressed_footer_serialization_bytes: u64,
    pub(crate) bundle_compressed_footer_serialization_mib_per_second: Option<f64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReconstructTimingsSummary {
    pub(crate) schema_version: u32,
    pub(crate) runtime_plan: RuntimePlanSummary,
    pub(crate) total_seconds: f64,
    pub(crate) unaccounted_seconds: f64,
    pub(crate) phases: ReconstructTimingPhasesSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReconstructTimingPhasesSummary {
    pub(crate) prepare: WallTimingPhaseSummary,
    pub(crate) reconstruct: ReconstructReconstructPhaseSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReconstructReconstructPhaseSummary {
    pub(crate) wall_seconds: f64,
    pub(crate) details: ReconstructReconstructDetailsSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReconstructReconstructDetailsSummary {
    pub(crate) artifact_read_decode_worker_seconds: f64,
    pub(crate) artifact_read_compressed_bytes: u64,
    pub(crate) artifact_read_decompressed_bytes: u64,
    pub(crate) artifact_read_decompressed_mib_per_second: f64,
    pub(crate) bundle_read_compressed_bytes: u64,
    pub(crate) bundle_read_decompressed_bytes: u64,
    pub(crate) bundle_read_decompressed_mib_per_second: f64,
    pub(crate) file_rebuild_worker_seconds: f64,
    pub(crate) file_rebuild_bytes: u64,
    pub(crate) file_rebuild_mib_per_second: f64,
    pub(crate) chunk_verify_worker_seconds: Option<f64>,
    pub(crate) chunk_verify_bytes: Option<u64>,
    pub(crate) chunk_verify_mib_per_second: Option<f64>,
    pub(crate) file_hash_worker_seconds: Option<f64>,
    pub(crate) file_verify_bytes: u64,
    pub(crate) file_verify_mib_per_second: Option<f64>,
    pub(crate) file_write_worker_seconds: f64,
    pub(crate) file_write_bytes: u64,
    pub(crate) file_write_mib_per_second: f64,
}

#[derive(Debug, Serialize)]
pub(crate) struct VerifyTimingsSummary {
    pub(crate) schema_version: u32,
    pub(crate) verify_mode: String,
    pub(crate) runtime_plan: RuntimePlanSummary,
    pub(crate) total_seconds: f64,
    pub(crate) unaccounted_seconds: f64,
    pub(crate) phases: VerifyTimingPhasesSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct VerifyTimingPhasesSummary {
    pub(crate) prepare: WallTimingPhaseSummary,
    pub(crate) verify_files: VerifyFilesPhaseSummary,
    pub(crate) extra_file_scan: WallTimingPhaseSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct VerifyFilesPhaseSummary {
    pub(crate) wall_seconds: f64,
    pub(crate) details: VerifyFilesDetailsSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct VerifyFilesDetailsSummary {
    pub(crate) stat_worker_seconds: f64,
    pub(crate) file_read_worker_seconds: f64,
    pub(crate) file_read_bytes: u64,
    pub(crate) file_read_mib_per_second: f64,
    pub(crate) file_hash_worker_seconds: f64,
    pub(crate) file_hash_bytes: u64,
    pub(crate) file_hash_mib_per_second: f64,
    pub(crate) chunk_hash_worker_seconds: Option<f64>,
    pub(crate) chunk_hash_bytes: Option<u64>,
    pub(crate) chunk_hash_mib_per_second: Option<f64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct WallTimingPhaseSummary {
    pub(crate) wall_seconds: f64,
}

pub(crate) fn build_timings_summary(timings: &g::BuildTimings) -> BuildTimingsSummary {
    let (total_seconds, unaccounted_seconds) = wall_timing_fields(
        timings.total,
        &[
            timings.scan_chunk_wall,
            timings.dedupe_plan_wall,
            timings.artifact_encode_wall,
            timings.manifest_finalize_wall,
        ],
    );
    let runtime_plan = runtime_plan_summary(timings.runtime_plan.as_ref());
    let bundled = timings.storage_mode == g::manifest::ArtifactStorageMode::Bundled;

    BuildTimingsSummary {
        schema_version: 1,
        scan_strategy: "single_pass".to_string(),
        runtime_plan,
        total_seconds,
        unaccounted_seconds,
        phases: BuildTimingPhasesSummary {
            scan_chunk: BuildScanChunkPhaseSummary {
                wall_seconds: secs(timings.scan_chunk_wall),
                details: BuildScanChunkDetailsSummary {
                    logical_bytes: timings.logical_bytes,
                    stream_chunk_worker_seconds: secs(timings.stream_chunk),
                    stream_chunk_bytes: timings.stream_chunk_bytes,
                    stream_chunk_mib_per_second: mib_per_s(
                        timings.stream_chunk_bytes,
                        timings.stream_chunk,
                    ),
                    file_hash_worker_seconds: secs(timings.file_hash),
                    file_hash_bytes: timings.file_hash_bytes,
                    file_hash_mib_per_second: mib_per_s(timings.file_hash_bytes, timings.file_hash),
                    chunk_hash_worker_seconds: secs(timings.chunk_hash_judgment),
                    chunk_hash_bytes: timings.chunk_hash_judgment_bytes,
                    chunk_hash_mib_per_second: mib_per_s(
                        timings.chunk_hash_judgment_bytes,
                        timings.chunk_hash_judgment,
                    ),
                    metadata_worker_seconds: secs(timings.metadata),
                },
            },
            dedupe_plan: BuildDedupePlanPhaseSummary {
                wall_seconds: secs(timings.dedupe_plan_wall),
                details: BuildDedupePlanDetailsSummary {
                    lookup_worker_seconds: secs(timings.dedupe_lookup),
                    dedupe_lookup_bytes: timings.dedupe_lookup_bytes,
                    dedupe_lookup_mib_per_second: mib_per_s(
                        timings.dedupe_lookup_bytes,
                        timings.dedupe_lookup,
                    ),
                },
            },
            artifact_encode: BuildArtifactEncodePhaseSummary {
                wall_seconds: secs(timings.artifact_encode_wall),
                details: BuildArtifactEncodeDetailsSummary {
                    compression_input_bytes: timings.compression_input_bytes,
                    compression_output_bytes: timings.compression_output_bytes,
                    compression_ratio_percent: ratio_pct(
                        timings.compression_output_bytes,
                        timings.compression_input_bytes,
                    ),
                    total_compressed_bytes: timings.total_compressed_bytes,
                    artifact_output_bytes: timings.artifact_output_bytes,
                    payload_load_worker_seconds: secs(timings.bundle_compressed_payload_load),
                    payload_load_bytes: timings.bundle_compressed_payload_load_bytes,
                    payload_load_mib_per_second: mib_per_s(
                        timings.bundle_compressed_payload_load_bytes,
                        timings.bundle_compressed_payload_load,
                    ),
                    bundle_sort_worker_seconds: bundled
                        .then(|| secs(timings.bundle_compressed_sort)),
                    bundle_compressed_sort_bytes: timings.bundle_compressed_sort_bytes,
                    bundle_compressed_sort_mib_per_second: bundled.then(|| {
                        mib_per_s(
                            timings.bundle_compressed_sort_bytes,
                            timings.bundle_compressed_sort,
                        )
                    }),
                    ratio_probe_worker_seconds: bundled
                        .then(|| secs(timings.bundle_compressed_ratio_probe)),
                    bundle_compressed_ratio_probe_bytes: timings
                        .bundle_compressed_ratio_probe_bytes,
                    bundle_compressed_ratio_probe_mib_per_second: bundled.then(|| {
                        mib_per_s(
                            timings.bundle_compressed_ratio_probe_bytes,
                            timings.bundle_compressed_ratio_probe,
                        )
                    }),
                    frame_compression_worker_seconds: bundled
                        .then(|| secs(timings.bundle_compressed_frame_compression)),
                    bundle_compressed_frame_input_bytes: timings
                        .bundle_compressed_frame_input_bytes,
                    bundle_compressed_frame_output_bytes: timings
                        .bundle_compressed_frame_output_bytes,
                    bundle_compressed_frame_input_mib_per_second: bundled.then(|| {
                        mib_per_s(
                            timings.bundle_compressed_frame_input_bytes,
                            timings.bundle_compressed_frame_compression,
                        )
                    }),
                    bundle_compressed_frame_ratio_percent: bundled.then(|| {
                        ratio_pct(
                            timings.bundle_compressed_frame_output_bytes,
                            timings.bundle_compressed_frame_input_bytes,
                        )
                    }),
                    footer_serialization_worker_seconds: bundled
                        .then(|| secs(timings.bundle_compressed_footer_serialization)),
                    bundle_compressed_footer_serialization_bytes: timings
                        .bundle_compressed_footer_serialization_bytes,
                    bundle_compressed_footer_serialization_mib_per_second: bundled.then(|| {
                        mib_per_s(
                            timings.bundle_compressed_footer_serialization_bytes,
                            timings.bundle_compressed_footer_serialization,
                        )
                    }),
                },
            },
            manifest_finalize: WallTimingPhaseSummary {
                wall_seconds: secs(timings.manifest_finalize_wall),
            },
        },
    }
}

pub(crate) fn reconstruct_timings_summary(
    timings: &g::ReconstructTimings,
) -> ReconstructTimingsSummary {
    let (total_seconds, unaccounted_seconds) = wall_timing_fields(
        timings.total,
        &[timings.prepare_wall, timings.reconstruct_wall],
    );
    let runtime_plan = runtime_plan_summary(timings.runtime_plan.as_ref());

    ReconstructTimingsSummary {
        schema_version: 1,
        runtime_plan,
        total_seconds,
        unaccounted_seconds,
        phases: ReconstructTimingPhasesSummary {
            prepare: WallTimingPhaseSummary {
                wall_seconds: secs(timings.prepare_wall),
            },
            reconstruct: ReconstructReconstructPhaseSummary {
                wall_seconds: secs(timings.reconstruct_wall),
                details: ReconstructReconstructDetailsSummary {
                    artifact_read_decode_worker_seconds: secs(timings.artifact_read),
                    artifact_read_compressed_bytes: timings.artifact_read_compressed_bytes,
                    artifact_read_decompressed_bytes: timings.artifact_read_decompressed_bytes,
                    artifact_read_decompressed_mib_per_second: mib_per_s(
                        timings.artifact_read_decompressed_bytes,
                        timings.artifact_read,
                    ),
                    bundle_read_compressed_bytes: timings.bundle_read_compressed_bytes,
                    bundle_read_decompressed_bytes: timings.bundle_read_decompressed_bytes,
                    bundle_read_decompressed_mib_per_second: mib_per_s(
                        timings.bundle_read_decompressed_bytes,
                        timings.bundle_read,
                    ),
                    file_rebuild_worker_seconds: secs(timings.file_rebuild),
                    file_rebuild_bytes: timings.file_rebuild_bytes,
                    file_rebuild_mib_per_second: mib_per_s(
                        timings.file_rebuild_bytes,
                        timings.file_rebuild,
                    ),
                    chunk_verify_worker_seconds: timings
                        .verify_chunks
                        .then(|| secs(timings.chunk_verify)),
                    chunk_verify_bytes: timings.verify_chunks.then_some(timings.chunk_verify_bytes),
                    chunk_verify_mib_per_second: timings
                        .verify_chunks
                        .then(|| mib_per_s(timings.chunk_verify_bytes, timings.chunk_verify)),
                    file_hash_worker_seconds: timings
                        .verify_files
                        .then(|| secs(timings.file_verify)),
                    file_verify_bytes: timings.file_verify_bytes,
                    file_verify_mib_per_second: timings
                        .verify_files
                        .then(|| mib_per_s(timings.file_verify_bytes, timings.file_verify)),
                    file_write_worker_seconds: secs(timings.file_write),
                    file_write_bytes: timings.file_write_bytes,
                    file_write_mib_per_second: mib_per_s(
                        timings.file_write_bytes,
                        timings.file_write,
                    ),
                },
            },
        },
    }
}

pub(crate) fn verify_timings_summary(timings: &g::VerifyTimings) -> VerifyTimingsSummary {
    let (total_seconds, unaccounted_seconds) = wall_timing_fields(
        timings.total,
        &[
            timings.prepare_wall,
            timings.verify_files_wall,
            timings.extra_file_scan_wall,
        ],
    );
    let runtime_plan = runtime_plan_summary(timings.runtime_plan.as_ref());
    let checks_chunk_hashes = timings.verify_mode.checks_chunk_hashes();

    VerifyTimingsSummary {
        schema_version: 1,
        verify_mode: timings.verify_mode.as_str().to_string(),
        runtime_plan,
        total_seconds,
        unaccounted_seconds,
        phases: VerifyTimingPhasesSummary {
            prepare: WallTimingPhaseSummary {
                wall_seconds: secs(timings.prepare_wall),
            },
            verify_files: VerifyFilesPhaseSummary {
                wall_seconds: secs(timings.verify_files_wall),
                details: VerifyFilesDetailsSummary {
                    stat_worker_seconds: secs(timings.stat),
                    file_read_worker_seconds: secs(timings.file_read),
                    file_read_bytes: timings.file_read_bytes,
                    file_read_mib_per_second: mib_per_s(timings.file_read_bytes, timings.file_read),
                    file_hash_worker_seconds: secs(timings.file_hash),
                    file_hash_bytes: timings.file_hash_bytes,
                    file_hash_mib_per_second: mib_per_s(timings.file_hash_bytes, timings.file_hash),
                    chunk_hash_worker_seconds: checks_chunk_hashes
                        .then(|| secs(timings.chunk_hash)),
                    chunk_hash_bytes: checks_chunk_hashes.then_some(timings.chunk_hash_bytes),
                    chunk_hash_mib_per_second: checks_chunk_hashes
                        .then(|| mib_per_s(timings.chunk_hash_bytes, timings.chunk_hash)),
                },
            },
            extra_file_scan: WallTimingPhaseSummary {
                wall_seconds: secs(timings.extra_file_scan_wall),
            },
        },
    }
}

pub(crate) fn optional_output_paths(stats_json_out: &Option<PathBuf>) -> OptionalOutputPaths {
    OptionalOutputPaths {
        stats_json_out: stats_json_out
            .as_ref()
            .map(|path| path.display().to_string()),
    }
}

pub(crate) fn ensure_parent_directory(path: &std::path::Path) -> g::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    Ok(())
}

pub(crate) fn write_stats_json_output<T: Serialize>(
    stats_json_out: &Option<PathBuf>,
    stats: &T,
) -> g::Result<()> {
    let Some(stats_json_out) = stats_json_out else {
        return Ok(());
    };

    ensure_parent_directory(stats_json_out)?;
    let json = serde_json::to_string_pretty(stats)?;
    std::fs::write(stats_json_out, json)?;
    Ok(())
}

pub(crate) fn print_build_timings(t: &g::BuildTimings) {
    print_section_title("Build Timing Summary");
    print_runtime_plan(t.runtime_plan.as_ref());
    print_table_header("wall seconds");
    print_throughput_row("scan + chunk", 0, t.scan_chunk_wall, t.logical_bytes);
    print_throughput_row(
        "dedupe + plan",
        0,
        t.dedupe_plan_wall,
        t.dedupe_lookup_bytes,
    );
    print_throughput_row(
        "artifact encode",
        0,
        t.artifact_encode_wall,
        t.compression_input_bytes,
    );
    print_table_row(
        "manifest finalize",
        0,
        Some(secs(t.manifest_finalize_wall)),
        None,
        None,
    );
    let (_, unaccounted_seconds) = wall_timing_fields(
        t.total,
        &[
            t.scan_chunk_wall,
            t.dedupe_plan_wall,
            t.artifact_encode_wall,
            t.manifest_finalize_wall,
        ],
    );
    print_table_row("unaccounted", 0, Some(unaccounted_seconds), None, None);
    print_throughput_row("total", 0, t.total, t.logical_bytes);

    if !super::verbose_output_enabled() {
        return;
    }

    print_blank_line();
    print_section_title("Build Worker Timing Details (worker time)");
    print_table_header("worker seconds");
    print_throughput_row("file hashing", 0, t.file_hash, t.file_hash_bytes);
    print_throughput_row("stream chunk", 0, t.stream_chunk, t.stream_chunk_bytes);
    print_throughput_row(
        "chunk hash",
        0,
        t.chunk_hash_judgment,
        t.chunk_hash_judgment_bytes,
    );
    print_table_row("metadata", 0, Some(secs(t.metadata)), None, None);
    print_throughput_row("dedupe lookup", 0, t.dedupe_lookup, t.dedupe_lookup_bytes);
    if t.bundle_compressed_sort_bytes > 0 {
        print_throughput_row(
            "bundle sort",
            1,
            t.bundle_compressed_sort,
            t.bundle_compressed_sort_bytes,
        );
    }
    if t.bundle_compressed_ratio_probe_bytes > 0 {
        print_throughput_row(
            "bundle ratio probe",
            1,
            t.bundle_compressed_ratio_probe,
            t.bundle_compressed_ratio_probe_bytes,
        );
    }
    if t.bundle_compressed_frame_input_bytes > 0 {
        print_throughput_row(
            "bundle payload load",
            2,
            t.bundle_compressed_payload_load,
            t.bundle_compressed_payload_load_bytes,
        );
        print_in_out_throughput_row_with_rate(
            "bundle frame zstd",
            2,
            t.bundle_compressed_frame_compression,
            t.bundle_compressed_frame_input_bytes,
            t.bundle_compressed_frame_output_bytes,
            t.bundle_compressed_frame_input_bytes,
        );
        print_throughput_row(
            "bundle footer serialize",
            2,
            t.bundle_compressed_footer_serialization,
            t.bundle_compressed_footer_serialization_bytes,
        );
    }
    print_metric_line(
        "compression ratio",
        format!(
            "{:.2}%  (out / in)",
            ratio_pct(t.compression_output_bytes, t.compression_input_bytes)
        ),
    );
    print_metric_line(
        "total dedupe ratio",
        format!(
            "{:.2}%  (out / logical)",
            ratio_pct(t.total_compressed_bytes, t.logical_bytes)
        ),
    );
}

pub(crate) fn print_reconstruct_timings(t: &g::ReconstructTimings) {
    print_section_title("Reconstruct Timing Summary");
    print_runtime_plan(t.runtime_plan.as_ref());
    print_table_header("wall seconds");
    print_table_row("prepare", 0, Some(secs(t.prepare_wall)), None, None);
    print_throughput_row("reconstruct", 0, t.reconstruct_wall, t.file_write_bytes);
    let (_, unaccounted_seconds) =
        wall_timing_fields(t.total, &[t.prepare_wall, t.reconstruct_wall]);
    print_table_row("unaccounted", 0, Some(unaccounted_seconds), None, None);
    print_throughput_row("total", 0, t.total, t.file_write_bytes);

    if !super::verbose_output_enabled() {
        return;
    }

    print_blank_line();
    print_section_title("Reconstruct Worker Timing Details (worker time)");
    print_table_header("worker seconds");
    print_in_out_throughput_row(
        "artifact read/decode",
        0,
        t.artifact_read,
        t.artifact_read_compressed_bytes,
        t.artifact_read_decompressed_bytes,
    );
    if t.bundle_read_compressed_bytes > 0 {
        print_in_out_throughput_row(
            "bundle read/decode",
            1,
            t.bundle_read,
            t.bundle_read_compressed_bytes,
            t.bundle_read_decompressed_bytes,
        );
    }
    if t.file_rebuild > std::time::Duration::ZERO || t.file_rebuild_bytes > 0 {
        print_throughput_row("file rebuild", 0, t.file_rebuild, t.file_rebuild_bytes);
    }
    if t.verify_chunks {
        print_throughput_row("chunk verify", 0, t.chunk_verify, t.chunk_verify_bytes);
    }
    if t.verify_files {
        print_throughput_row("file hash", 0, t.file_verify, t.file_verify_bytes);
    }
    print_throughput_row("file write", 0, t.file_write, t.file_write_bytes);
}

pub(crate) fn print_verify_timings(t: &g::VerifyTimings) {
    print_section_title("Verify Timing Summary");
    print_metric_line("mode", t.verify_mode.as_str());
    print_runtime_plan(t.runtime_plan.as_ref());
    print_table_header("wall seconds");
    print_table_row("prepare", 0, Some(secs(t.prepare_wall)), None, None);
    let wall_bytes = t.file_read_bytes.max(t.file_hash_bytes);
    print_throughput_row("verify files", 0, t.verify_files_wall, wall_bytes);
    print_table_row(
        "extra file scan",
        0,
        Some(secs(t.extra_file_scan_wall)),
        None,
        None,
    );
    let (_, unaccounted_seconds) = wall_timing_fields(
        t.total,
        &[t.prepare_wall, t.verify_files_wall, t.extra_file_scan_wall],
    );
    print_table_row("unaccounted", 0, Some(unaccounted_seconds), None, None);
    print_throughput_row("total", 0, t.total, wall_bytes);

    if !super::verbose_output_enabled() {
        return;
    }

    print_blank_line();
    print_section_title("Verify Worker Timing Details (worker time)");
    print_table_header("worker seconds");
    print_table_row("stat", 0, Some(secs(t.stat)), None, None);
    print_throughput_row("file read", 0, t.file_read, t.file_read_bytes);
    print_throughput_row("file hash", 0, t.file_hash, t.file_hash_bytes);
    if t.verify_mode.checks_chunk_hashes() {
        print_throughput_row("chunk hash", 0, t.chunk_hash, t.chunk_hash_bytes);
    }
}

pub(crate) fn print_transfer_timings(t: &g::TransferStats) {
    print_section_title("Fetch Transfer Timing");
    print_table_header("wall seconds");
    print_table_row(
        "download",
        0,
        Some(t.elapsed_seconds),
        Some(format!("{:.2} MiB", mib(t.transferred_bytes))),
        Some(format!("{:.2} MiB/s", t.mib_per_second)),
    );
    print_metric_line("expected", format!("{:.2} MiB", mib(t.total_bytes)));
    print_metric_line(
        "concurrency",
        format!(
            "initial={} max={} peak={}",
            t.initial_concurrency, t.maximum_concurrency, t.peak_concurrency
        ),
    );
    print_metric_line("adjustments", t.adjustment_count);
    print_metric_line("retries", t.retry_count);
}

pub(crate) fn print_repo_check_timings(t: &crate::repo::check::RepoCheckTimings) {
    print_section_title("Check Timing");
    print_metric_line("load manifest", format!("{} ms", t.load_manifest_ms));
    print_metric_line("scan bundles", format!("{} ms", t.scan_bundles_ms));
    print_metric_line("scan artifacts", format!("{} ms", t.scan_artifacts_ms));
    print_metric_line("load chunk index", format!("{} ms", t.load_chunk_index_ms));
    print_metric_line(
        "load snapshot manifests",
        format!(
            "{} ms  ({} snapshots)",
            t.load_snapshot_manifests_ms, t.snapshot_count
        ),
    );
    print_metric_line("total", format!("{} ms", t.total_ms));
}

#[cfg(test)]
#[path = "reporting_tests.rs"]
mod tests;
