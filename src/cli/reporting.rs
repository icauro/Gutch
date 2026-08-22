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

fn runtime_plan_detail(plan: &g::RuntimePlan, include_storage: bool) -> String {
    let storage = include_storage
        .then(|| format!(" storage={}", plan.chunk_storage_mode.as_str()))
        .unwrap_or_default();
    format!(
        "requested={} effective={}{} workers={} budget={} MiB{}",
        plan.requested_memory_mode.as_str(),
        plan.effective_memory_mode.as_str(),
        storage,
        plan.worker_count,
        plan.memory_budget_mib,
        plan.available_memory_mib
            .map(|mib| format!(" available={} MiB", mib))
            .unwrap_or_default()
    )
}

fn print_runtime_plan(plan: Option<&g::RuntimePlan>, include_storage: bool) {
    if let Some(plan) = plan {
        print_metric_line("runtime plan", runtime_plan_detail(plan, include_storage));
    }
}

fn print_timing_throughput_line(label: &str, duration: std::time::Duration, bytes: u64) {
    print_metric_line(
        label,
        format!(
            "{:.2} s  {:.2} MiB  {:.2} MiB/s",
            secs(duration),
            mib(bytes),
            mib_per_s(bytes, duration)
        ),
    );
}

fn print_table_border() {
    println!(
        "  +------------------------+------------+--------------------------+--------------------+"
    );
}

fn print_table_header() {
    print_table_border();
    println!(
        "  | {:<22} | {:>10} | {:<24} | {:>18} |",
        "phase", "seconds", "bytes", "throughput"
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
    let indent = "  ".repeat(level);
    let phase = format!("{indent}{phase}");
    println!(
        "  | {:<22} | {:>10} | {:<24} | {:>18} |",
        phase,
        seconds
            .map(|value| format!("{value:.2} s"))
            .unwrap_or_else(|| "-".to_string()),
        bytes.unwrap_or_else(|| "-".to_string()),
        throughput.unwrap_or_else(|| "-".to_string()),
    );
    print_table_border();
}

pub fn ratio_pct(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        (numerator as f64 * 100.0) / denominator as f64
    }
}

#[derive(Debug, Serialize)]
pub struct RuntimePlanSummary {
    requested_memory_mode: Option<String>,
    effective_memory_mode: Option<String>,
    chunk_storage_mode: Option<String>,
    worker_count: Option<usize>,
    available_memory_mib: Option<u64>,
    memory_budget_mib: Option<u64>,
}

fn runtime_plan_summary(plan: Option<&g::RuntimePlan>) -> RuntimePlanSummary {
    RuntimePlanSummary {
        requested_memory_mode: plan.map(|plan| plan.requested_memory_mode.as_str().to_string()),
        effective_memory_mode: plan.map(|plan| plan.effective_memory_mode.as_str().to_string()),
        chunk_storage_mode: plan.map(|plan| plan.chunk_storage_mode.as_str().to_string()),
        worker_count: plan.map(|plan| plan.worker_count),
        available_memory_mib: plan.and_then(|plan| plan.available_memory_mib),
        memory_budget_mib: plan.map(|plan| plan.memory_budget_mib),
    }
}

fn wall_timing_fields(total: std::time::Duration, parts: &[std::time::Duration]) -> (f64, f64) {
    let total_seconds = secs(total);
    let unaccounted_seconds = total_seconds - secs(duration_sum(parts));
    debug_assert!(unaccounted_seconds >= -1e-9);
    (total_seconds, unaccounted_seconds)
}

