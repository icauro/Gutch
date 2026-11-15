//! Captures one source into a repository snapshot and publishes it.

use crate::capture::contracts::{SnapshotCaptureRequest, SnapshotCaptureResult};
use crate::error::capture_outcome::{MetadataFieldDifference, SnapshotCaptureOutcome};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use crate::capture::reuse::CaptureReuseState;
use crate::capture::reuse::load_capture_reuse_state;
use crate::capture::scan::scan_source_inventory;
use crate::progress;
use crate::progress::OperationKind;
use crate::progress::ProgressEvent;
use crate::progress::ProgressPhase;
use crate::progress::ProgressSink;
use crate::repo::Result;

use crate::artifact::profiles::BundleSizingProfile;
use crate::artifact::profiles::ChunkSizingProfile;
use crate::artifact::profiles::validate_packaging_sizes;
use crate::capture::build::BuiltCapture;
use crate::capture::build::build_with_stable_source;
use crate::capture::build::types::BuildConfig;
use crate::capture::build::types::BuildTimings;
use crate::capture::plan::plan_snapshot_capture;
use crate::capture::publish::publish_snapshot_artifacts;
use crate::capture::scan::validate_source_inventory;
use crate::manifest::SnapshotManifest;
use crate::manifest::SnapshotTagSet;
use crate::progress::OperationControl;
use crate::repo::coordination::write_with_control;
use crate::repo::index::required_bundle_ids;
use crate::repo::index::required_chunk_hashes;
use crate::repo::index::write_chunk_index;
use crate::repo::paths::RepoPaths;
use crate::repo::paths::load_gutch_repo_manifest;
use crate::repo::paths::write_gutch_repo_manifest;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use crate::repo::snapshot_manifests::snapshot_metadata_path;
use crate::runtime::RuntimeResourceHints;
use crate::runtime::build_thread_pool;
use crate::runtime::plan_runtime;
use crate::snapshot::identity::format_system_time_utc;
use std::collections::BTreeMap;
use std::fs;
use std::mem;
fn metadata_display_value<T: serde::Serialize>(value: Option<&T>) -> String {
    match value {
        None => String::from("(absent)"),
        Some(value) => {
            serde_json::to_string(value).unwrap_or_else(|_| String::from("(unserializable)"))
        }
    }
}

pub fn capture_snapshot(request: SnapshotCaptureRequest) -> Result<SnapshotCaptureResult> {
    validate_packaging_sizes(
        &ChunkSizingProfile::for_preset(request.chunk_profile),
        &BundleSizingProfile::for_preset(request.bundle_profile),
        request.storage_mode,
    )?;
    request.control.checkpoint()?;
    let operation_root = request.repo_root.to_path_buf();
    write_with_control(&operation_root, &request.control.clone(), || {
        request.control.checkpoint()?;
        let build_pool = build_capture_pool(&request.control, &request.runtime_hints)?;

        let repo_paths = RepoPaths::from_repo_root(&request.repo_root);
        let _existing_state_span = request
            .control
            .profile_span("capture.prepare_source.load_existing_store_state");
        let mut reuse_state = load_capture_reuse_state(&repo_paths, &build_pool, &request.control)?;
        drop(_existing_state_span);

        let (_, mut manifest) = load_gutch_repo_manifest(&request.repo_root)?;
        manifest.set_artifact_profile(
            request.chunk_profile,
            request.compression_algorithm,
            request.compression_level,
            request.bundle_profile,
            request.storage_mode,
        );
        manifest.artifact_profile.chunker = request.chunker;
        let result = capture_snapshot_reusing_state(request, &mut reuse_state, &build_pool)?;
        write_gutch_repo_manifest(&repo_paths, &manifest)?;
        Ok(result)
    })
    .map_err(capture_error)
}

pub(crate) fn capture_snapshot_reusing_state(
    request: SnapshotCaptureRequest,
    reuse_state: &mut CaptureReuseState,
    build_pool: &rayon::ThreadPool,
) -> Result<SnapshotCaptureResult> {
    request.control.checkpoint()?;
    let operation_root = request.repo_root.to_path_buf();
    write_with_control(&operation_root, &request.control.clone(), || {
        plan_snapshot_capture(
            request.repo_root.as_path(),
            request.source_dir.as_path(),
            &request.explicit_tags,
            &request.control,
        )?;
        execute_repo_snapshot_capture(request, reuse_state, build_pool)
    })
    .map_err(capture_error)
}

