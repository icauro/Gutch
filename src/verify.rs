use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;

use crate::Result;
use crate::hash::hash_hex;
use crate::manifest::{self, target::TargetManifest};
use crate::path_guard::resolve_manifest_relative_path;
use crate::runtime::{
    RuntimePlan, RuntimeResourceHints, install_in_thread_pool, plan_verify_runtime,
};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VerifyMode {
    #[default]
    Fast,
    Strict,
}

impl VerifyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            VerifyMode::Fast => "fast",
            VerifyMode::Strict => "strict",
        }
    }

    fn checks_chunk_hashes(self) -> bool {
        matches!(self, VerifyMode::Strict)
    }
}

#[derive(Debug, Clone, Default)]
pub struct VerifyTimings {
    pub verify_mode: VerifyMode,
    pub runtime_plan: Option<RuntimePlan>,
    pub stat: Duration,
    pub file_read: Duration,
    pub file_read_bytes: u64,
    pub file_hash: Duration,
    pub file_hash_bytes: u64,
    pub chunk_hash: Duration,
    pub chunk_hash_bytes: u64,
    pub total: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairIssueKind {
    Missing,
    Corrupt,
    Extra,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairIssue {
    pub kind: RepairIssueKind,
    pub path: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct RepairClassification {
    pub missing_files: Vec<RepairIssue>,
    pub corrupt_files: Vec<RepairIssue>,
    pub extra_files: Vec<RepairIssue>,
}

impl RepairClassification {
    fn push(&mut self, issue: RepairIssue) {
        match issue.kind {
            RepairIssueKind::Missing => self.missing_files.push(issue),
            RepairIssueKind::Corrupt => self.corrupt_files.push(issue),
            RepairIssueKind::Extra => self.extra_files.push(issue),
        }
    }

    pub fn into_messages(self) -> Vec<String> {
        self.missing_files
            .into_iter()
            .chain(self.corrupt_files)
            .chain(self.extra_files)
            .map(|issue| issue.detail)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

struct VerifyOptions<'a> {
    mode: VerifyMode,
    hints: &'a RuntimeResourceHints,
    progress: Option<&'a ProgressBar>,
}

impl<'a> Default for VerifyOptions<'a> {
    fn default() -> Self {
        Self {
            mode: VerifyMode::Fast,
            hints: default_runtime_hints(),
            progress: None,
        }
    }
}

impl<'a> VerifyOptions<'a> {
    fn with_mode(mut self, mode: VerifyMode) -> Self {
        self.mode = mode;
        self
    }

    fn with_hints(mut self, hints: &'a RuntimeResourceHints) -> Self {
        self.hints = hints;
        self
    }

    fn with_progress(mut self, progress: &'a ProgressBar) -> Self {
        self.progress = Some(progress);
        self
    }
}

// ---------------------------------------------------------------------------
// Core implementation
// ---------------------------------------------------------------------------

fn classify_inner(
    target_manifest: &TargetManifest,
    dir: &Path,
    opts: &VerifyOptions,
) -> Result<(RepairClassification, VerifyTimings)> {
    let total_start = Instant::now();
    let mut classification = RepairClassification::default();
    let runtime_plan = plan_verify_runtime(opts.hints);
    let mut timings = VerifyTimings {
        verify_mode: opts.mode,
        runtime_plan: Some(runtime_plan.clone()),
        ..VerifyTimings::default()
    };
    let manifest_paths: HashSet<&str> = target_manifest
        .files
        .iter()
        .map(|fm| fm.path.as_str())
        .collect();

    let per_file_results = install_in_thread_pool(runtime_plan.worker_count, || {
        target_manifest
            .files
            .par_iter()
            .map(|fm| {
                let result = verify_manifest_file(target_manifest, dir, fm, opts.mode);
                if let Some(pb) = opts.progress {
                    pb.inc(1);
                }
                result
            })
            .collect::<Vec<_>>()
    })?;

    for result in per_file_results {
        let (issues, file_timings) = result?;
        accumulate_timings(&mut timings, &file_timings);
        for issue in issues {
            classification.push(issue);
        }
    }

    // Detect extra files not in manifest
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file() {
            continue;
        }
        let Ok(relative_path) = entry.path().strip_prefix(dir) else {
            continue;
        };
        let relative = relative_path.to_string_lossy().replace('\\', "/");
        if manifest_paths.contains(relative.as_str()) {
            continue;
        }
        classification.push(RepairIssue {
            kind: RepairIssueKind::Extra,
            path: relative.clone(),
            detail: format!("extra file: {}", relative),
        });
    }

    timings.total = total_start.elapsed();
    Ok((classification, timings))
}

fn verify_inner(
    target_manifest: &TargetManifest,
    dir: &Path,
    opts: &VerifyOptions,
) -> Result<(Vec<String>, VerifyTimings)> {
    let (classification, timings) = classify_inner(target_manifest, dir, opts)?;
    Ok((classification.into_messages(), timings))
}

// ---------------------------------------------------------------------------
// Public API — verify (returns error messages)
// ---------------------------------------------------------------------------

pub fn verify_directory(target_manifest: &TargetManifest, dir: &Path) -> Result<Vec<String>> {
    Ok(verify_inner(target_manifest, dir, &VerifyOptions::default())?.0)
}

pub fn verify_directory_strict(
    target_manifest: &TargetManifest,
    dir: &Path,
) -> Result<Vec<String>> {
    Ok(verify_inner(
        target_manifest,
        dir,
        &VerifyOptions::default().with_mode(VerifyMode::Strict),
    )?
    .0)
}

pub fn verify_directory_with_timings(
    target_manifest: &TargetManifest,
    dir: &Path,
) -> Result<(Vec<String>, VerifyTimings)> {
    verify_inner(target_manifest, dir, &VerifyOptions::default())
}

pub fn verify_directory_with_timings_and_mode(
    target_manifest: &TargetManifest,
    dir: &Path,
    verify_mode: VerifyMode,
) -> Result<(Vec<String>, VerifyTimings)> {
    verify_inner(
        target_manifest,
        dir,
        &VerifyOptions::default().with_mode(verify_mode),
    )
}

pub fn verify_directory_with_timings_and_hints(
    target_manifest: &TargetManifest,
    dir: &Path,
    runtime_hints: &RuntimeResourceHints,
) -> Result<(Vec<String>, VerifyTimings)> {
    verify_inner(
        target_manifest,
        dir,
        &VerifyOptions::default().with_hints(runtime_hints),
    )
}

pub fn verify_directory_with_timings_and_hints_and_mode(
    target_manifest: &TargetManifest,
    dir: &Path,
    runtime_hints: &RuntimeResourceHints,
    verify_mode: VerifyMode,
) -> Result<(Vec<String>, VerifyTimings)> {
    verify_inner(
        target_manifest,
        dir,
        &VerifyOptions::default()
            .with_mode(verify_mode)
            .with_hints(runtime_hints),
    )
}

pub fn verify_directory_with_progress(
    target_manifest: &TargetManifest,
    dir: &Path,
    runtime_hints: &RuntimeResourceHints,
) -> Result<(Vec<String>, VerifyTimings)> {
    verify_with_progress_bar(target_manifest, dir, runtime_hints, VerifyMode::Fast)
}

pub fn verify_directory_with_progress_and_mode(
    target_manifest: &TargetManifest,
    dir: &Path,
    runtime_hints: &RuntimeResourceHints,
    verify_mode: VerifyMode,
) -> Result<(Vec<String>, VerifyTimings)> {
    verify_with_progress_bar(target_manifest, dir, runtime_hints, verify_mode)
}

fn verify_with_progress_bar(
    target_manifest: &TargetManifest,
    dir: &Path,
    runtime_hints: &RuntimeResourceHints,
    verify_mode: VerifyMode,
) -> Result<(Vec<String>, VerifyTimings)> {
    run_with_progress_bar(target_manifest, |progress| {
        verify_inner(
            target_manifest,
            dir,
            &VerifyOptions::default()
                .with_mode(verify_mode)
                .with_hints(runtime_hints)
                .with_progress(progress),
        )
    })
}

// ---------------------------------------------------------------------------
// Public API — classify (returns RepairClassification)
// ---------------------------------------------------------------------------

pub fn classify_directory(
    target_manifest: &TargetManifest,
    dir: &Path,
) -> Result<RepairClassification> {
    Ok(classify_inner(target_manifest, dir, &VerifyOptions::default())?.0)
}

pub fn classify_directory_with_timings(
    target_manifest: &TargetManifest,
    dir: &Path,
) -> Result<(RepairClassification, VerifyTimings)> {
    classify_inner(target_manifest, dir, &VerifyOptions::default())
}

pub fn classify_directory_with_timings_and_hints(
    target_manifest: &TargetManifest,
    dir: &Path,
    runtime_hints: &RuntimeResourceHints,
) -> Result<(RepairClassification, VerifyTimings)> {
    classify_inner(
        target_manifest,
        dir,
        &VerifyOptions::default().with_hints(runtime_hints),
    )
}

pub fn classify_directory_with_timings_and_hints_and_mode(
    target_manifest: &TargetManifest,
    dir: &Path,
    runtime_hints: &RuntimeResourceHints,
    verify_mode: VerifyMode,
) -> Result<(RepairClassification, VerifyTimings)> {
    classify_inner(
        target_manifest,
        dir,
        &VerifyOptions::default()
            .with_mode(verify_mode)
            .with_hints(runtime_hints),
    )
}

pub fn classify_directory_with_progress(
    target_manifest: &TargetManifest,
    dir: &Path,
    runtime_hints: &RuntimeResourceHints,
) -> Result<(RepairClassification, VerifyTimings)> {
    classify_with_progress_bar(target_manifest, dir, runtime_hints, VerifyMode::Fast)
}

pub fn classify_directory_with_progress_and_mode(
    target_manifest: &TargetManifest,
    dir: &Path,
    runtime_hints: &RuntimeResourceHints,
    verify_mode: VerifyMode,
) -> Result<(RepairClassification, VerifyTimings)> {
    classify_with_progress_bar(target_manifest, dir, runtime_hints, verify_mode)
}

fn classify_with_progress_bar(
    target_manifest: &TargetManifest,
    dir: &Path,
    runtime_hints: &RuntimeResourceHints,
    verify_mode: VerifyMode,
) -> Result<(RepairClassification, VerifyTimings)> {
    run_with_progress_bar(target_manifest, |progress| {
        classify_inner(
            target_manifest,
            dir,
            &VerifyOptions::default()
                .with_mode(verify_mode)
                .with_hints(runtime_hints)
                .with_progress(progress),
        )
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn create_progress_bar(file_count: u64) -> ProgressBar {
    let pb = ProgressBar::new(file_count);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} verifying [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} files ({eta})")
            .unwrap()
            .progress_chars("#>-"),
    );
    pb
}

fn run_with_progress_bar<T>(
    target_manifest: &TargetManifest,
    run: impl FnOnce(&ProgressBar) -> Result<T>,
) -> Result<T> {
    let progress = create_progress_bar(target_manifest.files.len() as u64);
    let result = run(&progress);
    progress.finish_and_clear();
    result
}

fn default_runtime_hints() -> &'static RuntimeResourceHints {
    static DEFAULT_HINTS: std::sync::OnceLock<RuntimeResourceHints> = std::sync::OnceLock::new();
    DEFAULT_HINTS.get_or_init(RuntimeResourceHints::default)
}

fn accumulate_timings(total: &mut VerifyTimings, add: &VerifyTimings) {
    total.verify_mode = add.verify_mode;
    total.stat += add.stat;
    total.file_read += add.file_read;
    total.file_read_bytes += add.file_read_bytes;
    total.file_hash += add.file_hash;
    total.file_hash_bytes += add.file_hash_bytes;
    total.chunk_hash += add.chunk_hash;
    total.chunk_hash_bytes += add.chunk_hash_bytes;
}

// ---------------------------------------------------------------------------
// Per-file verification
// ---------------------------------------------------------------------------

fn verify_manifest_file(
    _target_manifest: &TargetManifest,
    dir: &Path,
    fm: &manifest::FileManifest,
    verify_mode: VerifyMode,
) -> Result<(Vec<RepairIssue>, VerifyTimings)> {
    let mut timings = VerifyTimings {
        verify_mode,
        ..VerifyTimings::default()
    };
    let file_path = resolve_manifest_relative_path(dir, &fm.path)?;

    if !file_path.exists() {
        return Ok((
            vec![RepairIssue {
                kind: RepairIssueKind::Missing,
                path: fm.path.clone(),
                detail: format!("missing file: {}", fm.path),
            }],
            timings,
        ));
    }

    let t = Instant::now();
    let metadata = match std::fs::metadata(&file_path) {
        Ok(m) => m,
        Err(e) => {
            return Ok((
                vec![RepairIssue {
                    kind: RepairIssueKind::Corrupt,
                    path: fm.path.clone(),
                    detail: format!("cannot stat {}: {}", fm.path, e),
                }],
                timings,
            ));
        }
    };
    timings.stat += t.elapsed();

    if metadata.len() != fm.size {
        return Ok((
            vec![RepairIssue {
                kind: RepairIssueKind::Corrupt,
                path: fm.path.clone(),
                detail: format!(
                    "size mismatch for {}: expected {}, got {}",
                    fm.path,
                    fm.size,
                    metadata.len()
                ),
            }],
            timings,
        ));
    }

    match verify_file_streaming(fm, &file_path, verify_mode, &mut timings) {
        Ok(issues) => Ok((issues, timings)),
        Err(e) => Ok((
            vec![RepairIssue {
                kind: RepairIssueKind::Corrupt,
                path: fm.path.clone(),
                detail: format!("cannot read {}: {}", fm.path, e),
            }],
            timings,
        )),
    }
}

fn verify_file_streaming(
    fm: &manifest::FileManifest,
    file_path: &Path,
    verify_mode: VerifyMode,
    timings: &mut VerifyTimings,
) -> std::io::Result<Vec<RepairIssue>> {
    let file = std::fs::File::open(file_path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut issues = Vec::new();
    let mut full_file_hasher = blake3::Hasher::new();
    let mut chunk_buffer = Vec::new();
    let mut chunk_offset = 0u64;

    for chunk_ref in &fm.chunks {
        let chunk_size = chunk_ref.raw_size as usize;
        let start = chunk_offset;
        let end = start + chunk_ref.raw_size;
        chunk_offset = end;

        if end > fm.size {
            issues.push(RepairIssue {
                kind: RepairIssueKind::Corrupt,
                path: fm.path.clone(),
                detail: format!(
                    "chunk reference out of bounds in {}: offset {}, size {}",
                    fm.path, start, chunk_ref.raw_size
                ),
            });
            break;
        }

        chunk_buffer.resize(chunk_size, 0);

        let t = Instant::now();
        reader.read_exact(&mut chunk_buffer)?;
        timings.file_read += t.elapsed();
        timings.file_read_bytes += chunk_ref.raw_size;

        let t = Instant::now();
        full_file_hasher.update(&chunk_buffer);
        timings.file_hash += t.elapsed();
        timings.file_hash_bytes += chunk_ref.raw_size;

        if verify_mode.checks_chunk_hashes() {
            let t = Instant::now();
            let chunk_hash_actual = hash_hex(&crate::hash::hash_chunk(&chunk_buffer));
            timings.chunk_hash += t.elapsed();
            timings.chunk_hash_bytes += chunk_ref.raw_size;
            if chunk_hash_actual != chunk_ref.chunk_hash {
                issues.push(RepairIssue {
                    kind: RepairIssueKind::Corrupt,
                    path: fm.path.clone(),
                    detail: format!(
                        "chunk hash mismatch in {} at offset {}: expected {}, got {}",
                        fm.path, start, chunk_ref.chunk_hash, chunk_hash_actual
                    ),
                });
            }
        }
    }

    if chunk_offset != fm.size {
        issues.push(RepairIssue {
            kind: RepairIssueKind::Corrupt,
            path: fm.path.clone(),
            detail: format!(
                "chunk coverage mismatch for {}: expected {} bytes, got {}",
                fm.path, fm.size, chunk_offset
            ),
        });
    }

    let t = Instant::now();
    let actual_hash = hash_hex(&full_file_hasher.finalize());
    timings.file_hash += t.elapsed();
    if actual_hash != fm.full_file_hash {
        issues.push(RepairIssue {
            kind: RepairIssueKind::Corrupt,
            path: fm.path.clone(),
            detail: format!(
                "hash mismatch for {}: expected {}, got {}",
                fm.path, fm.full_file_hash, actual_hash
            ),
        });
    }

    Ok(issues)
}

// ---------------------------------------------------------------------------
// Estimate
// ---------------------------------------------------------------------------

pub fn estimate_build(
    target_manifest: &TargetManifest,
    available_chunks: &HashSet<String>,
    available_disk_space: u64,
) -> Result<manifest::Estimate> {
    let mut missing_chunks: HashSet<String> = HashSet::new();
    let mut missing_logical: u64 = 0;

    for fm in &target_manifest.files {
        for cr in &fm.chunks {
            if !available_chunks.contains(&cr.chunk_hash) {
                missing_chunks.insert(cr.chunk_hash.clone());
                missing_logical += cr.raw_size;
            }
        }
    }

    let estimate_ratio = 0.5;
    let missing_compressed = (missing_logical as f64 * estimate_ratio) as u64;
    let bundle_estimate = if missing_chunks.is_empty() {
        0
    } else {
        1 + (missing_chunks.len() as u64 / 2000)
    };

    Ok(manifest::Estimate {
        missing_logical_bytes: missing_logical,
        missing_compressed_bytes: missing_compressed,
        missing_chunk_count: missing_chunks.len() as u64,
        required_bundle_count: bundle_estimate,
        range_request_count: missing_chunks.len() as u64,
        local_cache_hits: 0,
        available_disk_space,
        fragmentation_score: bundle_estimate as f64,
    })
}

#[cfg(test)]
#[path = "verify_tests.rs"]
mod tests;
