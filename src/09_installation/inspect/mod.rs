//! File and tree inspection; metadata checks remain distinct from content verification.

mod content;
mod inventory;
mod non_files;
use crate::profiling::ProfileInstant;
use content::verify_manifest_file;
use inventory::{
    augment_inventory_for_symlinked_parents, build_filesystem_inventory, inventory_path_key,
};
use non_files::{classify_inventory_non_files, classify_resolved_non_files};
use std::fs::Metadata;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use rayon::ThreadPool;
use rayon::prelude::*;

use crate::Result;
use crate::manifest;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::OperationControl;
use crate::progress::OperationKind;
use crate::progress::ProgressEvent;
use crate::progress::ProgressPhase;
use crate::progress::ProgressSink;
use crate::progress::emit_progress;
use crate::runtime::RuntimeResourceHints;
use crate::runtime::build_thread_pool;
use crate::runtime::plan_runtime;
mod types;

use crate::filesystem::paths::ManifestPathResolver;
use std::collections::BTreeMap;
pub use types::RepairClassification;
pub use types::RepairIssue;
pub use types::RepairIssueKind;
pub use types::VerifyMode;
pub use types::VerifyTimings;

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

struct PreparedVerifyFile<'a> {
    manifest: &'a manifest::FileManifest,
    path: PathBuf,
    metadata: PreparedFileMetadata,
}

enum PreparedFileMetadata {
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
    let mut timings = VerifyTimings {
        verify_mode: mode,
        runtime_plan: Some(runtime_plan.clone()),
        ..VerifyTimings::default()
    };
    let owned_pool = if supplied_pool.is_none() {
        Some(build_thread_pool(
            runtime_plan.worker_count,
            "Verification worker",
        )?)
    } else {
        None
    };
    let verify_pool = supplied_pool
        .or(owned_pool.as_ref())
        .expect("verification pool must be available");
    runtime_plan.worker_count = verify_pool.current_num_threads().max(1);
    timings.runtime_plan = Some(runtime_plan.clone());
    let (inventory, resolved_paths) = if scan_extra_files {
        let _resolve_paths_span = control.profile_span_with_args(spans.prepare_paths, || {
            BTreeMap::from([
                (
                    "path_count".to_string(),
                    serde_json::json!(snapshot_manifest.entries.len()),
                ),
                (
                    "strategy".to_string(),
                    serde_json::json!("filesystem_inventory"),
                ),
            ])
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
        (Some(inventory), None)
    } else {
        let path_resolver = ManifestPathResolver::new(dir)?;
        let resolved_paths = {
            let _resolve_paths_span = control.profile_span_with_args(spans.prepare_paths, || {
                BTreeMap::from([
                    (
                        "path_count".to_string(),
                        serde_json::json!(snapshot_manifest.entries.len()),
                    ),
                    (
                        "strategy".to_string(),
                        serde_json::json!("targeted_resolver"),
                    ),
                ])
            });
            verify_pool.install(|| {
                snapshot_manifest
                    .entries
                    .par_iter()
                    .map(|entry| match entry {
                        manifest::SnapshotEntry::Symlink(_) => path_resolver.resolve(entry.path()),
                        manifest::SnapshotEntry::File(_)
                        | manifest::SnapshotEntry::Directory(_) => {
                            path_resolver.resolve_entry(entry.path())
                        }
                    })
                    .collect::<Vec<_>>()
            })
        };
        (
            None,
            Some(resolved_paths.into_iter().collect::<Result<Vec<_>>>()?),
        )
    };
    let mut prepared_files = if let Some(inventory) = inventory.as_ref() {
        snapshot_manifest
            .entries
            .iter()
            .filter_map(|entry| {
                let file = entry.as_file()?;
                let actual = inventory.entries.get(&inventory_path_key(&file.path));
                Some(PreparedVerifyFile {
                    manifest: file,
                    path: actual
                        .map(|entry| entry.path.clone())
                        .unwrap_or_else(|| inventory.canonical_root.join(&file.path)),
                    metadata: PreparedFileMetadata::Inventory(
                        actual.and_then(|entry| entry.metadata.clone()),
                    ),
                })
            })
            .collect::<Vec<_>>()
    } else {
        let resolved_paths = resolved_paths
            .as_ref()
            .expect("targeted verification paths must be prepared");
        snapshot_manifest
            .entries
            .iter()
            .zip(resolved_paths)
            .filter_map(|(entry, path)| {
                let file = entry.as_file()?;
                Some(PreparedVerifyFile {
                    manifest: file,
                    path: path.clone(),
                    metadata: PreparedFileMetadata::ResolveDuringVerification,
                })
            })
            .collect::<Vec<_>>()
    };
    prepared_files.sort_unstable_by(|left, right| {
        right
            .manifest
            .size
            .cmp(&left.manifest.size)
            .then_with(|| left.manifest.path.cmp(&right.manifest.path))
    });
    let completed_files = AtomicUsize::new(0);
    emit_verify_progress(control.progress(), 0, snapshot_manifest.file_count(), false);

    timings.prepare_wall = total_start.elapsed();
    if let Some(inventory) = inventory.as_ref() {
        classify_inventory_non_files(snapshot_manifest, inventory, &mut classification)?;
    } else {
        let resolved_paths = resolved_paths
            .as_ref()
            .expect("targeted verification paths must be prepared");
        classify_resolved_non_files(snapshot_manifest, resolved_paths, &mut classification)?;
    }
    let verify_files_start = ProfileInstant::start(control.profiler());
    let per_file_results = {
        let _file_span = control.profile_span_with_args(spans.files, || {
            BTreeMap::from([
                (
                    "file_count".to_string(),
                    serde_json::json!(prepared_files.len()),
                ),
                (
                    "scheduling".to_string(),
                    serde_json::json!("largest_first_work_stealing"),
                ),
                (
                    "worker_count".to_string(),
                    serde_json::json!(runtime_plan.worker_count),
                ),
            ])
        });
        // Indexed collection preserves size/path order while idle workers steal work.
        verify_pool.install(|| {
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
                        runtime_plan.worker_count.min(prepared_files.len()),
                        spans.file,
                    );
                    let current = completed_files.fetch_add(1, Ordering::Relaxed) + 1;
                    emit_verify_progress(
                        control.progress(),
                        current,
                        snapshot_manifest.file_count(),
                        false,
                    );
                    result
                })
                .collect::<Vec<_>>()
        })
    };
    for result in per_file_results {
        let (issues, file_timings) = result?;
        accumulate_timings(&mut timings, &file_timings);
        for issue in issues {
            classification.push(issue);
        }
    }
    timings.verify_files_wall = verify_files_start.elapsed();

    if let Some(inventory) = inventory.as_ref() {
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

// ---------------------------------------------------------------------------
// Per-file verification
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