pub(super) fn capture_error(error: crate::Error) -> crate::Error {
    match error {
        crate::Error::SnapshotIntegrityMismatch { snapshot_id } => {
            crate::Error::SnapshotCaptureConflict {
                outcome: SnapshotCaptureOutcome::IntegrityConflict { snapshot_id },
            }
        }
        error => error,
    }
}

fn execute_repo_snapshot_capture(
    request: SnapshotCaptureRequest,
    reuse_state: &mut CaptureReuseState,
    build_pool: &rayon::ThreadPool,
) -> Result<SnapshotCaptureResult> {
    let control = &request.control;
    control.checkpoint()?;
    let prepare_span = control.profile_span("capture.prepare_source");
    let (repo_paths, repo_manifest) = {
        let _span = control.profile_span("capture.prepare_source.load_repo_state");
        load_gutch_repo_manifest(&request.repo_root)?
    };
    let prepared_input = {
        let _span = control.profile_span("capture.prepare_source.prepare_input");
        prepare_input_path(&request.source_dir)?
    };
    let runtime_plan = plan_runtime(&request.runtime_hints);
    let source_inventory = {
        let _span = control.profile_span("capture.prepare_source.inventory_source");
        scan_source_inventory(
            &prepared_input.dir,
            control,
            runtime_plan.worker_count,
            build_pool,
        )?
    };
    let constraints = repo_manifest.constraints.clone().unwrap_or_default();
    validate_source_inventory(&source_inventory, &constraints)?;
    let snapshot_tags = {
        let _span = control.profile_span("capture.prepare_source.resolve_tags");
        let snapshot_tags = request.explicit_tags.normalized()?;
        constraints
            .validate_snapshot_tags(&snapshot_tags, &request.source_dir.display().to_string())?;
        snapshot_tags
    };
    let build_config = configure_build(&request, reuse_state, &repo_paths, &snapshot_tags);
    drop(prepare_span);

    let BuiltCapture {
        mut result,
        timings,
        source_inventory,
    } = build_with_stable_source(
        &prepared_input.dir,
        &build_config,
        build_pool,
        source_inventory,
        &constraints,
        runtime_plan.worker_count,
    )?;
    let _publish_span = control.profile_span("capture.publish_snapshot");
    control.checkpoint()?;
    let snapshot_id = result.snapshot_manifest.snapshot_id.clone();
    result.snapshot_manifest.source_modified_at =
        format_system_time_utc(source_inventory.source_modified_at);
    result.snapshot_manifest.snapshot_tags = snapshot_tags;

    let existing_manifest_path = snapshot_metadata_path(&repo_paths, &snapshot_id)?;
    let snapshot_already_existed = existing_manifest_path.exists();
    if snapshot_already_existed {
        // The content-derived ID already exists. Identical metadata returns
        // unchanged; differing metadata needs an explicit update.
        let existing_manifest = load_snapshot_manifest(&repo_paths, &snapshot_id)?;
        ensure_same_metadata(&existing_manifest, &result.snapshot_manifest)?;
        let required_chunks = required_chunk_hashes(&existing_manifest);
        if required_chunks
            .iter()
            .all(|hash| reuse_state.available_chunk_sizes.contains_key(hash))
        {
            let counts = ArtifactCounts {
                required_chunks: required_chunks.len(),
                required_bundles: required_bundle_ids(
                    &existing_manifest,
                    &reuse_state.chunk_index,
                )?
                .len(),
                generated_chunks: 0,
                generated_bundles: 0,
                reused_chunks: required_chunks.len(),
            };
            write_chunk_index(&repo_paths, &reuse_state.chunk_index)?;
            return Ok(capture_result(
                repo_paths.repo_root,
                &existing_manifest,
                existing_manifest_path,
                SnapshotCaptureOutcome::Unchanged { snapshot_id },
                counts,
                timings,
            ));
        }
        // A damaged snapshot may be rebuilt with a different chunk layout. Publish
        // its new blueprint with its artifacts, retaining descriptive metadata.
        let rebuilt = mem::replace(&mut result.snapshot_manifest, existing_manifest);
        result.snapshot_manifest.chunker = request.chunker;
        result.snapshot_manifest.entries = rebuilt.entries;
        result.snapshot_manifest.total_compressed_size_estimate =
            rebuilt.total_compressed_size_estimate;
    }

    // Publication adds newly built chunks to reuse state. Count against the pre-publication state.
    let required_chunks = required_chunk_hashes(&result.snapshot_manifest);
    let required_chunk_count = required_chunks.len();
    let reused_chunk_count = required_chunks
        .into_iter()
        .filter(|chunk_id| reuse_state.available_chunk_sizes.contains_key(chunk_id))
        .count();

    let (manifest_path, required_bundle_count) =
        publish_snapshot_artifacts(control, &repo_paths, &result, reuse_state, build_pool)?;

    let outcome = if snapshot_already_existed {
        SnapshotCaptureOutcome::Unchanged { snapshot_id }
    } else {
        SnapshotCaptureOutcome::Created { snapshot_id }
    };
    let counts = ArtifactCounts {
        required_chunks: required_chunk_count,
        required_bundles: required_bundle_count,
        generated_chunks: result.loose_chunks.len(),
        generated_bundles: result.bundles.len(),
        reused_chunks: reused_chunk_count,
    };
    let capture_result = capture_result(
        repo_paths.repo_root,
        &result.snapshot_manifest,
        manifest_path,
        outcome,
        counts,
        timings,
    );
    capture_result.emit_done(control.progress());
    Ok(capture_result)
}

