use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::Serialize;
use tempfile::TempDir;

use crate::cli::path_compare::PathCompareStats;
use crate::cli::reporting::{
    BuildTimingsSummary, OptionalOutputPaths, ReconstructTimingsSummary, VerifyTimingsSummary,
    ensure_parent_directory, format_bytes, write_stats_json_output,
};
use crate::cli::verbose_output_enabled;
use g::repo::{required_bundle_ids as resolve_required_bundle_ids, required_chunk_hashes};
use gutch as g;

pub(crate) struct InputSizingStats {
    pub(crate) target_logical_size_bytes: u64,
    pub(crate) median_file_size_bytes: u64,
}

pub(crate) struct PreparedInputPath {
    pub(crate) dir: PathBuf,
    pub(crate) _tempdir: Option<TempDir>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TargetInsertStats {
    pub(crate) command: &'static str,
    pub(crate) repo_root: String,
    pub(crate) input_path: String,
    pub(crate) target_id: String,
    pub(crate) manifest_path: String,
    pub(crate) product_id: String,
    pub(crate) content_repo_id: String,
    pub(crate) version_id: String,
    pub(crate) platform: String,
    pub(crate) architecture: String,
    pub(crate) tier_id: String,
    pub(crate) file_count: usize,
    pub(crate) total_logical_size: u64,
    pub(crate) total_compressed_size_estimate: u64,
    pub(crate) required_chunk_count: usize,
    pub(crate) required_bundle_count: usize,
    pub(crate) generated_bundle_count: usize,
    pub(crate) generated_bundle_bytes: u64,
    pub(crate) reused_chunk_count: usize,
    pub(crate) new_chunk_count: usize,
    pub(crate) reused_chunk_percent: f64,
    pub(crate) reused_bundle_count: usize,
    pub(crate) reused_bundle_percent: f64,
    pub(crate) timings: BuildTimingsSummary,
    pub(crate) outputs: OptionalOutputPaths,
}

#[derive(Debug, Serialize)]
pub(crate) struct TargetExportStats {
    pub(crate) command: &'static str,
    pub(crate) repo_root: String,
    pub(crate) target_id: String,
    pub(crate) output_dir: String,
    pub(crate) file_count: usize,
    pub(crate) required_chunk_count: usize,
    pub(crate) required_bundle_count: usize,
    pub(crate) resolved_bundle_file_count: usize,
    pub(crate) total_logical_size: u64,
    pub(crate) total_compressed_size_estimate: u64,
    pub(crate) timings: ReconstructTimingsSummary,
    pub(crate) outputs: OptionalOutputPaths,
}

#[derive(Debug, Serialize)]
pub(crate) struct TargetInstallVerifyStats {
    pub(crate) command: &'static str,
    pub(crate) repo_root: String,
    pub(crate) target_id: String,
    pub(crate) install_dir: String,
    pub(crate) expected_file_count: usize,
    pub(crate) missing_file_count: usize,
    pub(crate) corrupt_file_count: usize,
    pub(crate) extra_file_count: usize,
    pub(crate) verification_passed: bool,
    pub(crate) errors: Vec<String>,
    pub(crate) timings: VerifyTimingsSummary,
    pub(crate) outputs: OptionalOutputPaths,
}

#[derive(Debug, Serialize)]
pub(crate) struct TargetInstallApplyStats {
    pub(crate) command: &'static str,
    pub(crate) repo_root: String,
    pub(crate) target_id: String,
    pub(crate) install_dir: String,
    pub(crate) staging_dir: String,
    pub(crate) source_target_id: Option<String>,
    pub(crate) source_install_dir: Option<String>,
    pub(crate) file_count: usize,
    pub(crate) required_chunk_count: usize,
    pub(crate) required_bundle_count: usize,
    pub(crate) resolved_bundle_file_count: usize,
    pub(crate) reused_source_chunk_count: usize,
    pub(crate) reused_source_chunk_percent: f64,
    pub(crate) total_logical_size: u64,
    pub(crate) total_compressed_size_estimate: u64,
    pub(crate) reconstruct_timings: ReconstructTimingsSummary,
    pub(crate) verify_timings: VerifyTimingsSummary,
    pub(crate) outputs: OptionalOutputPaths,
}

pub(crate) trait SummaryReport {
    fn summary_lines(&self) -> Vec<String>;
}

pub(crate) fn write_optional_summary(
    summary_out: &Option<PathBuf>,
    lines: &[String],
) -> g::Result<()> {
    let Some(summary_out) = summary_out else {
        return Ok(());
    };

    ensure_parent_directory(summary_out)?;
    std::fs::write(summary_out, lines.join("\n"))?;
    Ok(())
}

pub(crate) fn write_report<T: Serialize + SummaryReport>(
    summary_out: &Option<PathBuf>,
    stats_json_out: &Option<PathBuf>,
    stats: &T,
) -> g::Result<()> {
    write_optional_summary(summary_out, &stats.summary_lines())?;
    write_stats_json_output(stats_json_out, stats)
}

fn summary_title(title: &str) -> String {
    format!("== {} ==", title)
}

fn summary_kv(label: &str, value: impl Into<String>) -> String {
    format!("{label:<20} {}", value.into())
}

fn summary_blank_line(lines: &mut Vec<String>) {
    if lines.last().is_some_and(|line| !line.is_empty()) {
        lines.push(String::new());
    }
}

fn push_summary_section(
    lines: &mut Vec<String>,
    title: &str,
    entries: impl IntoIterator<Item = String>,
) {
    summary_blank_line(lines);
    lines.push(summary_title(title));
    lines.extend(entries);
}

fn status_label(passed: bool, success: &'static str, failure: &'static str) -> &'static str {
    if passed { success } else { failure }
}

