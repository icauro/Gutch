//! Classifies snapshot entries on disk: locates them, verifies the files and reports issues.

use super::content::verify_manifest_file;
use super::inventory::{
    FilesystemInventory, augment_inventory_for_symlinked_parents, build_filesystem_inventory,
};
use super::non_files::{classify_inventory_non_files, classify_resolved_non_files};
use super::types::RepairClassification;
use super::types::RepairIssue;
use super::types::VerifyMode;
use super::types::VerifyTimings;
use crate::Result;
use crate::filesystem::paths::ManifestPathResolver;
use crate::manifest::FileManifest;
use crate::manifest::SnapshotEntry;
use crate::manifest::snapshot::SnapshotManifest;
use crate::profiling::ProfileInstant;
use crate::progress::OperationControl;
use crate::progress::OperationKind;
use crate::progress::ProgressEvent;
use crate::progress::ProgressPhase;
use crate::progress::ProgressSink;
use crate::progress::emit_progress;
use crate::runtime::RuntimeResourceHints;
use crate::runtime::build_thread_pool;
use crate::runtime::plan_runtime;
use rayon::ThreadPool;
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::fs::Metadata;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

/// Profile span names for one kind of inspection.
pub(crate) struct InspectionSpans {
    operation: &'static str,
    prepare_paths: &'static str,
    prepare_inventory: &'static str,
    files: &'static str,
    file: &'static str,
    extra_scan: &'static str,
}

macro_rules! inspection_spans {
    ($operation:literal) => {
        InspectionSpans {
            operation: $operation,
            prepare_paths: concat!($operation, ".prepare.paths"),
            prepare_inventory: concat!($operation, ".prepare.paths.inventory"),
            files: concat!($operation, ".files"),
            file: concat!($operation, ".file"),
            extra_scan: concat!($operation, ".extra_scan"),
        }
    };
}

impl InspectionSpans {
    pub(crate) const VERIFY: Self = inspection_spans!("installation.verify");
    pub(crate) const REPAIR: Self = inspection_spans!("installation.repair.verify");
    pub(crate) const REPAIR_RESULT: Self = inspection_spans!("installation.repair.verify.final");
}

pub(crate) struct VerifyRequest<'a> {
    pub snapshot_manifest: &'a SnapshotManifest,
    pub dir: &'a Path,
    pub mode: VerifyMode,
    pub runtime_hints: &'a RuntimeResourceHints,
    pub control: OperationControl,
    pub spans: &'static InspectionSpans,
    /// Reuses the caller's worker pool; `None` builds one from `runtime_hints`.
    pub pool: Option<&'a ThreadPool>,
}

pub(super) struct PreparedVerifyFile<'a> {
    pub(super) manifest: &'a FileManifest,
    pub(super) path: PathBuf,
    pub(super) metadata: PreparedFileMetadata,
}

pub(super) enum PreparedFileMetadata {
    Inventory(Option<Metadata>),
    ResolveDuringVerification,
}

/// Classifies every snapshot entry under `request.dir` and reports files the
/// snapshot does not contain.
pub(crate) fn classify_directory(
    request: VerifyRequest<'_>,
) -> Result<(RepairClassification, VerifyTimings)> {
    classify(request, true)
}

/// Classifies only the entries the snapshot lists; other files are ignored.
pub(crate) fn classify_expected_entries(
    request: VerifyRequest<'_>,
) -> Result<(RepairClassification, VerifyTimings)> {
    classify(request, false)
}