#[derive(Debug, Serialize)]
pub struct OptionalOutputPaths {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats_json_out: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BuildTimingsSummary {
    pub schema_version: u32,
    pub scan_strategy: String,
    pub runtime_plan: RuntimePlanSummary,
    pub total_seconds: f64,
    pub unaccounted_seconds: f64,
    pub phases: BuildTimingPhasesSummary,
}

#[derive(Debug, Serialize)]
pub struct BuildTimingPhasesSummary {
    pub scan_chunk: BuildScanChunkPhaseSummary,
    pub dedupe_plan: BuildDedupePlanPhaseSummary,
    pub artifact_encode: BuildArtifactEncodePhaseSummary,
    pub manifest_finalize: WallTimingPhaseSummary,
}

#[derive(Debug, Serialize)]
pub struct BuildScanChunkPhaseSummary {
    pub wall_seconds: f64,
    pub details: BuildScanChunkDetailsSummary,
}

#[derive(Debug, Serialize)]
pub struct BuildScanChunkDetailsSummary {
    pub logical_bytes: u64,
    pub stream_chunk_worker_seconds: f64,
    pub stream_chunk_bytes: u64,
    pub stream_chunk_mib_per_second: f64,
    pub file_hash_worker_seconds: f64,
    pub file_hash_bytes: u64,
    pub file_hash_mib_per_second: f64,
    pub chunk_hash_worker_seconds: f64,
    pub chunk_hash_bytes: u64,
    pub chunk_hash_mib_per_second: f64,
    pub metadata_worker_seconds: f64,
}

#[derive(Debug, Serialize)]
pub struct BuildDedupePlanPhaseSummary {
    pub wall_seconds: f64,
    pub details: BuildDedupePlanDetailsSummary,
}

#[derive(Debug, Serialize)]
pub struct BuildDedupePlanDetailsSummary {
    pub lookup_worker_seconds: f64,
    pub dedupe_lookup_bytes: u64,
    pub dedupe_lookup_mib_per_second: f64,
}

#[derive(Debug, Serialize)]
pub struct BuildArtifactEncodePhaseSummary {
    pub wall_seconds: f64,
    pub details: BuildArtifactEncodeDetailsSummary,
}

#[derive(Debug, Serialize)]
pub struct BuildArtifactEncodeDetailsSummary {
    pub compression_input_bytes: u64,
    pub compression_output_bytes: u64,
    pub compression_ratio_percent: f64,
    pub total_compressed_bytes: u64,
    pub artifact_output_bytes: u64,
    pub payload_load_worker_seconds: f64,
    pub payload_load_bytes: u64,
    pub payload_load_mib_per_second: f64,
    pub bundle_sort_worker_seconds: Option<f64>,
    pub bundle_compressed_sort_bytes: u64,
    pub bundle_compressed_sort_mib_per_second: Option<f64>,
    pub ratio_probe_worker_seconds: Option<f64>,
    pub bundle_compressed_ratio_probe_bytes: u64,
    pub bundle_compressed_ratio_probe_mib_per_second: Option<f64>,
    pub payload_assembly_worker_seconds: Option<f64>,
    pub bundle_compressed_payload_assembly_bytes: u64,
    pub bundle_compressed_payload_assembly_mib_per_second: Option<f64>,
    pub zstd_worker_seconds: Option<f64>,
    pub bundle_compressed_stream_input_bytes: u64,
    pub bundle_compressed_stream_output_bytes: u64,
    pub bundle_compressed_stream_input_mib_per_second: Option<f64>,
    pub bundle_compressed_stream_ratio_percent: Option<f64>,
    pub serialization_worker_seconds: Option<f64>,
    pub bundle_compressed_serialization_bytes: u64,
    pub bundle_compressed_serialization_mib_per_second: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct ReconstructTimingsSummary {
    pub schema_version: u32,
    pub runtime_plan: RuntimePlanSummary,
    pub total_seconds: f64,
    pub unaccounted_seconds: f64,
    pub phases: ReconstructTimingPhasesSummary,
}

#[derive(Debug, Serialize)]
pub struct ReconstructTimingPhasesSummary {
    pub prepare: WallTimingPhaseSummary,
    pub reconstruct: ReconstructReconstructPhaseSummary,
}

#[derive(Debug, Serialize)]
pub struct ReconstructReconstructPhaseSummary {
    pub wall_seconds: f64,
    pub details: ReconstructReconstructDetailsSummary,
}

#[derive(Debug, Serialize)]
pub struct ReconstructReconstructDetailsSummary {
    pub artifact_read_decode_worker_seconds: f64,
    pub bundle_read_compressed_bytes: u64,
    pub bundle_read_decompressed_bytes: u64,
    pub bundle_read_decompressed_mib_per_second: f64,
    pub chunk_verify_worker_seconds: Option<f64>,
    pub chunk_verify_bytes: Option<u64>,
    pub chunk_verify_mib_per_second: Option<f64>,
    pub file_hash_worker_seconds: Option<f64>,
    pub file_verify_bytes: u64,
    pub file_verify_mib_per_second: Option<f64>,
    pub file_write_worker_seconds: f64,
    pub file_write_bytes: u64,
    pub file_write_mib_per_second: f64,
}

#[derive(Debug, Serialize)]
pub struct VerifyTimingsSummary {
    pub schema_version: u32,
    pub verify_mode: String,
    pub runtime_plan: RuntimePlanSummary,
    pub total_seconds: f64,
    pub unaccounted_seconds: f64,
    pub phases: VerifyTimingPhasesSummary,
}

#[derive(Debug, Serialize)]
pub struct VerifyTimingPhasesSummary {
    pub prepare: WallTimingPhaseSummary,
    pub verify_files: VerifyFilesPhaseSummary,
    pub extra_file_scan: WallTimingPhaseSummary,
}

#[derive(Debug, Serialize)]
pub struct VerifyFilesPhaseSummary {
    pub wall_seconds: f64,
    pub details: VerifyFilesDetailsSummary,
}

#[derive(Debug, Serialize)]
pub struct VerifyFilesDetailsSummary {
    pub stat_worker_seconds: f64,
    pub file_read_worker_seconds: f64,
    pub file_read_bytes: u64,
    pub file_read_mib_per_second: f64,
    pub file_hash_worker_seconds: f64,
    pub file_hash_bytes: u64,
    pub file_hash_mib_per_second: f64,
    pub chunk_hash_worker_seconds: Option<f64>,
    pub chunk_hash_bytes: Option<u64>,
    pub chunk_hash_mib_per_second: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct WallTimingPhaseSummary {
    pub wall_seconds: f64,
}

pub fn build_timings_summary(timings: &g::BuildTimings) -> BuildTimingsSummary {
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
                    payload_assembly_worker_seconds: bundled
                        .then(|| secs(timings.bundle_compressed_payload_assembly)),
                    bundle_compressed_payload_assembly_bytes: timings
                        .bundle_compressed_payload_assembly_bytes,
                    bundle_compressed_payload_assembly_mib_per_second: bundled.then(|| {
                        mib_per_s(
                            timings.bundle_compressed_payload_assembly_bytes,
                            timings.bundle_compressed_payload_assembly,
                        )
                    }),
                    zstd_worker_seconds: bundled
                        .then(|| secs(timings.bundle_compressed_stream_compression)),
                    bundle_compressed_stream_input_bytes: timings
                        .bundle_compressed_stream_input_bytes,
                    bundle_compressed_stream_output_bytes: timings
                        .bundle_compressed_stream_output_bytes,
                    bundle_compressed_stream_input_mib_per_second: bundled.then(|| {
                        mib_per_s(
                            timings.bundle_compressed_stream_input_bytes,
                            timings.bundle_compressed_stream_compression,
                        )
                    }),
                    bundle_compressed_stream_ratio_percent: bundled.then(|| {
                        ratio_pct(
                            timings.bundle_compressed_stream_output_bytes,
                            timings.bundle_compressed_stream_input_bytes,
                        )
                    }),
                    serialization_worker_seconds: bundled
                        .then(|| secs(timings.bundle_compressed_serialization)),
                    bundle_compressed_serialization_bytes: timings
                        .bundle_compressed_serialization_bytes,
                    bundle_compressed_serialization_mib_per_second: bundled.then(|| {
                        mib_per_s(
                            timings.bundle_compressed_serialization_bytes,
                            timings.bundle_compressed_serialization,
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

pub fn reconstruct_timings_summary(timings: &g::ReconstructTimings) -> ReconstructTimingsSummary {
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
                    artifact_read_decode_worker_seconds: secs(timings.bundle_read),
                    bundle_read_compressed_bytes: timings.bundle_read_compressed_bytes,
                    bundle_read_decompressed_bytes: timings.bundle_read_decompressed_bytes,
                    bundle_read_decompressed_mib_per_second: mib_per_s(
                        timings.bundle_read_decompressed_bytes,
                        timings.bundle_read,
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

pub fn verify_timings_summary(timings: &g::VerifyTimings) -> VerifyTimingsSummary {
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

pub fn optional_output_paths(stats_json_out: &Option<PathBuf>) -> OptionalOutputPaths {
    OptionalOutputPaths {
        stats_json_out: stats_json_out
            .as_ref()
            .map(|path| path.display().to_string()),
    }
}

pub fn ensure_parent_directory(path: &std::path::Path) -> g::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    Ok(())
}

pub fn write_stats_json_output<T: Serialize>(
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

pub fn print_build_timings(t: &g::BuildTimings) {
    print_section_title("Build Timing Summary");
    print_runtime_plan(t.runtime_plan.as_ref(), true);
    print_table_header();
    print_table_row("scan + chunk", 0, Some(secs(t.scan_chunk_wall)), None, None);
    print_table_row(
        "dedupe + plan",
        0,
        Some(secs(t.dedupe_plan_wall)),
        None,
        None,
    );
    print_table_row(
        "artifact encode",
        0,
        Some(secs(t.artifact_encode_wall)),
        None,
        None,
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
    print_table_row("total", 0, Some(secs(t.total)), None, None);

    if !super::verbose_output_enabled() {
        return;
    }

    print_blank_line();
    print_section_title("Build Worker Timing Details");
    print_table_header();
    print_table_row(
        "file hashing",
        0,
        Some(secs(t.file_hash)),
        Some(format!("{:.2} MiB", mib(t.file_hash_bytes))),
        Some(format!(
            "{:.2} MiB/s",
            mib_per_s(t.file_hash_bytes, t.file_hash)
        )),
    );
    print_table_row(
        "stream chunk",
        0,
        Some(secs(t.stream_chunk)),
        Some(format!("{:.2} MiB", mib(t.stream_chunk_bytes))),
        Some(format!(
            "{:.2} MiB/s",
            mib_per_s(t.stream_chunk_bytes, t.stream_chunk)
        )),
    );
    print_table_row(
        "chunk hash",
        0,
        Some(secs(t.chunk_hash_judgment)),
        Some(format!("{:.2} MiB", mib(t.chunk_hash_judgment_bytes))),
        Some(format!(
            "{:.2} MiB/s",
            mib_per_s(t.chunk_hash_judgment_bytes, t.chunk_hash_judgment)
        )),
    );
    print_table_row("metadata", 0, Some(secs(t.metadata)), None, None);
    print_table_row(
        "dedupe lookup",
        0,
        Some(secs(t.dedupe_lookup)),
        Some(format!("{:.2} MiB", mib(t.dedupe_lookup_bytes))),
        Some(format!(
            "{:.2} MiB/s",
            mib_per_s(t.dedupe_lookup_bytes, t.dedupe_lookup)
        )),
    );
    if t.bundle_compressed_sort_bytes > 0 {
        print_table_row(
            "bundle sort",
            1,
            Some(secs(t.bundle_compressed_sort)),
            Some(format!("{:.2} MiB", mib(t.bundle_compressed_sort_bytes))),
            Some(format!(
                "{:.2} MiB/s",
                mib_per_s(t.bundle_compressed_sort_bytes, t.bundle_compressed_sort)
            )),
        );
    }
    if t.bundle_compressed_ratio_probe_bytes > 0 {
        print_table_row(
            "bundle ratio probe",
            1,
            Some(secs(t.bundle_compressed_ratio_probe)),
            Some(format!(
                "{:.2} MiB",
                mib(t.bundle_compressed_ratio_probe_bytes)
            )),
            Some(format!(
                "{:.2} MiB/s",
                mib_per_s(
                    t.bundle_compressed_ratio_probe_bytes,
                    t.bundle_compressed_ratio_probe
                )
            )),
        );
    }
    if t.bundle_compressed_stream_input_bytes > 0 {
        print_table_row(
            "bundle payload load",
            2,
            Some(secs(t.bundle_compressed_payload_load)),
            Some(format!(
                "{:.2} MiB",
                mib(t.bundle_compressed_payload_load_bytes)
            )),
            Some(format!(
                "{:.2} MiB/s",
                mib_per_s(
                    t.bundle_compressed_payload_load_bytes,
                    t.bundle_compressed_payload_load
                )
            )),
        );
        print_table_row(
            "bundle payload asm",
            2,
            Some(secs(t.bundle_compressed_payload_assembly)),
            Some(format!(
                "{:.2} MiB",
                mib(t.bundle_compressed_payload_assembly_bytes)
            )),
            Some(format!(
                "{:.2} MiB/s",
                mib_per_s(
                    t.bundle_compressed_payload_assembly_bytes,
                    t.bundle_compressed_payload_assembly
                )
            )),
        );
        print_table_row(
            "bundle stream zstd",
            2,
            Some(secs(t.bundle_compressed_stream_compression)),
            Some(format!(
                "{:.2} / {:.2} MiB",
                mib(t.bundle_compressed_stream_input_bytes),
                mib(t.bundle_compressed_stream_output_bytes)
            )),
            Some(format!(
                "{:.2} MiB/s",
                mib_per_s(
                    t.bundle_compressed_stream_input_bytes,
                    t.bundle_compressed_stream_compression
                )
            )),
        );
        print_table_row(
            "bundle serialize",
            2,
            Some(secs(t.bundle_compressed_serialization)),
            Some(format!(
                "{:.2} MiB",
                mib(t.bundle_compressed_serialization_bytes)
            )),
            Some(format!(
                "{:.2} MiB/s",
                mib_per_s(
                    t.bundle_compressed_serialization_bytes,
                    t.bundle_compressed_serialization
                )
            )),
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

pub fn print_reconstruct_timings(t: &g::ReconstructTimings) {
    print_section_title("Reconstruct Timing Summary");
    print_runtime_plan(t.runtime_plan.as_ref(), false);
    print_table_header();
    print_table_row("prepare", 0, Some(secs(t.prepare_wall)), None, None);
    print_table_row("reconstruct", 0, Some(secs(t.reconstruct_wall)), None, None);
    let (_, unaccounted_seconds) =
        wall_timing_fields(t.total, &[t.prepare_wall, t.reconstruct_wall]);
    print_table_row("unaccounted", 0, Some(unaccounted_seconds), None, None);
    print_table_row("total", 0, Some(secs(t.total)), None, None);

    if !super::verbose_output_enabled() {
        return;
    }

    print_blank_line();
    print_section_title("Reconstruct Worker Timing Details");
    print_metric_line(
        "artifact read/decode",
        format!(
            "{:.2} s  in {:.2} MiB  out {:.2} MiB  {:.2} MiB/s",
            secs(t.bundle_read),
            mib(t.bundle_read_compressed_bytes),
            mib(t.bundle_read_decompressed_bytes),
            mib_per_s(t.bundle_read_decompressed_bytes, t.bundle_read)
        ),
    );
    if t.verify_chunks {
        print_timing_throughput_line("chunk verify", t.chunk_verify, t.chunk_verify_bytes);
    }
    if t.verify_files {
        print_timing_throughput_line("file hash", t.file_verify, t.file_verify_bytes);
    }
    print_timing_throughput_line("file write", t.file_write, t.file_write_bytes);
}

pub fn print_verify_timings(t: &g::VerifyTimings) {
    print_section_title("Verify Timing Summary");
    print_metric_line("mode", t.verify_mode.as_str());
    print_runtime_plan(t.runtime_plan.as_ref(), false);
    print_table_header();
    print_table_row("prepare", 0, Some(secs(t.prepare_wall)), None, None);
    print_table_row(
        "verify files",
        0,
        Some(secs(t.verify_files_wall)),
        None,
        None,
    );
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
    print_table_row("total", 0, Some(secs(t.total)), None, None);

    if !super::verbose_output_enabled() {
        return;
    }

    print_blank_line();
    print_section_title("Verify Worker Timing Details");
    print_metric_line("stat", format!("{:.2} s", secs(t.stat)));
    print_timing_throughput_line("file read", t.file_read, t.file_read_bytes);
    print_timing_throughput_line("file hash", t.file_hash, t.file_hash_bytes);
    if t.verify_mode.checks_chunk_hashes() {
        print_timing_throughput_line("chunk hash", t.chunk_hash, t.chunk_hash_bytes);
    }
}

pub fn print_repo_check_timings(t: &g::RepoCheckTimings) {
    print_section_title("Check Timing");
    print_metric_line("load manifest", format!("{} ms", t.load_manifest_ms));
    print_metric_line("scan bundles", format!("{} ms", t.scan_bundles_ms));
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

pub fn print_tracked_repo_check_timings(
    repo: &g::RepoCheckTimings,
    tracked: &g::TrackedRepoCheckTimings,
) {
    print_repo_check_timings(repo);
    print_section_title("Tracked Check Timing");
    print_metric_line("load config", format!("{} ms", tracked.load_config_ms));
    print_metric_line(
        "scan source snapshots",
        format!(
            "{} ms  ({} folders)",
            tracked.scan_sources_ms, tracked.source_count
        ),
    );
    print_metric_line(
        "plan source sync",
        format!("{} ms", tracked.plan_sources_ms),
    );
    print_metric_line("total tracked", format!("{} ms", tracked.total_ms));
}

#[cfg(test)]
#[path = "reporting_tests.rs"]
mod tests;