fn extend_summary_section(summary_lines: &mut Vec<String>, header: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }

    summary_blank_line(summary_lines);
    summary_lines.push(header.to_string());
    summary_lines.extend(values.iter().map(|value| format!("  {value}")));
}

impl SummaryReport for TargetInsertStats {
    fn summary_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        push_summary_section(
            &mut lines,
            "Command",
            [
                summary_kv("command", self.command),
                summary_kv("repo_root", &self.repo_root),
                summary_kv("input_path", &self.input_path),
                summary_kv("target_id", &self.target_id),
                summary_kv("manifest_path", &self.manifest_path),
            ],
        );
        push_summary_section(
            &mut lines,
            "Identity",
            [
                summary_kv("content_repo", &self.content_repo_id),
                summary_kv("version", &self.version_id),
                summary_kv("platform", &self.platform),
                summary_kv("architecture", &self.architecture),
                summary_kv("tier", &self.tier_id),
            ],
        );
        push_summary_section(
            &mut lines,
            "Content",
            [
                summary_kv("files", self.file_count.to_string()),
                summary_kv("logical_size", format_bytes(self.total_logical_size)),
                summary_kv(
                    "compressed_est",
                    format_bytes(self.total_compressed_size_estimate),
                ),
                summary_kv("required_chunks", self.required_chunk_count.to_string()),
                summary_kv("new_chunks", self.new_chunk_count.to_string()),
                summary_kv("reused_chunks", self.reused_chunk_count.to_string()),
                summary_kv("chunk_reuse", format!("{:.2}%", self.reused_chunk_percent)),
                summary_kv("required_bundles", self.required_bundle_count.to_string()),
                summary_kv("generated_bundles", self.generated_bundle_count.to_string()),
                summary_kv("reused_bundles", self.reused_bundle_count.to_string()),
                summary_kv("generated_size", format_bytes(self.generated_bundle_bytes)),
                summary_kv("bundle_reuse", format!("{:.2}%", self.reused_bundle_percent)),
            ],
        );
        push_summary_section(
            &mut lines,
            "Timing",
            [summary_kv(
                "total_seconds",
                format!("{:.2}", self.timings.total_seconds),
            )],
        );
        lines
    }
}

impl SummaryReport for TargetExportStats {
    fn summary_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        push_summary_section(
            &mut lines,
            "Command",
            [
                summary_kv("command", self.command),
                summary_kv("repo_root", &self.repo_root),
                summary_kv("target_id", &self.target_id),
                summary_kv("output_dir", &self.output_dir),
            ],
        );
        push_summary_section(
            &mut lines,
            "Content",
            [
                summary_kv("files", self.file_count.to_string()),
                summary_kv("logical_size", format_bytes(self.total_logical_size)),
                summary_kv(
                    "compressed_est",
                    format_bytes(self.total_compressed_size_estimate),
                ),
                summary_kv("required_chunks", self.required_chunk_count.to_string()),
                summary_kv("required_bundles", self.required_bundle_count.to_string()),
                summary_kv("resolved_bundles", self.resolved_bundle_file_count.to_string()),
            ],
        );
        push_summary_section(
            &mut lines,
            "Timing",
            [summary_kv(
                "total_seconds",
                format!("{:.2}", self.timings.total_seconds),
            )],
        );
        lines
    }
}

