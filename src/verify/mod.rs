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
use crate::manifest::{self, snapshot::SnapshotManifest};
use crate::path_guard::resolve_manifest_relative_path;
use crate::progress::{
    GutchOperation, GutchOperationControl, GutchProgressEvent, GutchProgressPhase,
    GutchProgressSink, emit_progress,
};
use crate::runtime::{RuntimeResourceHints, plan_verify_runtime, run_in_thread_pool};
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
    control: &'a GutchOperationControl,
}

impl<'a> Default for VerifyOptions<'a> {
    fn default() -> Self {
        Self {
            mode: VerifyMode::Fast,
            hints: default_runtime_hints(),
            control: default_control(),
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

    fn with_control(mut self, control: &'a GutchOperationControl) -> Self {
        self.control = control;
        self
    }
}

pub struct VerifyRequest<'a> {
    pub snapshot_manifest: &'a SnapshotManifest,
    pub dir: &'a Path,
    pub mode: VerifyMode,
    pub runtime_hints: &'a RuntimeResourceHints,
    pub control: GutchOperationControl,
}

// ---------------------------------------------------------------------------
// Core implementation
// ---------------------------------------------------------------------------

fn classify_inner(
    snapshot_manifest: &SnapshotManifest,
    dir: &Path,
    opts: &VerifyOptions,
) -> Result<(RepairClassification, VerifyTimings)> {
    let total_start = Instant::now();
    opts.control.checkpoint()?;
    let mut classification = RepairClassification::default();
    let runtime_plan = plan_verify_runtime(opts.hints);
    let mut timings = VerifyTimings {
        verify_mode: opts.mode,
        runtime_plan: Some(runtime_plan.clone()),
        ..VerifyTimings::default()
    };
    let manifest_paths: HashSet<&str> = snapshot_manifest
        .entries
        .iter()
        .map(crate::manifest::SnapshotEntry::path)
        .collect();
    let completed_files = AtomicUsize::new(0);
    emit_verify_progress(
        opts.control.progress(),
        0,
        snapshot_manifest.file_count(),
        false,
    );

    timings.prepare_wall = total_start.elapsed();
    for entry in &snapshot_manifest.entries {
        match entry {
            manifest::SnapshotEntry::File(_) => {}
            manifest::SnapshotEntry::Directory(directory) => {
                let path = resolve_manifest_relative_path(dir, &directory.path)?;
                match std::fs::symlink_metadata(&path) {
                    Ok(metadata) if metadata.file_type().is_dir() => {
                        if metadata.permissions().readonly() != directory.read_only {
                            classification.push(RepairIssue {
                                kind: RepairIssueKind::Corrupt,
                                path: directory.path.clone(),
                                detail: format!(
                                    "read-only mismatch for directory: {}",
                                    directory.path
                                ),
                            });
                        }
                    }
                    Ok(_) => classification.push(RepairIssue {
                        kind: RepairIssueKind::Corrupt,
                        path: directory.path.clone(),
                        detail: format!("entry type mismatch for directory: {}", directory.path),
                    }),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => classification
                        .push(RepairIssue {
                            kind: RepairIssueKind::Missing,
                            path: directory.path.clone(),
                            detail: format!("missing directory: {}", directory.path),
                        }),
                    Err(error) => return Err(error.into()),
                }
            }
            manifest::SnapshotEntry::Symlink(link) => {
                let path = resolve_manifest_relative_path(dir, &link.path)?;
                match std::fs::symlink_metadata(&path) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        let raw_target = std::fs::read_link(&path)?;
                        let actual_target = normalize_actual_link_target(&link.path, &raw_target)?;
                        if actual_target != link.target_path {
                            classification.push(RepairIssue {
                                kind: RepairIssueKind::Corrupt,
                                path: link.path.clone(),
                                detail: format!("symbolic-link target mismatch for {}", link.path),
                            });
                        }
                    }
                    Ok(_) => classification.push(RepairIssue {
                        kind: RepairIssueKind::Corrupt,
                        path: link.path.clone(),
                        detail: format!("entry type mismatch for symbolic link: {}", link.path),
                    }),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => classification
                        .push(RepairIssue {
                            kind: RepairIssueKind::Missing,
                            path: link.path.clone(),
                            detail: format!("missing symbolic link: {}", link.path),
                        }),
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
    let verify_files_start = Instant::now();
    let per_file_results = run_in_thread_pool(runtime_plan.worker_count, || {
        snapshot_manifest
            .entries
            .par_iter()
            .filter_map(crate::manifest::SnapshotEntry::as_file)
            .map(|fm| {
                opts.control.checkpoint()?;
                let result =
                    verify_manifest_file(snapshot_manifest, dir, fm, opts.mode, opts.control);
                let current = completed_files.fetch_add(1, Ordering::Relaxed) + 1;
                emit_verify_progress(
                    opts.control.progress(),
                    current,
                    snapshot_manifest.file_count(),
                    false,
                );
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
    timings.verify_files_wall = verify_files_start.elapsed();

    // Detect extra filesystem entries without following symbolic links.
    let extra_file_scan_start = Instant::now();
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        opts.control.checkpoint()?;
        let entry = entry?;
        if entry.depth() == 0 {
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
            detail: format!("extra filesystem entry: {}", relative),
        });
    }
    timings.extra_file_scan_wall = extra_file_scan_start.elapsed();

    timings.total = total_start.elapsed();
    emit_verify_progress(
        opts.control.progress(),
        snapshot_manifest.file_count(),
        snapshot_manifest.file_count(),
        true,
    );
    Ok((classification, timings))
}

fn normalize_actual_link_target(link_path: &str, raw_target: &Path) -> Result<String> {
    if raw_target.is_absolute() {
        return Err(crate::Error::verify("symbolic-link target is absolute"));
    }
    let combined = Path::new(link_path)
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(raw_target);
    let mut normalized = std::path::PathBuf::new();
    for component in combined.components() {
        match component {
            std::path::Component::Normal(value) => normalized.push(value),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return Err(crate::Error::verify("symbolic-link target escapes root"));
                }
            }
            _ => return Err(crate::Error::verify("symbolic-link target is invalid")),
        }
    }
    Ok(normalized.to_string_lossy().replace('\\', "/"))
}

fn verify_inner(
    snapshot_manifest: &SnapshotManifest,
    dir: &Path,
    opts: &VerifyOptions,
) -> Result<(Vec<String>, VerifyTimings)> {
    let (classification, timings) = classify_inner(snapshot_manifest, dir, opts)?;
    Ok((classification.into_messages(), timings))
}

pub fn verify_directory(request: VerifyRequest<'_>) -> Result<(Vec<String>, VerifyTimings)> {
    verify_inner(
        request.snapshot_manifest,
        request.dir,
        &VerifyOptions::default()
            .with_mode(request.mode)
            .with_hints(request.runtime_hints)
            .with_control(&request.control),
    )
}

pub fn classify_directory(
    request: VerifyRequest<'_>,
) -> Result<(RepairClassification, VerifyTimings)> {
    classify_inner(
        request.snapshot_manifest,
        request.dir,
        &VerifyOptions::default()
            .with_mode(request.mode)
            .with_hints(request.runtime_hints)
            .with_control(&request.control),
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

fn default_control() -> &'static GutchOperationControl {
    static CONTROL: std::sync::OnceLock<GutchOperationControl> = std::sync::OnceLock::new();
    CONTROL.get_or_init(GutchOperationControl::default)
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
    _snapshot_manifest: &SnapshotManifest,
    dir: &Path,
    fm: &manifest::FileManifest,
    verify_mode: VerifyMode,
    control: &GutchOperationControl,
) -> Result<(Vec<RepairIssue>, VerifyTimings)> {
    control.checkpoint()?;
    let mut timings = VerifyTimings {
        verify_mode,
        ..VerifyTimings::default()
    };
    let file_path = resolve_manifest_relative_path(dir, &fm.path)?;

    let source_metadata = match std::fs::symlink_metadata(&file_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((
                vec![RepairIssue {
                    kind: RepairIssueKind::Missing,
                    path: fm.path.clone(),
                    detail: format!("missing file: {}", fm.path),
                }],
                timings,
            ));
        }
        Err(error) => return Err(error.into()),
    };
    if !source_metadata.file_type().is_file() {
        return Ok((
            vec![RepairIssue {
                kind: RepairIssueKind::Corrupt,
                path: fm.path.clone(),
                detail: format!("entry type mismatch for file: {}", fm.path),
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

    if metadata.permissions().readonly() != fm.read_only {
        return Ok((
            vec![RepairIssue {
                kind: RepairIssueKind::Corrupt,
                path: fm.path.clone(),
                detail: format!("read-only mismatch for file: {}", fm.path),
            }],
            timings,
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if (metadata.permissions().mode() & 0o111 != 0) != fm.executable {
            return Ok((
                vec![RepairIssue {
                    kind: RepairIssueKind::Corrupt,
                    path: fm.path.clone(),
                    detail: format!("executable mismatch for file: {}", fm.path),
                }],
                timings,
            ));
        }
    }

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

    match verify_file_streaming(fm, &file_path, verify_mode, &mut timings, control) {
        Ok(issues) => Ok((issues, timings)),
        Err(crate::Error::Cancelled) => Err(crate::Error::Cancelled),
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
    control: &GutchOperationControl,
) -> Result<Vec<RepairIssue>> {
    let typed_file =
        typed_file_manifest(fm).map_err(|err| std::io::Error::other(err.to_string()))?;
    let file = std::fs::File::open(file_path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut issues = Vec::new();
    let mut full_file_hasher = blake3::Hasher::new();
    let mut chunk_buffer = Vec::new();
    let mut chunk_offset = 0u64;

    for chunk_ref in &typed_file.chunks {
        control.checkpoint()?;
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
