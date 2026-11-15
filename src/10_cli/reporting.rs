//! Human-readable result tables and metrics.

use std::path::PathBuf;

use crate::artifact::profiles::ArtifactStorageMode;
use crate::capture::build::types::BuildTimings;
use crate::cli::output;
use crate::installation::ReconstructTimings;
use crate::installation::inspect::VerifyTimings;
use crate::profiling::ProfileMode;
use crate::profiling::Profiler;
use crate::runtime::RuntimePlan;
use crate::transfer::types::TransferStats;
use serde::Serialize;
use std::fmt::Display;
use std::fs;
use std::path::Path;
use std::time::Duration;

fn secs(d: Duration) -> f64 {
    d.as_secs_f64()
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn mib_per_s(bytes: u64, d: Duration) -> f64 {
    let secs = d.as_secs_f64();
    if secs <= f64::EPSILON {
        0.0
    } else {
        mib(bytes) / secs
    }
}

fn duration_sum(values: &[Duration]) -> Duration {
    values.iter().copied().sum()
}

fn print_section_title(title: &str) {
    output::line(format_args!("== {} ==", title));
}

fn print_metric_line(label: &str, value: impl Display) {
    output::line(format_args!("  {label:<22} {value}"));
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

fn print_table_border() {
    output::line(format_args!(
        "  +{}+{}+{}+{}+",
        "-".repeat(TABLE_PHASE_WIDTH + 2),
        "-".repeat(TABLE_SECONDS_WIDTH + 2),
        "-".repeat(TABLE_BYTES_WIDTH + 2),
        "-".repeat(TABLE_THROUGHPUT_WIDTH + 2),
    ));
}

fn print_table_header(seconds_label: &str) {
    print_table_border();
    print_table_cells("phase", seconds_label, "bytes", "throughput");
}

fn print_table_row(
    phase: &str,
    level: usize,
    seconds: Option<f64>,
    bytes: Option<String>,
    throughput: Option<String>,
) {
    let seconds = seconds
        .map(|value| format!("{value:.2} s"))
        .unwrap_or_else(|| "-".to_string());
    print_table_cells(
        &format!("{}{}", "  ".repeat(level), phase),
        &seconds,
        bytes.as_deref().unwrap_or("-"),
        throughput.as_deref().unwrap_or("-"),
    );
}

/// Prints one fitted table line followed by its border.
fn print_table_cells(phase: &str, seconds: &str, bytes: &str, throughput: &str) {
    let phase = fit_table_cell(phase, TABLE_PHASE_WIDTH);
    let seconds = fit_table_cell(seconds, TABLE_SECONDS_WIDTH);
    let bytes = fit_table_cell(bytes, TABLE_BYTES_WIDTH);
    let throughput = fit_table_cell(throughput, TABLE_THROUGHPUT_WIDTH);
    output::line(format_args!(
        "  | {phase:<phase_width$} | {seconds:>seconds_width$} | {bytes:<bytes_width$} | {throughput:>throughput_width$} |",
        phase_width = TABLE_PHASE_WIDTH,
        seconds_width = TABLE_SECONDS_WIDTH,
        bytes_width = TABLE_BYTES_WIDTH,
        throughput_width = TABLE_THROUGHPUT_WIDTH,
    ));
    print_table_border();
}

pub(crate) fn print_profile_summary(profile: Option<&Profiler>) {
    let Some(profile) = profile else {
        return;
    };
    let summary = profile.summary();
    let spans = summary
        .spans
        .iter()
        .filter(|span| span.name != "cli.command")
        .collect::<Vec<_>>();
    print_section_title("Profiling Summary");
    print_metric_line(
        "measured spans",
        spans.iter().map(|span| span.count).sum::<usize>(),
    );
    print_metric_line(
        "inclusive span sum",
        format!(
            "{:.3} s",
            spans.iter().map(|span| span.total_seconds).sum::<f64>()
        ),
    );
    output::line(format_args!(
        "  +--------------------------------------+----------+------------+------------+"
    ));
    output::line(format_args!(
        "  | {:<36} | {:>8} | {:>10} | {:>10} |",
        "span", "count", "sum", "max"
    ));
    output::line(format_args!(
        "  +--------------------------------------+----------+------------+------------+"
    ));
    for span in spans.into_iter().take(24) {
        let name = fit_table_cell(span.name, 36);
        output::line(format_args!(
            "  | {:<36} | {:>8} | {:>9.3} s | {:>9.3} s |",
            name, span.count, span.total_seconds, span.maximum_seconds
        ));
    }
    output::line(format_args!(
        "  +--------------------------------------+----------+------------+------------+"
    ));
    print_metric_line(
        "job coverage",
        "recorded jobs; nested spans overlap; recording limits may omit jobs",
    );
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

fn runtime_plan_summary(plan: Option<&RuntimePlan>) -> RuntimePlanSummary {
    RuntimePlanSummary {
        worker_count: plan.map(|plan| plan.worker_count),
    }
}

fn wall_timing_fields(total: Duration, parts: &[Duration]) -> (f64, f64) {
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
    pub(crate) total_seconds: Option<f64>,
    pub(crate) unaccounted_seconds: Option<f64>,
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
    pub(crate) wall_seconds: Option<f64>,
    pub(crate) details: BuildScanChunkDetailsSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct BuildScanChunkDetailsSummary {
    pub(crate) logical_bytes: u64,
    pub(crate) stream_chunk_worker_seconds: Option<f64>,
    pub(crate) stream_chunk_bytes: u64,
    pub(crate) stream_chunk_mib_per_second: Option<f64>,
    pub(crate) file_hash_worker_seconds: Option<f64>,
    pub(crate) file_hash_bytes: u64,
    pub(crate) file_hash_mib_per_second: Option<f64>,
    pub(crate) chunk_hash_worker_seconds: Option<f64>,
    pub(crate) chunk_hash_bytes: u64,
    pub(crate) chunk_hash_mib_per_second: Option<f64>,
    pub(crate) metadata_worker_seconds: Option<f64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct BuildDedupePlanPhaseSummary {
    pub(crate) wall_seconds: Option<f64>,
    pub(crate) details: BuildDedupePlanDetailsSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct BuildDedupePlanDetailsSummary {
    pub(crate) lookup_worker_seconds: Option<f64>,
    pub(crate) dedupe_lookup_bytes: u64,
    pub(crate) dedupe_lookup_mib_per_second: Option<f64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct BuildArtifactEncodePhaseSummary {
    pub(crate) wall_seconds: Option<f64>,
    pub(crate) details: BuildArtifactEncodeDetailsSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct BuildArtifactEncodeDetailsSummary {
    pub(crate) compression_input_bytes: u64,
    pub(crate) compression_output_bytes: u64,
    pub(crate) compression_ratio_percent: f64,
    pub(crate) total_compressed_bytes: u64,
    pub(crate) artifact_output_bytes: u64,
    pub(crate) payload_load_worker_seconds: Option<f64>,
    pub(crate) payload_load_bytes: u64,
    pub(crate) payload_load_mib_per_second: Option<f64>,
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
    pub(crate) total_seconds: Option<f64>,
    pub(crate) unaccounted_seconds: Option<f64>,
    pub(crate) phases: ReconstructTimingPhasesSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReconstructTimingPhasesSummary {
    pub(crate) prepare: WallTimingPhaseSummary,
    pub(crate) reconstruct: ReconstructPhaseSummary,
    pub(crate) finalize: WallTimingPhaseSummary,
    pub(crate) promote: WallTimingPhaseSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReconstructPhaseSummary {
    pub(crate) wall_seconds: Option<f64>,
    pub(crate) details: ReconstructDetailsSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReconstructDetailsSummary {
    pub(crate) artifact_read_decode_worker_seconds: Option<f64>,
    pub(crate) artifact_read_compressed_bytes: u64,
    pub(crate) artifact_read_decompressed_bytes: u64,
    pub(crate) artifact_read_decompressed_mib_per_second: Option<f64>,
    pub(crate) bundle_read_compressed_bytes: u64,
    pub(crate) bundle_read_decompressed_bytes: u64,
    pub(crate) bundle_read_decompressed_mib_per_second: Option<f64>,
    pub(crate) file_rebuild_worker_seconds: Option<f64>,
    pub(crate) file_rebuild_bytes: u64,
    pub(crate) file_rebuild_mib_per_second: Option<f64>,
    pub(crate) chunk_verify_worker_seconds: Option<f64>,
    pub(crate) chunk_verify_bytes: Option<u64>,
    pub(crate) chunk_verify_mib_per_second: Option<f64>,
    pub(crate) file_hash_worker_seconds: Option<f64>,
    pub(crate) file_verify_bytes: u64,
    pub(crate) file_verify_mib_per_second: Option<f64>,
    pub(crate) file_write_worker_seconds: Option<f64>,
    pub(crate) file_write_bytes: u64,
    pub(crate) file_write_mib_per_second: Option<f64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct VerifyTimingsSummary {
    pub(crate) schema_version: u32,
    pub(crate) verify_mode: String,
    pub(crate) runtime_plan: RuntimePlanSummary,
    pub(crate) total_seconds: Option<f64>,
    pub(crate) unaccounted_seconds: Option<f64>,
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
    pub(crate) wall_seconds: Option<f64>,
    pub(crate) details: VerifyFilesDetailsSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct VerifyFilesDetailsSummary {
    pub(crate) stat_worker_seconds: Option<f64>,
    pub(crate) file_read_worker_seconds: Option<f64>,
    pub(crate) file_read_bytes: u64,
    pub(crate) file_read_mib_per_second: Option<f64>,
    pub(crate) file_hash_worker_seconds: Option<f64>,
    pub(crate) file_hash_bytes: u64,
    pub(crate) file_hash_mib_per_second: Option<f64>,
    pub(crate) chunk_hash_worker_seconds: Option<f64>,
    pub(crate) chunk_hash_bytes: Option<u64>,
    pub(crate) chunk_hash_mib_per_second: Option<f64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct WallTimingPhaseSummary {
    pub(crate) wall_seconds: Option<f64>,
}

pub(crate) fn build_timings_summary(
    timings: &BuildTimings,
    mode: ProfileMode,
) -> BuildTimingsSummary {
    let detailed = mode == ProfileMode::Detailed;
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
    let bundled = timings.storage_mode == ArtifactStorageMode::Bundled;

    BuildTimingsSummary {
        schema_version: 1,
        scan_strategy: "single_pass".to_string(),
        runtime_plan,
        total_seconds: detailed.then_some(total_seconds),
        unaccounted_seconds: detailed.then_some(unaccounted_seconds),
        phases: BuildTimingPhasesSummary {
            scan_chunk: BuildScanChunkPhaseSummary {
                wall_seconds: detailed.then_some(secs(timings.scan_chunk_wall)),
                details: BuildScanChunkDetailsSummary {
                    logical_bytes: timings.logical_bytes,
                    stream_chunk_worker_seconds: detailed.then_some(secs(timings.stream_chunk)),
                    stream_chunk_bytes: timings.stream_chunk_bytes,
                    stream_chunk_mib_per_second: detailed
                        .then_some(mib_per_s(timings.stream_chunk_bytes, timings.stream_chunk)),
                    file_hash_worker_seconds: detailed.then_some(secs(timings.file_hash)),
                    file_hash_bytes: timings.file_hash_bytes,
                    file_hash_mib_per_second: detailed
                        .then_some(mib_per_s(timings.file_hash_bytes, timings.file_hash)),
                    chunk_hash_worker_seconds: detailed
                        .then_some(secs(timings.chunk_hash_judgment)),
                    chunk_hash_bytes: timings.chunk_hash_judgment_bytes,
                    chunk_hash_mib_per_second: detailed.then_some(mib_per_s(
                        timings.chunk_hash_judgment_bytes,
                        timings.chunk_hash_judgment,
                    )),
                    metadata_worker_seconds: detailed.then_some(secs(timings.metadata)),
                },
            },
            dedupe_plan: BuildDedupePlanPhaseSummary {
                wall_seconds: detailed.then_some(secs(timings.dedupe_plan_wall)),
                details: BuildDedupePlanDetailsSummary {
                    lookup_worker_seconds: detailed.then_some(secs(timings.dedupe_lookup)),
                    dedupe_lookup_bytes: timings.dedupe_lookup_bytes,
                    dedupe_lookup_mib_per_second: detailed.then_some(mib_per_s(
                        timings.dedupe_lookup_bytes,
                        timings.dedupe_lookup,
                    )),
                },
            },
            artifact_encode: BuildArtifactEncodePhaseSummary {
                wall_seconds: detailed.then_some(secs(timings.artifact_encode_wall)),
                details: BuildArtifactEncodeDetailsSummary {
                    compression_input_bytes: timings.compression_input_bytes,
                    compression_output_bytes: timings.compression_output_bytes,
                    compression_ratio_percent: ratio_pct(
                        timings.compression_output_bytes,
                        timings.compression_input_bytes,
                    ),
                    total_compressed_bytes: timings.total_compressed_bytes,
                    artifact_output_bytes: timings.artifact_output_bytes,
                    payload_load_worker_seconds: detailed
                        .then_some(secs(timings.artifact_payload_load)),
                    payload_load_bytes: timings.artifact_payload_load_bytes,
                    payload_load_mib_per_second: detailed.then_some(mib_per_s(
                        timings.artifact_payload_load_bytes,
                        timings.artifact_payload_load,
                    )),
                    bundle_sort_worker_seconds: (bundled
                        .then(|| secs(timings.bundle_compressed_sort)))
                    .filter(|_| detailed),
                    bundle_compressed_sort_bytes: timings.bundle_compressed_sort_bytes,
                    bundle_compressed_sort_mib_per_second: (bundled.then(|| {
                        mib_per_s(
                            timings.bundle_compressed_sort_bytes,
                            timings.bundle_compressed_sort,
                        )
                    }))
                    .filter(|_| detailed),
                    ratio_probe_worker_seconds: (bundled
                        .then(|| secs(timings.bundle_compressed_ratio_probe)))
                    .filter(|_| detailed),
                    bundle_compressed_ratio_probe_bytes: timings
                        .bundle_compressed_ratio_probe_bytes,
                    bundle_compressed_ratio_probe_mib_per_second: (bundled.then(|| {
                        mib_per_s(
                            timings.bundle_compressed_ratio_probe_bytes,
                            timings.bundle_compressed_ratio_probe,
                        )
                    }))
                    .filter(|_| detailed),
                    frame_compression_worker_seconds: (bundled
                        .then(|| secs(timings.bundle_compressed_frame_compression)))
                    .filter(|_| detailed),
                    bundle_compressed_frame_input_bytes: timings
                        .bundle_compressed_frame_input_bytes,
                    bundle_compressed_frame_output_bytes: timings
                        .bundle_compressed_frame_output_bytes,
                    bundle_compressed_frame_input_mib_per_second: (bundled.then(|| {
                        mib_per_s(
                            timings.bundle_compressed_frame_input_bytes,
                            timings.bundle_compressed_frame_compression,
                        )
                    }))
                    .filter(|_| detailed),
                    bundle_compressed_frame_ratio_percent: bundled.then(|| {
                        ratio_pct(
                            timings.bundle_compressed_frame_output_bytes,
                            timings.bundle_compressed_frame_input_bytes,
                        )
                    }),
                    footer_serialization_worker_seconds: (bundled
                        .then(|| secs(timings.bundle_compressed_footer_serialization)))
                    .filter(|_| detailed),
                    bundle_compressed_footer_serialization_bytes: timings
                        .bundle_compressed_footer_serialization_bytes,
                    bundle_compressed_footer_serialization_mib_per_second: (bundled.then(|| {
                        mib_per_s(
                            timings.bundle_compressed_footer_serialization_bytes,
                            timings.bundle_compressed_footer_serialization,
                        )
                    }))
                    .filter(|_| detailed),
                },
            },
            manifest_finalize: WallTimingPhaseSummary {
                wall_seconds: detailed.then_some(secs(timings.manifest_finalize_wall)),
            },
        },
    }
}

pub(crate) fn reconstruct_timings_summary(
    timings: &ReconstructTimings,
    mode: ProfileMode,
) -> ReconstructTimingsSummary {
    let detailed = mode == ProfileMode::Detailed;
    let (total_seconds, unaccounted_seconds) = wall_timing_fields(
        timings.total,
        &[
            timings.prepare_wall,
            timings.reconstruct_wall,
            timings.finalize_wall,
            timings.promote_wall,
        ],
    );
    let runtime_plan = runtime_plan_summary(timings.runtime_plan.as_ref());

    ReconstructTimingsSummary {
        schema_version: 1,
        runtime_plan,
        total_seconds: detailed.then_some(total_seconds),
        unaccounted_seconds: detailed.then_some(unaccounted_seconds),
        phases: ReconstructTimingPhasesSummary {
            prepare: WallTimingPhaseSummary {
                wall_seconds: detailed.then_some(secs(timings.prepare_wall)),
            },
            reconstruct: ReconstructPhaseSummary {
                wall_seconds: detailed.then_some(secs(timings.reconstruct_wall)),
                details: ReconstructDetailsSummary {
                    artifact_read_decode_worker_seconds: detailed
                        .then_some(secs(timings.artifact_read)),
                    artifact_read_compressed_bytes: timings.artifact_read_compressed_bytes,
                    artifact_read_decompressed_bytes: timings.artifact_read_decompressed_bytes,
                    artifact_read_decompressed_mib_per_second: detailed.then_some(mib_per_s(
                        timings.artifact_read_decompressed_bytes,
                        timings.artifact_read,
                    )),
                    bundle_read_compressed_bytes: timings.bundle_read_compressed_bytes,
                    bundle_read_decompressed_bytes: timings.bundle_read_decompressed_bytes,
                    bundle_read_decompressed_mib_per_second: detailed.then_some(mib_per_s(
                        timings.bundle_read_decompressed_bytes,
                        timings.bundle_read,
                    )),
                    file_rebuild_worker_seconds: detailed.then_some(secs(timings.file_rebuild)),
                    file_rebuild_bytes: timings.file_rebuild_bytes,
                    file_rebuild_mib_per_second: detailed
                        .then_some(mib_per_s(timings.file_rebuild_bytes, timings.file_rebuild)),
                    chunk_verify_worker_seconds: (timings
                        .verify_chunks
                        .then(|| secs(timings.chunk_verify)))
                    .filter(|_| detailed),
                    chunk_verify_bytes: timings.verify_chunks.then_some(timings.chunk_verify_bytes),
                    chunk_verify_mib_per_second: (timings
                        .verify_chunks
                        .then(|| mib_per_s(timings.chunk_verify_bytes, timings.chunk_verify)))
                    .filter(|_| detailed),
                    file_hash_worker_seconds: (timings
                        .verify_files
                        .then(|| secs(timings.file_verify)))
                    .filter(|_| detailed),
                    file_verify_bytes: timings.file_verify_bytes,
                    file_verify_mib_per_second: (timings
                        .verify_files
                        .then(|| mib_per_s(timings.file_verify_bytes, timings.file_verify)))
                    .filter(|_| detailed),
                    file_write_worker_seconds: detailed.then_some(secs(timings.file_write)),
                    file_write_bytes: timings.file_write_bytes,
                    file_write_mib_per_second: detailed
                        .then_some(mib_per_s(timings.file_write_bytes, timings.file_write)),
                },
            },
            finalize: WallTimingPhaseSummary {
                wall_seconds: detailed.then_some(secs(timings.finalize_wall)),
            },
            promote: WallTimingPhaseSummary {
                wall_seconds: detailed.then_some(secs(timings.promote_wall)),
            },
        },
    }
}

pub(crate) fn verify_timings_summary(
    timings: &VerifyTimings,
    mode: ProfileMode,
) -> VerifyTimingsSummary {
    let detailed = mode == ProfileMode::Detailed;
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
        total_seconds: detailed.then_some(total_seconds),
        unaccounted_seconds: detailed.then_some(unaccounted_seconds),
        phases: VerifyTimingPhasesSummary {
            prepare: WallTimingPhaseSummary {
                wall_seconds: detailed.then_some(secs(timings.prepare_wall)),
            },
            verify_files: VerifyFilesPhaseSummary {
                wall_seconds: detailed.then_some(secs(timings.verify_files_wall)),
                details: VerifyFilesDetailsSummary {
                    stat_worker_seconds: detailed.then_some(secs(timings.stat)),
                    file_read_worker_seconds: detailed.then_some(secs(timings.file_read)),
                    file_read_bytes: timings.file_read_bytes,
                    file_read_mib_per_second: detailed
                        .then_some(mib_per_s(timings.file_read_bytes, timings.file_read)),
                    file_hash_worker_seconds: detailed.then_some(secs(timings.file_hash)),
                    file_hash_bytes: timings.file_hash_bytes,
                    file_hash_mib_per_second: detailed
                        .then_some(mib_per_s(timings.file_hash_bytes, timings.file_hash)),
                    chunk_hash_worker_seconds: (checks_chunk_hashes
                        .then(|| secs(timings.chunk_hash)))
                    .filter(|_| detailed),
                    chunk_hash_bytes: checks_chunk_hashes.then_some(timings.chunk_hash_bytes),
                    chunk_hash_mib_per_second: (checks_chunk_hashes
                        .then(|| mib_per_s(timings.chunk_hash_bytes, timings.chunk_hash)))
                    .filter(|_| detailed),
                },
            },
            extra_file_scan: WallTimingPhaseSummary {
                wall_seconds: detailed.then_some(secs(timings.extra_file_scan_wall)),
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

pub(crate) fn ensure_parent_directory(path: &Path) -> crate::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    Ok(())
}

pub(crate) fn write_stats_json_output<T: Serialize>(
    stats_json_out: &Option<PathBuf>,
    stats: &T,
    profile: Option<&Profiler>,
) -> crate::Result<()> {
    let mut value = serde_json::to_value(stats)?;
    let mode = profile.map(|p| p.mode()).unwrap_or_default();
    value["profiling_mode"] = serde_json::to_value(mode)?;
    let json = serde_json::to_string_pretty(&value)?;
    output::record(&value)?;
    if let Some(stats_json_out) = stats_json_out {
        ensure_parent_directory(stats_json_out)?;
        fs::write(stats_json_out, json)?;
    }
    Ok(())
}

pub(crate) fn print_transfer_timings(t: &TransferStats) {
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

#[cfg(test)]
#[path = "reporting_tests.rs"]
mod tests;

pub(crate) fn profile_mode(profile: Option<&Profiler>) -> ProfileMode {
    profile.map(|profile| profile.mode()).unwrap_or_default()
}

pub(crate) fn diagnostic_seconds(duration: Duration, profile: Option<&Profiler>) -> Option<f64> {
    (profile_mode(profile) == ProfileMode::Detailed).then_some(secs(duration))
}