impl SummaryReport for TargetInstallApplyStats {
    fn summary_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        push_summary_section(
            &mut lines,
            "Command",
            [
                summary_kv("command", self.command),
                summary_kv("repo_root", &self.repo_root),
                summary_kv("target_id", &self.target_id),
                summary_kv("install_dir", &self.install_dir),
                summary_kv("staging_dir", &self.staging_dir),
            ],
        );
        push_summary_section(
            &mut lines,
            "Source Reuse",
            [
                summary_kv(
                    "source_target",
                    self.source_target_id.as_deref().unwrap_or("none"),
                ),
                summary_kv(
                    "source_dir",
                    self.source_install_dir.as_deref().unwrap_or("none"),
                ),
                summary_kv("reused_chunks", self.reused_source_chunk_count.to_string()),
                summary_kv(
                    "reuse_percent",
                    format!("{:.2}%", self.reused_source_chunk_percent),
                ),
            ],
        );
        push_summary_section(
            &mut lines,
            "Content",
            [
                summary_kv("files", self.file_count.to_string()),
                summary_kv("logical_size", format_bytes(self.total_logical_size)),
                summary_kv(
                    "compressed_est",
                    format_bytes(self.total_compressed_size_estimate),
                ),
                summary_kv("required_chunks", self.required_chunk_count.to_string()),
                summary_kv("required_bundles", self.required_bundle_count.to_string()),
                summary_kv("resolved_bundles", self.resolved_bundle_file_count.to_string()),
            ],
        );
        push_summary_section(
            &mut lines,
            "Timing",
            [
                summary_kv(
                    "reconstruct_sec",
                    format!("{:.2}", self.reconstruct_timings.total_seconds),
                ),
                summary_kv(
                    "verify_sec",
                    format!("{:.2}", self.verify_timings.total_seconds),
                ),
            ],
        );
        lines
    }
}

impl SummaryReport for TargetInstallVerifyStats {
    fn summary_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        push_summary_section(
            &mut lines,
            "Command",
            [
                summary_kv("command", self.command),
                summary_kv("repo_root", &self.repo_root),
                summary_kv("target_id", &self.target_id),
                summary_kv("install_dir", &self.install_dir),
                summary_kv(
                    "status",
                    status_label(self.verification_passed, "passed", "failed"),
                ),
            ],
        );
        push_summary_section(
            &mut lines,
            "Results",
            [
                summary_kv("expected_files", self.expected_file_count.to_string()),
                summary_kv("missing", self.missing_file_count.to_string()),
                summary_kv("corrupt", self.corrupt_file_count.to_string()),
                summary_kv("extra", self.extra_file_count.to_string()),
            ],
        );
        push_summary_section(
            &mut lines,
            "Timing",
            [summary_kv(
                "total_seconds",
                format!("{:.2}", self.timings.total_seconds),
            )],
        );
        lines
    }
}

impl SummaryReport for PathCompareStats {
    fn summary_lines(&self) -> Vec<String> {
        let mut summary_lines = Vec::new();
        push_summary_section(
            &mut summary_lines,
            "Command",
            [
                summary_kv("command", self.command),
                summary_kv("expected_path", &self.expected_path),
                summary_kv("actual_path", &self.actual_path),
                summary_kv(
                    "status",
                    status_label(self.compare_passed, "match", "failed"),
                ),
            ],
        );
        push_summary_section(
            &mut summary_lines,
            "Results",
            [
                summary_kv("expected_files", self.expected_file_count.to_string()),
                summary_kv("actual_files", self.actual_file_count.to_string()),
                summary_kv("matched", self.matched_file_count.to_string()),
                summary_kv("mismatched", self.mismatched_file_count.to_string()),
                summary_kv("missing", self.missing_file_count.to_string()),
                summary_kv("extra", self.extra_file_count.to_string()),
            ],
        );
        extend_summary_section(&mut summary_lines, "[mismatched]", &self.mismatched_files);
        extend_summary_section(&mut summary_lines, "[missing]", &self.missing_files);
        extend_summary_section(&mut summary_lines, "[extra]", &self.extra_files);
        summary_lines
    }
}