fn classify(
    request: VerifyRequest<'_>,
    scan_extra_files: bool,
) -> Result<(RepairClassification, VerifyTimings)> {
    let VerifyRequest {
        snapshot_manifest,
        dir,
        mode,
        runtime_hints,
        control,
        spans,
        pool: supplied_pool,
    } = request;
    let control = &control;
    let total_start = ProfileInstant::start(control.profiler());
    control.checkpoint()?;
    let _operation_span = control.profile_span(spans.operation);
    let mut classification = RepairClassification::default();
    let mut runtime_plan = plan_runtime(runtime_hints);
    let owned_pool = match supplied_pool {
        Some(_) => None,
        None => Some(build_thread_pool(
            runtime_plan.worker_count,
            "Verification worker",
        )?),
    };
    let verify_pool = supplied_pool
        .or(owned_pool.as_ref())
        .expect("verification pool must be available");
    runtime_plan.worker_count = verify_pool.current_num_threads().max(1);
    let worker_count = runtime_plan.worker_count;
    let mut timings = VerifyTimings {
        verify_mode: mode,
        runtime_plan: Some(runtime_plan),
        ..VerifyTimings::default()
    };

    let locations = if scan_extra_files {
        EntryLocations::Inventory(inventory_entries(snapshot_manifest, dir, control, spans)?)
    } else {
        EntryLocations::Resolved(resolve_entry_paths(
            snapshot_manifest,
            dir,
            control,
            spans,
            verify_pool,
        )?)
    };
    let prepared_files = locations.prepared_files(snapshot_manifest);
    emit_verify_progress(control.progress(), 0, snapshot_manifest.file_count(), false);
    timings.prepare_wall = total_start.elapsed();

    locations.classify_non_files(snapshot_manifest, &mut classification)?;
    let verify_files_start = ProfileInstant::start(control.profiler());
    let file_results = verify_files(
        &prepared_files,
        FileVerification {
            mode,
            control,
            spans,
            worker_count,
            total_file_count: snapshot_manifest.file_count(),
        },
        verify_pool,
    );
    for result in file_results {
        let (issues, file_timings) = result?;
        accumulate_timings(&mut timings, &file_timings);
        for issue in issues {
            classification.push(issue);
        }
    }
    timings.verify_files_wall = verify_files_start.elapsed();

    if let EntryLocations::Inventory(inventory) = &locations {
        let extra_file_scan_start = ProfileInstant::start(control.profiler());
        let _extra_scan_span = control.profile_span(spans.extra_scan);
        for relative in &inventory.extra_paths {
            control.checkpoint()?;
            classification.push(RepairIssue::extra(
                relative,
                format!("extra filesystem entry: {relative}"),
            ));
        }
        timings.extra_file_scan_wall = extra_file_scan_start.elapsed();
    }

    timings.total = total_start.elapsed();
    emit_verify_progress(
        control.progress(),
        snapshot_manifest.file_count(),
        snapshot_manifest.file_count(),
        true,
    );
    Ok((classification, timings))
}

/// Where the snapshot's entries were found on disk.
enum EntryLocations {
    /// A walk of the whole directory, which also finds extra files.
    Inventory(FilesystemInventory),
    /// Each entry's path resolved on its own, in manifest order.
    Resolved(Vec<PathBuf>),
}

impl EntryLocations {
    /// Lists the snapshot's files, largest first and then by path, so the
    /// longest checks start early.
    fn prepared_files<'a>(
        &self,
        snapshot_manifest: &'a SnapshotManifest,
    ) -> Vec<PreparedVerifyFile<'a>> {
        let mut prepared_files: Vec<_> = match self {
            Self::Inventory(inventory) => snapshot_manifest
                .files()
                .map(|file| {
                    let actual = inventory.entries.get(&inventory.key(&file.path));
                    PreparedVerifyFile {
                        manifest: file,
                        path: actual
                            .map(|entry| entry.path.clone())
                            .unwrap_or_else(|| inventory.canonical_root.join(&file.path)),
                        metadata: PreparedFileMetadata::Inventory(
                            actual.and_then(|entry| entry.metadata.clone()),
                        ),
                    }
                })
                .collect(),
            Self::Resolved(resolved_paths) => snapshot_manifest
                .entries
                .iter()
                .zip(resolved_paths)
                .filter_map(|(entry, path)| {
                    Some(PreparedVerifyFile {
                        manifest: entry.as_file()?,
                        path: path.clone(),
                        metadata: PreparedFileMetadata::ResolveDuringVerification,
                    })
                })
                .collect(),
        };
        prepared_files.sort_unstable_by(|left, right| {
            right
                .manifest
                .size
                .cmp(&left.manifest.size)
                .then_with(|| left.manifest.path.cmp(&right.manifest.path))
        });
        prepared_files
    }

    fn classify_non_files(
        &self,
        snapshot_manifest: &SnapshotManifest,
        classification: &mut RepairClassification,
    ) -> Result<()> {
        match self {
            Self::Inventory(inventory) => {
                classify_inventory_non_files(snapshot_manifest, inventory, classification)
            }
            Self::Resolved(resolved_paths) => {
                classify_resolved_non_files(snapshot_manifest, resolved_paths, classification)
            }
        }
    }
}

