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

fn pct_of_total(part: f64, total: f64) -> f64 {
    if total <= f64::EPSILON {
        0.0
    } else {
        (part * 100.0) / total
    }
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

fn print_benchmark_snapshot(title: &str, total: std::time::Duration, accounted: f64) {
    print_blank_line();
    print_section_title(title);
    print_metric_line("total", format!("{:.2} s", secs(total)));
    print_metric_line(
        "accounted",
        format!("{:.2}%", pct_of_total(accounted, secs(total))),
    );
    print_metric_line(
        "unaccounted",
        format!("{:.2} s", (secs(total) - accounted).max(0.0)),
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

struct RuntimePlanSummary {
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

fn accounted_timing_fields(
    total: std::time::Duration,
    parts: &[std::time::Duration],
) -> (f64, f64, f64, f64) {
    let total_seconds = secs(total);
    let accounted_seconds = secs(duration_sum(parts));
    (
        accounted_seconds,
        (total_seconds - accounted_seconds).max(0.0),
        pct_of_total(accounted_seconds, total_seconds),
        total_seconds,
    )
}

#[derive(Debug, Serialize)]
pub struct OptionalOutputPaths {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats_json_out: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BuildTimingsSummary {
    pub requested_memory_mode: Option<String>,
    pub effective_memory_mode: Option<String>,
    pub chunk_storage_mode: Option<String>,
    pub worker_count: Option<usize>,
    pub available_memory_mib: Option<u64>,
    pub memory_budget_mib: Option<u64>,
    pub scan_read_seconds: f64,
    pub scan_read_bytes: u64,
    pub scan_read_mib_per_second: f64,
    pub file_hash_seconds: f64,
    pub file_hash_bytes: u64,
    pub file_hash_mib_per_second: f64,
    pub chunking_seconds: f64,
    pub chunking_bytes: u64,
    pub chunking_mib_per_second: f64,
    pub chunk_hash_judgment_seconds: f64,
    pub chunk_hash_judgment_bytes: u64,
    pub chunk_hash_judgment_mib_per_second: f64,
    pub metadata_seconds: f64,
    pub dedupe_lookup_seconds: f64,
    pub dedupe_lookup_bytes: u64,
    pub dedupe_lookup_mib_per_second: f64,
    pub compression_seconds: f64,
    pub compression_input_bytes: u64,
    pub compression_output_bytes: u64,
    pub compression_input_mib_per_second: f64,
    pub compression_ratio_percent: f64,
    pub total_compressed_bytes: u64,
    pub bundle_compressed_sort_seconds: f64,
    pub bundle_compressed_sort_bytes: u64,
    pub bundle_compressed_sort_mib_per_second: f64,
    pub bundle_compressed_ratio_probe_seconds: f64,
    pub bundle_compressed_ratio_probe_bytes: u64,
    pub bundle_compressed_ratio_probe_mib_per_second: f64,
    pub bundle_compressed_payload_load_seconds: f64,
    pub bundle_compressed_payload_load_bytes: u64,
    pub bundle_compressed_payload_load_mib_per_second: f64,
    pub bundle_compressed_payload_assembly_seconds: f64,
    pub bundle_compressed_payload_assembly_bytes: u64,
    pub bundle_compressed_payload_assembly_mib_per_second: f64,
    pub bundle_compressed_stream_compression_seconds: f64,
    pub bundle_compressed_stream_input_bytes: u64,
    pub bundle_compressed_stream_output_bytes: u64,
    pub bundle_compressed_stream_input_mib_per_second: f64,
    pub bundle_compressed_stream_ratio_percent: f64,
    pub bundle_compressed_serialization_seconds: f64,
    pub bundle_compressed_serialization_bytes: u64,
    pub bundle_compressed_serialization_mib_per_second: f64,
    pub bundle_build_seconds: f64,
    pub bundle_build_bytes: u64,
    pub bundle_build_mib_per_second: f64,
    pub target_manifest_finalize_seconds: f64,
    pub accounted_seconds: f64,
    pub unaccounted_seconds: f64,
    pub accounted_percent_of_total: f64,
    pub total_seconds: f64,
}

#[derive(Debug, Serialize)]
pub struct ReconstructTimingsSummary {
    pub requested_memory_mode: Option<String>,
    pub effective_memory_mode: Option<String>,
    pub worker_count: Option<usize>,
    pub available_memory_mib: Option<u64>,
    pub memory_budget_mib: Option<u64>,
    pub bundle_read_seconds: f64,
    pub bundle_read_compressed_bytes: u64,
    pub bundle_read_decompressed_bytes: u64,
    pub bundle_read_decompressed_mib_per_second: f64,
    pub file_rebuild_seconds: f64,
    pub file_rebuild_bytes: u64,
    pub file_rebuild_mib_per_second: f64,
    pub file_verify_seconds: f64,
    pub file_verify_bytes: u64,
    pub file_verify_mib_per_second: f64,
    pub file_write_seconds: f64,
    pub file_write_bytes: u64,
    pub file_write_mib_per_second: f64,
    pub accounted_seconds: f64,
    pub unaccounted_seconds: f64,
    pub accounted_percent_of_total: f64,
    pub total_seconds: f64,
}

#[derive(Debug, Serialize)]
pub struct VerifyTimingsSummary {
    pub verify_mode: String,
    pub requested_memory_mode: Option<String>,
    pub effective_memory_mode: Option<String>,
    pub worker_count: Option<usize>,
    pub available_memory_mib: Option<u64>,
    pub memory_budget_mib: Option<u64>,
    pub stat_seconds: f64,
    pub file_read_seconds: f64,
    pub file_read_bytes: u64,
    pub file_read_mib_per_second: f64,
    pub file_hash_seconds: f64,
    pub file_hash_bytes: u64,
    pub file_hash_mib_per_second: f64,
    pub chunk_hash_seconds: f64,
    pub chunk_hash_bytes: u64,
    pub chunk_hash_mib_per_second: f64,
    pub accounted_seconds: f64,
    pub unaccounted_seconds: f64,
    pub accounted_percent_of_total: f64,
    pub total_seconds: f64,
}

pub fn build_timings_summary(timings: &g::BuildTimings) -> BuildTimingsSummary {
    let scan_read_seconds = secs(timings.scan_read);
    let file_hash_seconds = secs(timings.file_hash);
    let chunking_seconds = secs(timings.chunking);
    let chunk_hash_judgment_seconds = secs(timings.chunk_hash_judgment);
    let metadata_seconds = secs(timings.metadata);
    let dedupe_lookup_seconds = secs(timings.dedupe_lookup);
    let compression_seconds = secs(timings.compression);
    let bundle_build_seconds = secs(timings.bundle_build);
    let bundle_compressed_sort_seconds = secs(timings.bundle_compressed_sort);
    let bundle_compressed_ratio_probe_seconds = secs(timings.bundle_compressed_ratio_probe);
    let bundle_compressed_payload_load_seconds = secs(timings.bundle_compressed_payload_load);
    let bundle_compressed_payload_assembly_seconds =
        secs(timings.bundle_compressed_payload_assembly);
    let bundle_compressed_stream_compression_seconds =
        secs(timings.bundle_compressed_stream_compression);
    let bundle_compressed_serialization_seconds = secs(timings.bundle_compressed_serialization);
    let target_manifest_finalize_seconds = secs(timings.target_manifest_finalize);
    let (accounted_seconds, unaccounted_seconds, accounted_percent_of_total, total_seconds) =
        accounted_timing_fields(
            timings.total,
            &[
                timings.scan_read,
                timings.file_hash,
                timings.chunking,
                timings.chunk_hash_judgment,
                timings.metadata,
                timings.dedupe_lookup,
                timings.compression,
                timings.bundle_build,
                timings.target_manifest_finalize,
            ],
        );
    let runtime_plan = runtime_plan_summary(timings.runtime_plan.as_ref());

    BuildTimingsSummary {
        requested_memory_mode: runtime_plan.requested_memory_mode,
        effective_memory_mode: runtime_plan.effective_memory_mode,
        chunk_storage_mode: runtime_plan.chunk_storage_mode,
        worker_count: runtime_plan.worker_count,
        available_memory_mib: runtime_plan.available_memory_mib,
        memory_budget_mib: runtime_plan.memory_budget_mib,
        scan_read_seconds,
        scan_read_bytes: timings.scan_read_bytes,
        scan_read_mib_per_second: mib_per_s(timings.scan_read_bytes, timings.scan_read),
        file_hash_seconds,
        file_hash_bytes: timings.file_hash_bytes,
        file_hash_mib_per_second: mib_per_s(timings.file_hash_bytes, timings.file_hash),
        chunking_seconds,
        chunking_bytes: timings.chunking_bytes,
        chunking_mib_per_second: mib_per_s(timings.chunking_bytes, timings.chunking),
        chunk_hash_judgment_seconds,
        chunk_hash_judgment_bytes: timings.chunk_hash_judgment_bytes,
        chunk_hash_judgment_mib_per_second: mib_per_s(
            timings.chunk_hash_judgment_bytes,
            timings.chunk_hash_judgment,
        ),
        metadata_seconds,
        dedupe_lookup_seconds,
        dedupe_lookup_bytes: timings.dedupe_lookup_bytes,
        dedupe_lookup_mib_per_second: mib_per_s(timings.dedupe_lookup_bytes, timings.dedupe_lookup),
        compression_seconds,
        compression_input_bytes: timings.compression_input_bytes,
        compression_output_bytes: timings.compression_output_bytes,
        compression_input_mib_per_second: mib_per_s(
            timings.compression_input_bytes,
            timings.compression,
        ),
        compression_ratio_percent: ratio_pct(
            timings.compression_output_bytes,
            timings.compression_input_bytes,
        ),
        total_compressed_bytes: timings.total_compressed_bytes,
        bundle_compressed_sort_seconds,
        bundle_compressed_sort_bytes: timings.bundle_compressed_sort_bytes,
        bundle_compressed_sort_mib_per_second: mib_per_s(
            timings.bundle_compressed_sort_bytes,
            timings.bundle_compressed_sort,
        ),
        bundle_compressed_ratio_probe_seconds,
        bundle_compressed_ratio_probe_bytes: timings.bundle_compressed_ratio_probe_bytes,
        bundle_compressed_ratio_probe_mib_per_second: mib_per_s(
            timings.bundle_compressed_ratio_probe_bytes,
            timings.bundle_compressed_ratio_probe,
        ),
        bundle_compressed_payload_load_seconds,
        bundle_compressed_payload_load_bytes: timings.bundle_compressed_payload_load_bytes,
        bundle_compressed_payload_load_mib_per_second: mib_per_s(
            timings.bundle_compressed_payload_load_bytes,
            timings.bundle_compressed_payload_load,
        ),
        bundle_compressed_payload_assembly_seconds,
        bundle_compressed_payload_assembly_bytes: timings.bundle_compressed_payload_assembly_bytes,
        bundle_compressed_payload_assembly_mib_per_second: mib_per_s(
            timings.bundle_compressed_payload_assembly_bytes,
            timings.bundle_compressed_payload_assembly,
        ),
        bundle_compressed_stream_compression_seconds,
        bundle_compressed_stream_input_bytes: timings.bundle_compressed_stream_input_bytes,
        bundle_compressed_stream_output_bytes: timings.bundle_compressed_stream_output_bytes,
        bundle_compressed_stream_input_mib_per_second: mib_per_s(
            timings.bundle_compressed_stream_input_bytes,
            timings.bundle_compressed_stream_compression,
        ),
        bundle_compressed_stream_ratio_percent: ratio_pct(
            timings.bundle_compressed_stream_output_bytes,
            timings.bundle_compressed_stream_input_bytes,
        ),
        bundle_compressed_serialization_seconds,
        bundle_compressed_serialization_bytes: timings.bundle_compressed_serialization_bytes,
        bundle_compressed_serialization_mib_per_second: mib_per_s(
            timings.bundle_compressed_serialization_bytes,
            timings.bundle_compressed_serialization,
        ),
        bundle_build_seconds,
        bundle_build_bytes: timings.bundle_build_bytes,
        bundle_build_mib_per_second: mib_per_s(timings.bundle_build_bytes, timings.bundle_build),
        target_manifest_finalize_seconds,
        accounted_seconds,
        unaccounted_seconds,
        accounted_percent_of_total,
        total_seconds,
    }
}

pub fn reconstruct_timings_summary(timings: &g::ReconstructTimings) -> ReconstructTimingsSummary {
    let bundle_read_seconds = secs(timings.bundle_read);
    let file_rebuild_seconds = secs(timings.file_rebuild);
    let file_verify_seconds = secs(timings.file_verify);
    let file_write_seconds = secs(timings.file_write);
    let (accounted_seconds, unaccounted_seconds, accounted_percent_of_total, total_seconds) =
        accounted_timing_fields(
            timings.total,
            &[
                timings.bundle_read,
                timings.file_rebuild,
                timings.file_verify,
                timings.file_write,
            ],
        );
    let runtime_plan = runtime_plan_summary(timings.runtime_plan.as_ref());

    ReconstructTimingsSummary {
        requested_memory_mode: runtime_plan.requested_memory_mode,
        effective_memory_mode: runtime_plan.effective_memory_mode,
        worker_count: runtime_plan.worker_count,
        available_memory_mib: runtime_plan.available_memory_mib,
        memory_budget_mib: runtime_plan.memory_budget_mib,
        bundle_read_seconds,
        bundle_read_compressed_bytes: timings.bundle_read_compressed_bytes,
        bundle_read_decompressed_bytes: timings.bundle_read_decompressed_bytes,
        bundle_read_decompressed_mib_per_second: mib_per_s(
            timings.bundle_read_decompressed_bytes,
            timings.bundle_read,
        ),
        file_rebuild_seconds,
        file_rebuild_bytes: timings.file_rebuild_bytes,
        file_rebuild_mib_per_second: mib_per_s(timings.file_rebuild_bytes, timings.file_rebuild),
        file_verify_seconds,
        file_verify_bytes: timings.file_verify_bytes,
        file_verify_mib_per_second: mib_per_s(timings.file_verify_bytes, timings.file_verify),
        file_write_seconds,
        file_write_bytes: timings.file_write_bytes,
        file_write_mib_per_second: mib_per_s(timings.file_write_bytes, timings.file_write),
        accounted_seconds,
        unaccounted_seconds,
        accounted_percent_of_total,
        total_seconds,
    }
}

pub fn verify_timings_summary(timings: &g::VerifyTimings) -> VerifyTimingsSummary {
    let stat_seconds = secs(timings.stat);
    let file_read_seconds = secs(timings.file_read);
    let file_hash_seconds = secs(timings.file_hash);
    let chunk_hash_seconds = secs(timings.chunk_hash);
    let (accounted_seconds, unaccounted_seconds, accounted_percent_of_total, total_seconds) =
        accounted_timing_fields(
            timings.total,
            &[
                timings.stat,
                timings.file_read,
                timings.file_hash,
                timings.chunk_hash,
            ],
        );
    let runtime_plan = runtime_plan_summary(timings.runtime_plan.as_ref());

    VerifyTimingsSummary {
        verify_mode: timings.verify_mode.as_str().to_string(),
        requested_memory_mode: runtime_plan.requested_memory_mode,
        effective_memory_mode: runtime_plan.effective_memory_mode,
        worker_count: runtime_plan.worker_count,
        available_memory_mib: runtime_plan.available_memory_mib,
        memory_budget_mib: runtime_plan.memory_budget_mib,
        stat_seconds,
        file_read_seconds,
        file_read_bytes: timings.file_read_bytes,
        file_read_mib_per_second: mib_per_s(timings.file_read_bytes, timings.file_read),
        file_hash_seconds,
        file_hash_bytes: timings.file_hash_bytes,
        file_hash_mib_per_second: mib_per_s(timings.file_hash_bytes, timings.file_hash),
        chunk_hash_seconds,
        chunk_hash_bytes: timings.chunk_hash_bytes,
        chunk_hash_mib_per_second: mib_per_s(timings.chunk_hash_bytes, timings.chunk_hash),
        accounted_seconds,
        unaccounted_seconds,
        accounted_percent_of_total,
        total_seconds,
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
    print_table_row(
        "read files",
        0,
        Some(secs(t.scan_read)),
        Some(format!("{:.2} MiB", mib(t.scan_read_bytes))),
        Some(format!(
            "{:.2} MiB/s",
            mib_per_s(t.scan_read_bytes, t.scan_read)
        )),
    );
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
        "chunking",
        0,
        Some(secs(t.chunking)),
        Some(format!("{:.2} MiB", mib(t.chunking_bytes))),
        Some(format!(
            "{:.2} MiB/s",
            mib_per_s(t.chunking_bytes, t.chunking)
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
    print_table_row(
        "compression",
        0,
        Some(secs(t.compression)),
        Some(format!(
            "{:.2} / {:.2} MiB",
            mib(t.compression_input_bytes),
            mib(t.compression_output_bytes)
        )),
        Some(format!(
            "{:.2} MiB/s",
            mib_per_s(t.compression_input_bytes, t.compression)
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
    print_table_row(
        "bundle build",
        1,
        Some(secs(t.bundle_build)),
        Some(format!("out {:.2} MiB", mib(t.bundle_build_bytes))),
        Some(format!(
            "{:.2} MiB/s",
            mib_per_s(t.bundle_build_bytes, t.bundle_build)
        )),
    );
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
    print_table_row(
        "manifest finalize",
        0,
        Some(secs(t.target_manifest_finalize)),
        None,
        None,
    );
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
            ratio_pct(t.total_compressed_bytes, t.scan_read_bytes)
        ),
    );
    print_blank_line();
    print_section_title("Build Benchmark Snapshot");
    print_metric_line("total", format!("{:.2} s", secs(t.total)));
    print_metric_line("note", "phase timers overlap and are not additive");
}

pub fn print_reconstruct_timings(t: &g::ReconstructTimings) {
    print_section_title("Reconstruct Timing Summary");
    print_runtime_plan(t.runtime_plan.as_ref(), false);
    print_metric_line(
        "bundle read",
        format!(
            "{:.2} s  in {:.2} MiB  out {:.2} MiB  {:.2} MiB/s",
            secs(t.bundle_read),
            mib(t.bundle_read_compressed_bytes),
            mib(t.bundle_read_decompressed_bytes),
            mib_per_s(t.bundle_read_decompressed_bytes, t.bundle_read)
        ),
    );
    print_timing_throughput_line("file rebuild", t.file_rebuild, t.file_rebuild_bytes);
    print_timing_throughput_line("file verify", t.file_verify, t.file_verify_bytes);
    print_timing_throughput_line("file write", t.file_write, t.file_write_bytes);
    let accounted =
        secs(t.bundle_read) + secs(t.file_rebuild) + secs(t.file_verify) + secs(t.file_write);
    print_benchmark_snapshot("Reconstruct Benchmark Snapshot", t.total, accounted);
}

pub fn print_verify_timings(t: &g::VerifyTimings) {
    print_section_title("Verify Timing Summary");
    print_metric_line("mode", t.verify_mode.as_str());
    print_runtime_plan(t.runtime_plan.as_ref(), false);
    print_metric_line("stat", format!("{:.2} s", secs(t.stat)));
    print_timing_throughput_line("file read", t.file_read, t.file_read_bytes);
    print_timing_throughput_line("file hash", t.file_hash, t.file_hash_bytes);
    print_timing_throughput_line("chunk hash", t.chunk_hash, t.chunk_hash_bytes);
    let accounted = secs(t.stat) + secs(t.file_read) + secs(t.file_hash) + secs(t.chunk_hash);
    print_benchmark_snapshot("Verify Benchmark Snapshot", t.total, accounted);
}

pub fn print_repo_check_timings(t: &g::RepoCheckTimings) {
    print_section_title("Check Timing");
    print_metric_line("load manifest", format!("{} ms", t.load_manifest_ms));
    print_metric_line("scan bundles", format!("{} ms", t.scan_bundles_ms));
    print_metric_line("load chunk index", format!("{} ms", t.load_chunk_index_ms));
    print_metric_line(
        "load target manifests",
        format!(
            "{} ms  ({} targets)",
            t.load_target_manifests_ms, t.target_count
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
        "scan source installs",
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