pub(crate) fn median_u64(sorted_values: &[u64]) -> u64 {
    if sorted_values.is_empty() {
        return 0;
    }

    let mid = sorted_values.len() / 2;
    if sorted_values.len() % 2 == 1 {
        sorted_values[mid]
    } else {
        (sorted_values[mid - 1] + sorted_values[mid]) / 2
    }
}

pub(crate) fn collect_input_sizing_stats(dir: &PathBuf) -> g::Result<InputSizingStats> {
    let mut sizes = Vec::new();
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry?;
        if entry.path().is_file() {
            sizes.push(entry.metadata()?.len());
        }
    }

    sizes.sort_unstable();
    let target_logical_size_bytes = sizes.iter().copied().sum();
    Ok(InputSizingStats {
        target_logical_size_bytes,
        median_file_size_bytes: median_u64(&sizes),
    })
}

pub(crate) fn prepare_input_path(path: &PathBuf) -> g::Result<PreparedInputPath> {
    if path.is_dir() {
        return Ok(PreparedInputPath {
            dir: path.clone(),
            _tempdir: None,
        });
    }

    if path.is_file() {
        let tempdir = tempfile::Builder::new().prefix("gutch_input_").tempdir()?;
        let filename = path.file_name().ok_or_else(|| {
            g::Error::manifest(format!("input file has no name: {}", path.display()))
        })?;
        let staged_path = tempdir.path().join(filename);
        std::fs::copy(path, &staged_path)?;
        return Ok(PreparedInputPath {
            dir: tempdir.path().to_path_buf(),
            _tempdir: Some(tempdir),
        });
    }

    Err(g::Error::manifest(format!(
        "input path does not exist or is unsupported: {}",
        path.display()
    )))
}

pub(crate) fn load_optional_rules(
    rules_path: &Option<PathBuf>,
) -> g::Result<Option<g::manifest::AutoTaggingRules>> {
    let Some(rules_path) = rules_path else {
        return Ok(None);
    };

    let rules_json = std::fs::read_to_string(rules_path)?;
    let rules: g::manifest::AutoTaggingRules = serde_json::from_str(&rules_json)?;
    Ok(Some(rules))
}

pub(crate) fn load_target_manifest_from_repo(
    repo_root: &std::path::Path,
    target_id: &str,
) -> g::Result<(
    g::ContentRepoPaths,
    g::manifest::content_repo::ContentRepoManifest,
    PathBuf,
    g::manifest::target::TargetManifest,
)> {
    let (repo_paths, repo_manifest) = g::load_content_repo_manifest(repo_root)?;
    let manifest_path = g::target_manifest_path(&repo_paths, target_id);
    if !manifest_path.exists() {
        return Err(g::Error::manifest(format!(
            "target manifest not found at {}",
            manifest_path.display()
        )));
    }

    let manifest_json = std::fs::read_to_string(&manifest_path)?;
    let target_manifest: g::manifest::target::TargetManifest =
        serde_json::from_str(&manifest_json)?;
    Ok((repo_paths, repo_manifest, manifest_path, target_manifest))
}

pub(crate) fn count_required_chunks(
    target_manifest: &g::manifest::target::TargetManifest,
) -> usize {
    required_chunk_hashes(target_manifest).len()
}

pub(crate) fn count_required_bundles_from_chunk_index(
    target_manifest: &g::manifest::target::TargetManifest,
    chunk_index: &g::manifest::ChunkIndex,
) -> usize {
    resolve_required_bundle_ids(target_manifest, chunk_index).len()
}

pub(crate) fn count_required_bundles_for_build(
    target_manifest: &g::manifest::target::TargetManifest,
    existing_chunk_locations: &HashMap<String, g::ExistingChunkLocation>,
    generated_bundles: &[g::BuiltBundle],
) -> usize {
    let mut required_bundle_ids = HashSet::new();
    for chunk_hash in required_chunk_hashes(target_manifest) {
        if let Some(existing) = existing_chunk_locations.get(&chunk_hash) {
            required_bundle_ids.insert(existing.bundle_id.clone());
        }
    }

    required_bundle_ids.len() + generated_bundles.len()
}