/// Configures the build to reuse the repository's chunks, bundles and
/// previous file manifests.
fn configure_build<'a>(
    request: &SnapshotCaptureRequest,
    reuse_state: &'a CaptureReuseState,
    repo_paths: &RepoPaths,
    snapshot_tags: &SnapshotTagSet,
) -> BuildConfig<'a> {
    let _span =
        request
            .control
            .profile_span_with_args("capture.prepare_source.configure_build", || {
                BTreeMap::from([
                    (
                        "existing_bundle_count".to_string(),
                        serde_json::json!(reuse_state.bundle_ids.len()),
                    ),
                    (
                        "existing_chunk_location_count".to_string(),
                        serde_json::json!(reuse_state.available_chunk_sizes.len()),
                    ),
                    (
                        "previous_file_manifest_path_count".to_string(),
                        serde_json::json!(reuse_state.previous_file_manifests.len()),
                    ),
                ])
            });
    let mut build_config = BuildConfig::for_snapshot(snapshot_tags)
        .with_existing_chunks(&reuse_state.available_chunk_sizes)
        .with_existing_bundles(&reuse_state.bundle_ids)
        .with_previous_manifests(&reuse_state.previous_file_manifests)
        .with_chunk_profile(request.chunk_profile)
        .with_chunker(request.chunker)
        .with_bundle_profile(request.bundle_profile)
        .with_compression(request.compression_algorithm, request.compression_level)
        .with_storage_mode(request.storage_mode)
        .with_runtime_hints(request.runtime_hints)
        .with_artifact_staging_root(repo_paths.bundles_root.clone());
    build_config.control = request.control.clone();
    build_config
}

/// How a capture used the repository's chunks and bundles.
struct ArtifactCounts {
    required_chunks: usize,
    required_bundles: usize,
    generated_chunks: usize,
    generated_bundles: usize,
    reused_chunks: usize,
}

fn capture_result(
    repo_root: PathBuf,
    manifest: &SnapshotManifest,
    manifest_path: PathBuf,
    outcome: SnapshotCaptureOutcome,
    counts: ArtifactCounts,
    build_timings: BuildTimings,
) -> SnapshotCaptureResult {
    let tag_value = |key: &str| {
        manifest
            .snapshot_tags
            .get(key)
            .unwrap_or_default()
            .to_string()
    };
    SnapshotCaptureResult {
        repo_root,
        snapshot_id: manifest.snapshot_id.clone(),
        outcome,
        manifest_path,
        version_tag: tag_value("version"),
        os: tag_value("os"),
        arch: tag_value("arch"),
        file_count: manifest.file_count(),
        total_logical_size: manifest.total_logical_size,
        total_compressed_size_estimate: manifest.total_compressed_size_estimate,
        required_chunk_count: counts.required_chunks,
        required_bundle_count: counts.required_bundles,
        generated_bundle_count: counts.generated_bundles,
        generated_chunk_count: counts.generated_chunks,
        reused_chunk_count: counts.reused_chunks,
        new_chunk_count: counts.required_chunks.saturating_sub(counts.reused_chunks),
        build_timings,
    }
}

