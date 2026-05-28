use std::collections::{HashMap, HashSet};
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

pub fn ratio_pct(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        (numerator as f64 * 100.0) / denominator as f64
    }
}

#[derive(Debug, Serialize)]
pub struct OptionalOutputPaths {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_out: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats_json_out: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BuildTimingsSummary {
    pub scan_read_seconds: f64,
    pub scan_read_bytes: u64,
    pub file_hash_seconds: f64,
    pub file_hash_bytes: u64,
    pub chunking_seconds: f64,
    pub chunking_bytes: u64,
    pub chunk_hash_judgment_seconds: f64,
    pub chunk_hash_judgment_bytes: u64,
    pub metadata_seconds: f64,
    pub dedupe_lookup_seconds: f64,
    pub dedupe_lookup_bytes: u64,
    pub compression_seconds: f64,
    pub compression_input_bytes: u64,
    pub compression_output_bytes: u64,
    pub total_compressed_bytes: u64,
    pub bundle_build_seconds: f64,
    pub bundle_build_bytes: u64,
    pub target_manifest_finalize_seconds: f64,
    pub total_seconds: f64,
}

#[derive(Debug, Serialize)]
pub struct ReconstructTimingsSummary {
    pub bundle_read_seconds: f64,
    pub bundle_read_compressed_bytes: u64,
    pub bundle_read_decompressed_bytes: u64,
    pub file_rebuild_seconds: f64,
    pub file_rebuild_bytes: u64,
    pub file_verify_seconds: f64,
    pub file_verify_bytes: u64,
    pub file_write_seconds: f64,
    pub file_write_bytes: u64,
    pub total_seconds: f64,
}

#[derive(Debug, Serialize)]
pub struct VerifyTimingsSummary {
    pub stat_seconds: f64,
    pub file_read_seconds: f64,
    pub file_read_bytes: u64,
    pub file_hash_seconds: f64,
    pub file_hash_bytes: u64,
    pub chunk_hash_seconds: f64,
    pub chunk_hash_bytes: u64,
    pub total_seconds: f64,
}

pub fn build_timings_summary(timings: &g::BuildTimings) -> BuildTimingsSummary {
    BuildTimingsSummary {
        scan_read_seconds: secs(timings.scan_read),
        scan_read_bytes: timings.scan_read_bytes,
        file_hash_seconds: secs(timings.file_hash),
        file_hash_bytes: timings.file_hash_bytes,
        chunking_seconds: secs(timings.chunking),
        chunking_bytes: timings.chunking_bytes,
        chunk_hash_judgment_seconds: secs(timings.chunk_hash_judgment),
        chunk_hash_judgment_bytes: timings.chunk_hash_judgment_bytes,
        metadata_seconds: secs(timings.metadata),
        dedupe_lookup_seconds: secs(timings.dedupe_lookup),
        dedupe_lookup_bytes: timings.dedupe_lookup_bytes,
        compression_seconds: secs(timings.compression),
        compression_input_bytes: timings.compression_input_bytes,
        compression_output_bytes: timings.compression_output_bytes,
        total_compressed_bytes: timings.total_compressed_bytes,
        bundle_build_seconds: secs(timings.bundle_build),
        bundle_build_bytes: timings.bundle_build_bytes,
        target_manifest_finalize_seconds: secs(timings.target_manifest_finalize),
        total_seconds: secs(timings.total),
    }
}

pub fn reconstruct_timings_summary(timings: &g::ReconstructTimings) -> ReconstructTimingsSummary {
    ReconstructTimingsSummary {
        bundle_read_seconds: secs(timings.bundle_read),
        bundle_read_compressed_bytes: timings.bundle_read_compressed_bytes,
        bundle_read_decompressed_bytes: timings.bundle_read_decompressed_bytes,
        file_rebuild_seconds: secs(timings.file_rebuild),
        file_rebuild_bytes: timings.file_rebuild_bytes,
        file_verify_seconds: secs(timings.file_verify),
        file_verify_bytes: timings.file_verify_bytes,
        file_write_seconds: secs(timings.file_write),
        file_write_bytes: timings.file_write_bytes,
        total_seconds: secs(timings.total),
    }
}

pub fn verify_timings_summary(timings: &g::VerifyTimings) -> VerifyTimingsSummary {
    VerifyTimingsSummary {
        stat_seconds: secs(timings.stat),
        file_read_seconds: secs(timings.file_read),
        file_read_bytes: timings.file_read_bytes,
        file_hash_seconds: secs(timings.file_hash),
        file_hash_bytes: timings.file_hash_bytes,
        chunk_hash_seconds: secs(timings.chunk_hash),
        chunk_hash_bytes: timings.chunk_hash_bytes,
        total_seconds: secs(timings.total),
    }
}

pub fn optional_output_paths(
    summary_out: &Option<PathBuf>,
    stats_json_out: &Option<PathBuf>,
) -> OptionalOutputPaths {
    OptionalOutputPaths {
        summary_out: summary_out.as_ref().map(|path| path.display().to_string()),
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

pub fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    const TIB: u64 = 1024 * GIB;

    if bytes >= TIB {
        format!("{:.2} TiB", bytes as f64 / TIB as f64)
    } else if bytes >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.2} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.2} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn directory_size_bytes(root_path: &std::path::Path) -> g::Result<u64> {
    if !root_path.exists() {
        return Ok(0);
    }

    let mut total_bytes = 0u64;
    for entry in walkdir::WalkDir::new(root_path).sort_by_file_name() {
        let entry = entry?;
        if entry.path().is_file() {
            total_bytes += entry.metadata()?.len();
        }
    }

    Ok(total_bytes)
}

pub fn collect_repo_init_target_added_bytes<'a>(
    ordered_target_manifests: &[&'a g::manifest::target::TargetManifest],
    target_manifest_sizes: &HashMap<String, u64>,
    _bundle_sizes: &HashMap<String, u64>,
) -> Vec<(&'a g::manifest::target::TargetManifest, u64)> {
    let mut _seen_bundle_ids: HashSet<String> = HashSet::new();
    let mut added_bytes_by_target = Vec::with_capacity(ordered_target_manifests.len());

    for target_manifest in ordered_target_manifests {
        let manifest_bytes = target_manifest_sizes
            .get(&target_manifest.target_id)
            .copied()
            .unwrap_or(0);
        let new_bundle_bytes: u64 = 0; // bundle size tracking requires chunk_index.json
        added_bytes_by_target.push((*target_manifest, manifest_bytes + new_bundle_bytes));
    }

    added_bytes_by_target
}

pub fn print_repo_init_storage_summary(
    ordered_target_manifests: &[&g::manifest::target::TargetManifest],
    repo_root: &std::path::Path,
    target_manifest_sizes: &HashMap<String, u64>,
    bundle_sizes: &HashMap<String, u64>,
) -> g::Result<()> {
    let total_install_bytes: u64 = ordered_target_manifests
        .iter()
        .map(|target_manifest| target_manifest.total_logical_size)
        .sum();
    let repo_size_bytes = directory_size_bytes(repo_root)?;
    let repo_percent_of_installs = ratio_pct(repo_size_bytes, total_install_bytes);
    let saved_percent_vs_full_installs = if total_install_bytes == 0 {
        0.0
    } else {
        100.0 - repo_percent_of_installs
    };
    let added_bytes_by_target = collect_repo_init_target_added_bytes(
        ordered_target_manifests,
        target_manifest_sizes,
        bundle_sizes,
    );

    println!("storage summary:");
    println!(
        "  total install size: {}",
        format_bytes(total_install_bytes)
    );
    println!("  repo size:          {}", format_bytes(repo_size_bytes));
    println!("  repo vs installs:   {:.2}%", repo_percent_of_installs);
    println!(
        "  saved vs full copy: {:.2}%",
        saved_percent_vs_full_installs
    );
    println!("  per-target added repo bytes:");
    for (target_manifest, added_bytes) in added_bytes_by_target {
        println!(
            "    {}: added {} for install {}",
            target_manifest.target_id,
            format_bytes(added_bytes),
            format_bytes(target_manifest.total_logical_size)
        );
    }

    Ok(())
}

pub fn print_build_timings(t: &g::BuildTimings) {
    println!("timings:");
    println!(
        "  read files:        {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.scan_read),
        mib(t.scan_read_bytes),
        mib_per_s(t.scan_read_bytes, t.scan_read)
    );
    println!(
        "  file hashing:      {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.file_hash),
        mib(t.file_hash_bytes),
        mib_per_s(t.file_hash_bytes, t.file_hash)
    );
    println!(
        "  chunking:          {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.chunking),
        mib(t.chunking_bytes),
        mib_per_s(t.chunking_bytes, t.chunking)
    );
    println!(
        "  hash judgment:     {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.chunk_hash_judgment),
        mib(t.chunk_hash_judgment_bytes),
        mib_per_s(t.chunk_hash_judgment_bytes, t.chunk_hash_judgment)
    );
    println!("  metadata:          {:.2} s", secs(t.metadata));
    println!(
        "  dedupe lookup:     {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.dedupe_lookup),
        mib(t.dedupe_lookup_bytes),
        mib_per_s(t.dedupe_lookup_bytes, t.dedupe_lookup)
    );
    println!(
        "  compression:       {:.2} s  in {:.2} MiB  out {:.2} MiB  {:.2} MiB/s",
        secs(t.compression),
        mib(t.compression_input_bytes),
        mib(t.compression_output_bytes),
        mib_per_s(t.compression_input_bytes, t.compression)
    );
    println!(
        "  total dedupe ratio: {:.2}%  (out / in)",
        ratio_pct(t.total_compressed_bytes, t.dedupe_lookup_bytes)
    );
    println!(
        "  bundle build:      {:.2} s  out {:.2} MiB  {:.2} MiB/s",
        secs(t.bundle_build),
        mib(t.bundle_build_bytes),
        mib_per_s(t.bundle_build_bytes, t.bundle_build)
    );
    println!(
        "  target manifest finalize: {:.2} s",
        secs(t.target_manifest_finalize)
    );
    println!("  total:             {:.2} s", secs(t.total));
}

pub fn print_reconstruct_timings(t: &g::ReconstructTimings) {
    println!("timings:");
    println!(
        "  bundle read:       {:.2} s  in {:.2} MiB  out {:.2} MiB  {:.2} MiB/s",
        secs(t.bundle_read),
        mib(t.bundle_read_compressed_bytes),
        mib(t.bundle_read_decompressed_bytes),
        mib_per_s(t.bundle_read_decompressed_bytes, t.bundle_read)
    );
    println!(
        "  file rebuild:      {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.file_rebuild),
        mib(t.file_rebuild_bytes),
        mib_per_s(t.file_rebuild_bytes, t.file_rebuild)
    );
    println!(
        "  file verify:       {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.file_verify),
        mib(t.file_verify_bytes),
        mib_per_s(t.file_verify_bytes, t.file_verify)
    );
    println!(
        "  file write:        {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.file_write),
        mib(t.file_write_bytes),
        mib_per_s(t.file_write_bytes, t.file_write)
    );
    println!("  total:             {:.2} s", secs(t.total));
}

pub fn print_verify_timings(t: &g::VerifyTimings) {
    println!("timings:");
    println!("  stat:              {:.2} s", secs(t.stat));
    println!(
        "  file read:         {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.file_read),
        mib(t.file_read_bytes),
        mib_per_s(t.file_read_bytes, t.file_read)
    );
    println!(
        "  file hash:         {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.file_hash),
        mib(t.file_hash_bytes),
        mib_per_s(t.file_hash_bytes, t.file_hash)
    );
    println!(
        "  chunk hash:        {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.chunk_hash),
        mib(t.chunk_hash_bytes),
        mib_per_s(t.chunk_hash_bytes, t.chunk_hash)
    );
    println!("  total:             {:.2} s", secs(t.total));
}

#[cfg(test)]
#[path = "cli_reporting_tests.rs"]
mod tests;

pub fn accumulate_build_timings(total: &mut g::BuildTimings, add: &g::BuildTimings) {
    total.scan_read += add.scan_read;
    total.scan_read_bytes += add.scan_read_bytes;
    total.file_hash += add.file_hash;
    total.file_hash_bytes += add.file_hash_bytes;
    total.chunking += add.chunking;
    total.chunking_bytes += add.chunking_bytes;
    total.chunk_hash_judgment += add.chunk_hash_judgment;
    total.chunk_hash_judgment_bytes += add.chunk_hash_judgment_bytes;
    total.metadata += add.metadata;
    total.dedupe_lookup += add.dedupe_lookup;
    total.dedupe_lookup_bytes += add.dedupe_lookup_bytes;
    total.compression += add.compression;
    total.compression_input_bytes += add.compression_input_bytes;
    total.compression_output_bytes += add.compression_output_bytes;
    total.total_compressed_bytes += add.total_compressed_bytes;
    total.bundle_build += add.bundle_build;
    total.bundle_build_bytes += add.bundle_build_bytes;
    total.target_manifest_finalize += add.target_manifest_finalize;
    total.total += add.total;
}
