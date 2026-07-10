use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use rayon::prelude::*;

use crate::Result;
use crate::hash::ChunkHash;
use crate::manifest::runtime::typed_file_manifest;
use crate::manifest::{self, target::TargetManifest};
use crate::path_guard::resolve_manifest_relative_path;
use crate::progress::{
    GutchOperation, GutchProgressEvent, GutchProgressPhase, GutchProgressSink, emit_progress,
};
use crate::runtime::{RuntimeResourceHints, install_in_thread_pool, plan_verify_runtime};
mod estimate;
mod types;

pub use estimate::estimate_build;
pub use types::{RepairClassification, RepairIssue, RepairIssueKind, VerifyMode, VerifyTimings};

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

struct VerifyOptions<'a> {
    mode: VerifyMode,
    hints: &'a RuntimeResourceHints,
    progress: &'a Option<Arc<dyn GutchProgressSink>>,
}

impl<'a> Default for VerifyOptions<'a> {
    fn default() -> Self {
        Self {
            mode: VerifyMode::Fast,
            hints: default_runtime_hints(),
            progress: no_progress(),
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

    fn with_progress(mut self, progress: &'a Option<Arc<dyn GutchProgressSink>>) -> Self {
        self.progress = progress;
        self
    }
}

pub struct VerifyRequest<'a> {
    pub target_manifest: &'a TargetManifest,
    pub dir: &'a Path,
    pub mode: VerifyMode,
    pub runtime_hints: &'a RuntimeResourceHints,
    pub progress: Option<Arc<dyn GutchProgressSink>>,
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
    let completed_files = AtomicUsize::new(0);
    emit_verify_progress(opts.progress, 0, target_manifest.files.len(), false);

    let per_file_results = install_in_thread_pool(runtime_plan.worker_count, || {
        target_manifest
            .files
            .par_iter()
            .map(|fm| {
                let result = verify_manifest_file(target_manifest, dir, fm, opts.mode);
                let current = completed_files.fetch_add(1, Ordering::Relaxed) + 1;
                emit_verify_progress(opts.progress, current, target_manifest.files.len(), false);
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
    emit_verify_progress(
        opts.progress,
        target_manifest.files.len(),
        target_manifest.files.len(),
        true,
    );
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

pub fn verify_directory(request: VerifyRequest<'_>) -> Result<(Vec<String>, VerifyTimings)> {
    verify_inner(
        request.target_manifest,
        request.dir,
        &VerifyOptions::default()
            .with_mode(request.mode)
            .with_hints(request.runtime_hints)
            .with_progress(&request.progress),
    )
}

pub fn classify_directory(
    request: VerifyRequest<'_>,
) -> Result<(RepairClassification, VerifyTimings)> {
    classify_inner(
        request.target_manifest,
        request.dir,
        &VerifyOptions::default()
            .with_mode(request.mode)
            .with_hints(request.runtime_hints)
            .with_progress(&request.progress),
    )
}

fn emit_verify_progress(
    progress: &Option<Arc<dyn GutchProgressSink>>,
    current: usize,
    total: usize,
    done: bool,
) {
    emit_progress(
        progress,
        GutchProgressEvent {
            operation: GutchOperation::Verify,
            phase: GutchProgressPhase::Verify,
            message: "Verifying files".to_string(),
            current,
            total: total.max(1),
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done,
        },
    );
}

fn no_progress() -> &'static Option<Arc<dyn GutchProgressSink>> {
    static NO_PROGRESS: Option<Arc<dyn GutchProgressSink>> = None;
    &NO_PROGRESS
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
    let typed_file =
        typed_file_manifest(fm).map_err(|err| std::io::Error::other(err.to_string()))?;
    let file = std::fs::File::open(file_path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut issues = Vec::new();
    let mut full_file_hasher = blake3::Hasher::new();
    let mut chunk_buffer = Vec::new();
    let mut chunk_offset = 0u64;

    for chunk_ref in &typed_file.chunks {
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
            let chunk_hash_actual = ChunkHash::from_blake3(crate::hash::hash_chunk(&chunk_buffer));
            timings.chunk_hash += t.elapsed();
            timings.chunk_hash_bytes += chunk_ref.raw_size;
            if chunk_hash_actual != chunk_ref.hash {
                issues.push(RepairIssue {
                    kind: RepairIssueKind::Corrupt,
                    path: fm.path.clone(),
                    detail: format!(
                        "chunk hash mismatch in {} at offset {}: expected {}, got {}",
                        fm.path,
                        start,
                        chunk_ref.hash_hex,
                        chunk_hash_actual.to_hex()
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
    let actual_hash = ChunkHash::from_blake3(full_file_hasher.finalize());
    timings.file_hash += t.elapsed();
    if actual_hash != typed_file.full_file_hash {
        issues.push(RepairIssue {
            kind: RepairIssueKind::Corrupt,
            path: fm.path.clone(),
            detail: format!(
                "hash mismatch for {}: expected {}, got {}",
                fm.path,
                fm.full_file_hash,
                actual_hash.to_hex()
            ),
        });
    }

    Ok(issues)
}

#[cfg(test)]
mod tests;