pub(crate) fn resolve_required_bundle_paths(
    bundles_root: &std::path::Path,
) -> g::Result<Vec<PathBuf>> {
    if bundles_root.is_file() {
        return Ok(vec![bundles_root.to_path_buf()]);
    }

    if !bundles_root.is_dir() {
        return Ok(Vec::new());
    }

    let mut paths = Vec::new();
    for entry in walkdir::WalkDir::new(bundles_root).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file() {
            continue;
        }
        if !entry.path().extension().is_some_and(|e| e == "bundle") {
            continue;
        }
        paths.push(entry.path().to_path_buf());
    }

    Ok(paths)
}

pub(crate) fn default_staging_dir(install_dir: &std::path::Path, target_id: &str) -> PathBuf {
    let install_name = install_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("install");
    install_dir.with_file_name(format!(".gutch-staging-{}-{}", install_name, target_id))
}

pub(crate) fn parse_bundle_downloads(
    bundle_args: &[String],
) -> g::Result<Vec<g::GutchBundleDownload>> {
    bundle_args
        .iter()
        .map(|bundle_arg| {
            let Some((bundle_id, url)) = bundle_arg.split_once('=') else {
                return Err(g::Error::manifest(format!(
                    "invalid --bundle value '{}'; use <bundle-id>=<url>",
                    bundle_arg
                )));
            };
            let bundle_id = bundle_id.trim();
            let url = url.trim();
            if bundle_id.is_empty() || url.is_empty() {
                return Err(g::Error::manifest(format!(
                    "invalid --bundle value '{}'; use <bundle-id>=<url>",
                    bundle_arg
                )));
            }

            Ok(g::GutchBundleDownload {
                bundle_id: bundle_id.to_string(),
                url: url.to_string(),
            })
        })
        .collect()
}

pub(crate) fn session_id_for_cli(session_override: Option<String>) -> g::Result<String> {
    session_override
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or(crate::auth::shared_session_id().map_err(g::Error::manifest)?)
        .ok_or_else(|| {
            g::Error::manifest(
                "authentication required. Run `gutch auth login` or pass --session-id.",
            )
        })
}

pub(crate) fn parse_version_parts(version: &str) -> g::Result<(i32, i32, i32)> {
    let normalized = version
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V');
    let parts = normalized.split('.').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(g::Error::manifest(format!(
            "expected semantic version like 1.2.3, got '{}'",
            version
        )));
    }

    let major = parts[0]
        .parse::<i32>()
        .map_err(|_| g::Error::manifest(format!("invalid version '{}'", version)))?;
    let minor = parts[1]
        .parse::<i32>()
        .map_err(|_| g::Error::manifest(format!("invalid version '{}'", version)))?;
    let patch = parts[2]
        .parse::<i32>()
        .map_err(|_| g::Error::manifest(format!("invalid version '{}'", version)))?;
    Ok((major, minor, patch))
}

pub(crate) fn write_target_manifest(
    target_root: &std::path::Path,
    target_manifest: &g::manifest::target::TargetManifest,
) -> g::Result<(PathBuf, u64)> {
    std::fs::create_dir_all(target_root)?;
    let manifest_path = target_root.join("manifest.json");
    let manifest_json = serde_json::to_string_pretty(target_manifest)?;
    std::fs::write(&manifest_path, &manifest_json)?;
    if verbose_output_enabled() {
        println!("wrote target manifest: {}", manifest_path.display());
    }
    let manifest_size = std::fs::metadata(&manifest_path)?.len();
    Ok((manifest_path, manifest_size))
}

pub(crate) fn write_bundles(
    bundles_root: &std::path::Path,
    bundles: &[g::BuiltBundle],
) -> g::Result<std::collections::HashMap<String, u64>> {
    let mut bundle_sizes = std::collections::HashMap::with_capacity(bundles.len());
    for bundle in bundles {
        let bundle_path = bundles_root.join(format!("{}.bundle", bundle.bundle_id));
        std::fs::write(&bundle_path, &bundle.data)?;
        let bundle_size = bundle.data.len() as u64;
        bundle_sizes.insert(bundle.bundle_id.clone(), bundle_size);
        if verbose_output_enabled() {
            println!(
                "wrote bundle: {} ({} bytes)",
                bundle_path.display(),
                bundle.data.len()
            );
        }
    }
    Ok(bundle_sizes)
}
