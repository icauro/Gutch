use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use rayon::prelude::*;

use crate::Result;
use crate::hash::ChunkHash;
use crate::manifest::runtime::typed_file_manifest;
use crate::manifest::{self, snapshot::SnapshotManifest};
use crate::progress::{
    GutchOperation, GutchOperationControl, GutchProgressEvent, GutchProgressPhase,
    GutchProgressSink, emit_progress,
};
use crate::runtime::{RuntimeResourceHints, build_thread_pool, plan_verify_runtime};
mod estimate;
mod types;

pub(crate) use estimate::estimate_build;
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

pub(crate) struct VerifyRequest<'a> {
    pub snapshot_manifest: &'a SnapshotManifest,
    pub dir: &'a Path,
    pub mode: VerifyMode,
    pub runtime_hints: &'a RuntimeResourceHints,
    pub control: GutchOperationControl,
}

struct PreparedVerifyFile<'a> {
    manifest: &'a manifest::FileManifest,
    path: PathBuf,
}

// ---------------------------------------------------------------------------
// Core implementation
// ---------------------------------------------------------------------------

fn classify_inner(
    snapshot_manifest: &SnapshotManifest,
    dir: &Path,
    opts: &VerifyOptions,
    scan_extra_files: bool,
) -> Result<(RepairClassification, VerifyTimings)> {
    let total_start = Instant::now();
    opts.control.checkpoint()?;
    let _operation_span = opts.control.profile_span("reconstructed.verify");
    let mut classification = RepairClassification::default();
    let runtime_plan = plan_verify_runtime(opts.hints);
    let mut timings = VerifyTimings {
        verify_mode: opts.mode,
        runtime_plan: Some(runtime_plan.clone()),
        ..VerifyTimings::default()
    };
    let verify_pool = build_thread_pool(runtime_plan.worker_count, "gutch-verify")?;
    let path_resolver = crate::path_guard::ManifestPathResolver::new(dir)?;
    let resolved_paths = {
        let _resolve_paths_span = opts.control.profile_span_with_args(
            "verify.prepare.paths",
            std::collections::BTreeMap::from([(
                "path_count".to_string(),
                serde_json::json!(snapshot_manifest.entries.len()),
            )]),
        );
        verify_pool.install(|| {
            snapshot_manifest
                .entries
                .par_iter()
                .map(|entry| match entry {
                    manifest::SnapshotEntry::Symlink(_) => path_resolver.resolve(entry.path()),
                    manifest::SnapshotEntry::File(_) | manifest::SnapshotEntry::Directory(_) => {
                        path_resolver.resolve_entry(entry.path())
                    }
                })
                .collect::<Vec<_>>()
        })
    };
    let resolved_paths = resolved_paths.into_iter().collect::<Result<Vec<_>>>()?;
    let mut prepared_files = snapshot_manifest
        .entries
        .iter()
        .zip(&resolved_paths)
        .filter_map(|(entry, path)| {
            let file = entry.as_file()?;
            Some(PreparedVerifyFile {
                manifest: file,
                path: path.clone(),
            })
        })
        .collect::<Vec<_>>();
    prepared_files.sort_unstable_by(|left, right| {
        right
            .manifest
            .size
            .cmp(&left.manifest.size)
            .then_with(|| left.manifest.path.cmp(&right.manifest.path))
    });
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
    for (entry, path) in snapshot_manifest.entries.iter().zip(&resolved_paths) {
        match entry {
            manifest::SnapshotEntry::File(_) => {}
            manifest::SnapshotEntry::Directory(directory) => {
                match std::fs::symlink_metadata(path) {
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
            manifest::SnapshotEntry::Symlink(link) => match std::fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    let raw_target = std::fs::read_link(path)?;
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
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    classification.push(RepairIssue {
                        kind: RepairIssueKind::Missing,
                        path: link.path.clone(),
                        detail: format!("missing symbolic link: {}", link.path),
                    })
                }
                Err(error) => return Err(error.into()),
            },
        }
    }
    let verify_files_start = Instant::now();
    let per_file_results = {
        let worker_count = runtime_plan.worker_count.max(1);
        let mut worker_lanes = (0..worker_count).map(|_| Vec::new()).collect::<Vec<_>>();
        for (sorted_index, prepared_file) in prepared_files.iter().enumerate() {
            worker_lanes[serpentine_worker_index(sorted_index, worker_count)]
                .push((sorted_index, prepared_file));
        }
        let _file_span = opts.control.profile_span_with_args(
            "verify.files",
            std::collections::BTreeMap::from([
                (
                    "file_count".to_string(),
                    serde_json::json!(prepared_files.len()),
                ),
                (
                    "scheduling".to_string(),
                    serde_json::json!("largest_first_serpentine"),
                ),
                ("worker_count".to_string(), serde_json::json!(worker_count)),
            ]),
        );
        let mut results = verify_pool
            .broadcast(|context| {
                worker_lanes[context.index()]
                    .iter()
                    .map(|(sorted_index, prepared_file)| {
                        opts.control.checkpoint()?;
                        let result = verify_manifest_file(
                            prepared_file,
                            opts.mode,
                            opts.control,
                            runtime_plan.worker_count,
                        );
                        let current = completed_files.fetch_add(1, Ordering::Relaxed) + 1;
                        emit_verify_progress(
                            opts.control.progress(),
                            current,
                            snapshot_manifest.file_count(),
                            false,
                        );
                        Ok((*sorted_index, result))
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .into_iter()
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        results.sort_unstable_by_key(|(sorted_index, _)| *sorted_index);
        results
    };

    for (_, result) in per_file_results {
        let (issues, file_timings) = result?;
        accumulate_timings(&mut timings, &file_timings);
        for issue in issues {
            classification.push(issue);
        }
    }
    timings.verify_files_wall = verify_files_start.elapsed();

    if scan_extra_files {
        // Detect extra filesystem entries without following symbolic links.
        let extra_file_scan_start = Instant::now();
        let _extra_scan_span = opts.control.profile_span("verify.extra_scan");
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
    }

    timings.total = total_start.elapsed();
    emit_verify_progress(
        opts.control.progress(),
        snapshot_manifest.file_count(),
        snapshot_manifest.file_count(),
        true,
    );
    Ok((classification, timings))
}

fn serpentine_worker_index(sorted_index: usize, worker_count: usize) -> usize {
    let worker_count = worker_count.max(1);
    let round = sorted_index / worker_count;
    let offset = sorted_index % worker_count;
    if round.is_multiple_of(2) {
        offset
    } else {
        worker_count - 1 - offset
    }
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

pub(crate) fn classify_directory(
    request: VerifyRequest<'_>,
) -> Result<(RepairClassification, VerifyTimings)> {
    classify_inner(
        request.snapshot_manifest,
        request.dir,
        &VerifyOptions::default()
            .with_mode(request.mode)
            .with_hints(request.runtime_hints)
            .with_control(&request.control),
        true,
    )
}

pub(crate) fn classify_expected_entries(
    request: VerifyRequest<'_>,
) -> Result<(RepairClassification, VerifyTimings)> {
    classify_inner(
        request.snapshot_manifest,
        request.dir,
        &VerifyOptions::default()
            .with_mode(request.mode)
            .with_hints(request.runtime_hints)
            .with_control(&request.control),
        false,
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
    prepared_file: &PreparedVerifyFile<'_>,
    verify_mode: VerifyMode,
    control: &GutchOperationControl,
    worker_count: usize,
) -> Result<(Vec<RepairIssue>, VerifyTimings)> {
    let fm = prepared_file.manifest;
    let _file_span = control.profile_span_with_args(
        "verify.file",
        crate::profile::file_trace_args(&fm.path, fm.size),
    );
    control.checkpoint()?;
    let mut timings = VerifyTimings {
        verify_mode,
        ..VerifyTimings::default()
    };
    let t = Instant::now();
    let metadata = match std::fs::symlink_metadata(&prepared_file.path) {
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
    timings.stat += t.elapsed();
    if !metadata.file_type().is_file() {
        return Ok((
            vec![RepairIssue {
                kind: RepairIssueKind::Corrupt,
                path: fm.path.clone(),
                detail: format!("entry type mismatch for file: {}", fm.path),
            }],
            timings,
        ));
    }

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

    let verification = if verify_mode == VerifyMode::Fast {
        verify_file_fast(prepared_file, worker_count, &mut timings, control)
    } else {
        verify_file_streaming(fm, &prepared_file.path, verify_mode, &mut timings, control)
    };
    match verification {
        Ok(issues) => Ok((issues, timings)),
        Err(crate::Error::Cancelled) => Err(crate::Error::Cancelled),
        Err(crate::Error::HashMismatch { expected, actual }) => Ok((
            vec![RepairIssue {
                kind: RepairIssueKind::Corrupt,
                path: fm.path.clone(),
                detail: format!(
                    "hash mismatch for {}: expected {}, got {}",
                    fm.path, expected, actual
                ),
            }],
            timings,
        )),
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

fn verify_file_fast(
    prepared_file: &PreparedVerifyFile<'_>,
    worker_count: usize,
    timings: &mut VerifyTimings,
    control: &GutchOperationControl,
) -> Result<Vec<RepairIssue>> {
    let expected_full_hash = ChunkHash::parse_hex(&prepared_file.manifest.full_file_hash)?;
    let integrity = crate::file_integrity::verify_full_file_integrity(
        &prepared_file.path,
        prepared_file.manifest.size,
        expected_full_hash,
        None,
        worker_count,
        control,
    )?;
    timings.file_read += integrity.read_elapsed;
    timings.file_read_bytes += integrity.bytes;
    timings.file_hash += integrity.hash_elapsed;
    timings.file_hash_bytes += integrity.bytes;
    Ok(Vec::new())
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