/// Rejects a capture whose descriptive metadata differs from the existing
/// snapshot with the same content.
fn ensure_same_metadata(
    existing_manifest: &SnapshotManifest,
    requested_manifest: &SnapshotManifest,
) -> Result<()> {
    let differences = capture_metadata_differences(existing_manifest, requested_manifest);
    if differences.is_empty() {
        return Ok(());
    }
    Err(crate::Error::SnapshotCaptureConflict {
        outcome: SnapshotCaptureOutcome::MetadataConflict {
            snapshot_id: requested_manifest.snapshot_id.clone(),
            differences,
        },
    })
}

fn capture_metadata_differences(
    existing_manifest: &SnapshotManifest,
    requested_manifest: &SnapshotManifest,
) -> Vec<MetadataFieldDifference> {
    let mut differences = Vec::new();
    if existing_manifest.snapshot_tags != requested_manifest.snapshot_tags {
        differences.push(MetadataFieldDifference {
            field: String::from("snapshot_tags"),
            existing: existing_manifest
                .snapshot_tags
                .to_canonical_strings()
                .join(" "),
            requested: requested_manifest
                .snapshot_tags
                .to_canonical_strings()
                .join(" "),
        });
    }
    if existing_manifest.product_metadata != requested_manifest.product_metadata {
        differences.push(MetadataFieldDifference {
            field: String::from("product_metadata"),
            existing: metadata_display_value(existing_manifest.product_metadata.as_ref()),
            requested: metadata_display_value(requested_manifest.product_metadata.as_ref()),
        });
    }
    if existing_manifest.launch != requested_manifest.launch {
        differences.push(MetadataFieldDifference {
            field: String::from("launch"),
            existing: metadata_display_value(existing_manifest.launch.as_ref()),
            requested: metadata_display_value(requested_manifest.launch.as_ref()),
        });
    }
    differences
}

impl SnapshotCaptureResult {
    /// Emit a done event if a progress sink was provided.
    pub(crate) fn emit_done(&self, progress: &Option<Arc<dyn ProgressSink>>) {
        progress::emit_progress(
            progress,
            ProgressEvent {
                feedback: None,
                operation: OperationKind::SnapshotCapture,
                phase: ProgressPhase::Done,
                message: "Snapshot capture complete".to_string(),
                current: 1,
                total: 1,
                transferred_bytes: 0,
                total_bytes: 0,
                detail: Some(format!("snapshot_id={}", self.snapshot_id)),
                done: true,
            },
        );
    }
}

/// Builds the capture worker pool sized from `hints`, timed as one profile span.
pub(super) fn build_capture_pool(
    control: &OperationControl,
    hints: &RuntimeResourceHints,
) -> Result<rayon::ThreadPool> {
    let runtime_plan = plan_runtime(hints);
    let _pool_span = control.profile_span_with_args("capture.create_worker_pool", || {
        BTreeMap::from([(
            "worker_count".to_string(),
            serde_json::json!(runtime_plan.worker_count),
        )])
    });
    build_thread_pool(runtime_plan.worker_count, "Capture worker")
}

/// Captures read a directory; a single input file is staged into a private
/// temporary directory that lives as long as the returned value.
pub(super) fn prepare_input_path(path: &Path) -> Result<PreparedInputPath> {
    if path.is_dir() {
        return Ok(PreparedInputPath {
            dir: path.to_path_buf(),
            _tempdir: None,
        });
    }
    if path.is_file() {
        let tempdir = tempfile::Builder::new().prefix("gutch_input_").tempdir()?;
        let filename = path.file_name().ok_or_else(|| {
            crate::Error::invalid_input(format!("input file has no name: {}", path.display()))
        })?;
        let staged_path = tempdir.path().join(filename);
        fs::copy(path, &staged_path)?;
        return Ok(PreparedInputPath {
            dir: tempdir.path().to_path_buf(),
            _tempdir: Some(tempdir),
        });
    }
    Err(crate::Error::invalid_input(format!(
        "input path does not exist or is unsupported: {}",
        path.display()
    )))
}

pub(super) struct PreparedInputPath {
    pub(super) dir: PathBuf,
    _tempdir: Option<tempfile::TempDir>,
}

#[cfg(test)]
#[path = "execute_tests.rs"]
mod tests;