fn prepare_paths_args(
    snapshot_manifest: &SnapshotManifest,
    strategy: &str,
) -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([
        (
            "path_count".to_string(),
            serde_json::json!(snapshot_manifest.entries.len()),
        ),
        ("strategy".to_string(), serde_json::json!(strategy)),
    ])
}

/// Walks `dir` once, including the targets of symlinked parent directories.
fn inventory_entries(
    snapshot_manifest: &SnapshotManifest,
    dir: &Path,
    control: &OperationControl,
    spans: &InspectionSpans,
) -> Result<FilesystemInventory> {
    let _span = control.profile_span_with_args(spans.prepare_paths, || {
        prepare_paths_args(snapshot_manifest, "filesystem_inventory")
    });
    let mut inventory = build_filesystem_inventory(snapshot_manifest, dir, control)?;
    augment_inventory_for_symlinked_parents(snapshot_manifest, dir, &mut inventory, control)?;
    control.profile_event_with_args(spans.prepare_inventory, || {
        BTreeMap::from([
            (
                "inventory_entry_count".to_string(),
                serde_json::json!(inventory.entries.len()),
            ),
            (
                "extra_path_count".to_string(),
                serde_json::json!(inventory.extra_paths.len()),
            ),
        ])
    });
    Ok(inventory)
}

/// Resolves each entry's path in parallel. A symlink resolves to the link
/// itself; other entries resolve through their parent directories.
fn resolve_entry_paths(
    snapshot_manifest: &SnapshotManifest,
    dir: &Path,
    control: &OperationControl,
    spans: &InspectionSpans,
    pool: &ThreadPool,
) -> Result<Vec<PathBuf>> {
    let path_resolver = ManifestPathResolver::new(dir)?;
    let _span = control.profile_span_with_args(spans.prepare_paths, || {
        prepare_paths_args(snapshot_manifest, "targeted_resolver")
    });
    pool.install(|| {
        snapshot_manifest
            .entries
            .par_iter()
            .map(|entry| match entry {
                SnapshotEntry::Symlink(_) => path_resolver.resolve(entry.path()),
                SnapshotEntry::File(_) | SnapshotEntry::Directory(_) => {
                    path_resolver.resolve_entry(entry.path())
                }
            })
            .collect()
    })
}

/// The settings shared by every file check of one inspection.
struct FileVerification<'a> {
    mode: VerifyMode,
    control: &'a OperationControl,
    spans: &'static InspectionSpans,
    worker_count: usize,
    total_file_count: usize,
}

/// Verifies the files in parallel and returns their results in input order.
fn verify_files(
    prepared_files: &[PreparedVerifyFile<'_>],
    verification: FileVerification<'_>,
    pool: &ThreadPool,
) -> Vec<Result<(Vec<RepairIssue>, VerifyTimings)>> {
    let FileVerification {
        mode,
        control,
        spans,
        worker_count,
        total_file_count,
    } = verification;
    let _span = control.profile_span_with_args(spans.files, || {
        BTreeMap::from([
            (
                "file_count".to_string(),
                serde_json::json!(prepared_files.len()),
            ),
            (
                "scheduling".to_string(),
                serde_json::json!("largest_first_work_stealing"),
            ),
            ("worker_count".to_string(), serde_json::json!(worker_count)),
        ])
    });
    let completed_files = AtomicUsize::new(0);
    let concurrent_file_count = worker_count.min(prepared_files.len());
    // Indexed collection preserves size/path order while idle workers steal work.
    pool.install(|| {
        prepared_files
            .par_iter()
            // Let another worker take the next file even when adjacent sizes differ.
            .with_max_len(1)
            .map(|prepared_file| {
                control.checkpoint()?;
                let result = verify_manifest_file(
                    prepared_file,
                    mode,
                    control,
                    concurrent_file_count,
                    spans.file,
                );
                let current = completed_files.fetch_add(1, Ordering::Relaxed) + 1;
                emit_verify_progress(control.progress(), current, total_file_count, false);
                result
            })
            .collect()
    })
}

fn emit_verify_progress(
    progress: &Option<Arc<dyn ProgressSink>>,
    current: usize,
    total: usize,
    done: bool,
) {
    emit_progress(
        progress,
        ProgressEvent {
            feedback: None,
            operation: OperationKind::Verify,
            phase: ProgressPhase::Verify,
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

#[cfg(test)]
#[path = "classify_tests.rs"]
mod tests;
