use super::contracts::{
    MetadataFieldDifference, SnapshotCaptureOutcome, SnapshotCaptureRequest, SnapshotCaptureResult,
};
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use crate::artifact::hash::ChunkHash;
use crate::capture::reuse::CaptureReuseState;
use crate::capture::reuse::load_capture_reuse_state;
use crate::capture::scan::SourceInventory;
use crate::capture::scan::scan_source_inventory;
use crate::progress;
use crate::progress::GutchOperation;
use crate::progress::GutchProgressEvent;
use crate::progress::GutchProgressPhase;
use crate::progress::GutchProgressSink;
use crate::repo::Result;

use super::plan::plan_snapshot_capture_with_control;
fn metadata_display_value<T: serde::Serialize>(value: Option<&T>) -> String {
    match value {
        None => String::from("(absent)"),
        Some(value) => {
            serde_json::to_string(value).unwrap_or_else(|_| String::from("(unserializable)"))
        }
    }
}

pub fn capture_snapshot(request: SnapshotCaptureRequest) -> Result<SnapshotCaptureResult> {
    crate::manifest::profiles::validate_packaging_sizes(
        &crate::manifest::ChunkSizingProfile::for_preset(request.chunk_profile),
        &crate::manifest::BundleSizingProfile::for_preset(request.bundle_profile),
        request.storage_mode,
    )?;
    request.control.checkpoint()?;
    let operation_root = (&request.repo_root).to_path_buf();
    crate::repo::coordination::write_with_control(&operation_root, &request.control.clone(), || {
        request.control.checkpoint()?;
        let runtime_plan = crate::runtime::plan_runtime(&request.runtime_hints);
        let _pool_span =
            request
                .control
                .profile_span_with_args("capture.create_worker_pool", || {
                    std::collections::BTreeMap::from([(
                        "worker_count".to_string(),
                        serde_json::json!(runtime_plan.worker_count),
                    )])
                });
        let build_pool =
            crate::runtime::build_thread_pool(runtime_plan.worker_count, "Capture worker")?;
        drop(_pool_span);

        let repo_paths = crate::repo::paths::GutchRepoPaths::from_repo_root(&request.repo_root);
        let _existing_state_span = request
            .control
            .profile_span("capture.prepare_source.load_existing_store_state");
        let mut reuse_state = load_capture_reuse_state(&repo_paths, &build_pool, &request.control)?;
        drop(_existing_state_span);

        let (_, mut manifest) = crate::repo::paths::load_gutch_repo_manifest(&request.repo_root)?;
        manifest.set_artifact_profile(
            request.chunk_profile,
            request.compression_algorithm,
            request.compression_level,
            request.bundle_profile,
            request.storage_mode,
        );
        manifest.artifact_profile.chunker = request.chunker;
        let result = capture_repo_snapshot_with_existing_reuse_state(
            request,
            &mut reuse_state,
            &build_pool,
        )?;
        crate::repo::paths::write_gutch_repo_manifest(&repo_paths, &manifest)?;
        Ok(result)
    })
    .map_err(capture_error)
}

pub(crate) fn capture_repo_snapshot_with_existing_reuse_state(
    request: SnapshotCaptureRequest,
    reuse_state: &mut CaptureReuseState,
    build_pool: &rayon::ThreadPool,
) -> Result<SnapshotCaptureResult> {
    request.control.checkpoint()?;
    let operation_root = (&request.repo_root).to_path_buf();
    crate::repo::coordination::write_with_control(&operation_root, &request.control.clone(), || {
        plan_snapshot_capture_with_control(
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
    request.control.checkpoint()?;
    let _prepare_span = request.control.profile_span("capture.prepare_source");
    let _repo_state_span = request
        .control
        .profile_span("capture.prepare_source.load_repo_state");
    let (repo_paths, repo_manifest) =
        crate::repo::paths::load_gutch_repo_manifest(&request.repo_root)?;
    drop(_repo_state_span);

    let _input_span = request
        .control
        .profile_span("capture.prepare_source.prepare_input");
    let prepared_input = prepare_input_path(&request.source_dir)?;
    drop(_input_span);

    let _source_inventory_span = request
        .control
        .profile_span("capture.prepare_source.inventory_source");
    let runtime_plan = crate::runtime::plan_runtime(&request.runtime_hints);
    let source_inventory: SourceInventory = scan_source_inventory(
        &prepared_input.dir,
        &request.control,
        runtime_plan.worker_count,
        build_pool,
    )?;
    drop(_source_inventory_span);
    let constraints = repo_manifest.constraints.clone().unwrap_or_default();
    crate::capture::scan::validate_source_inventory(&source_inventory, &constraints)?;

    let _identity_span = request
        .control
        .profile_span("capture.prepare_source.resolve_tags");
    let normalized_tags = request.explicit_tags.normalized()?;
    constraints
        .validate_snapshot_tags(&normalized_tags, &request.source_dir.display().to_string())?;
    drop(_identity_span);

    let _existing_chunks_span = request.control.profile_span_with_args(
        "capture.prepare_source.collect_existing_chunk_ids",
        || {
            std::collections::BTreeMap::from([
                (
                    "existing_chunk_location_count".to_string(),
                    serde_json::json!(reuse_state.chunk_locations.len()),
                ),
                (
                    "previous_file_manifest_path_count".to_string(),
                    serde_json::json!(reuse_state.previous_file_manifests.len()),
                ),
            ])
        },
    );
    let existing_chunk_ids = reuse_state
        .chunk_locations
        .keys()
        .map(|chunk_hash| ChunkHash::parse_hex(chunk_hash))
        .collect::<Result<HashSet<_>>>()?;
    drop(_existing_chunks_span);

    let _config_span =
        request
            .control
            .profile_span_with_args("capture.prepare_source.configure_build", || {
                std::collections::BTreeMap::from([
                    (
                        "existing_bundle_count".to_string(),
                        serde_json::json!(reuse_state.bundle_ids.len()),
                    ),
                    (
                        "existing_chunk_location_count".to_string(),
                        serde_json::json!(reuse_state.chunk_locations.len()),
                    ),
                    (
                        "previous_file_manifest_path_count".to_string(),
                        serde_json::json!(reuse_state.previous_file_manifests.len()),
                    ),
                ])
            });
    let mut build_config = crate::capture::types::BuildConfig::for_snapshot(&normalized_tags)
        .with_existing_chunks(&reuse_state.chunk_locations)
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
    drop(_config_span);

    drop(_prepare_span);
    let super::build::BuiltCapture {
        result,
        timings,
        source_inventory,
    } = super::build::build_with_stable_source(
        &prepared_input.dir,
        &build_config,
        build_pool,
        source_inventory,
        &constraints,
        runtime_plan.worker_count,
    )?;
    let _publish_span = request.control.profile_span("capture.publish_snapshot");
    request.control.checkpoint()?;
    let mut result = result;
    let snapshot_id = result.snapshot_manifest.snapshot_id.clone();
    result.snapshot_manifest.source_modified_at =
        crate::snapshot::identity::format_system_time_utc(source_inventory.source_modified_at);
    result.snapshot_manifest.snapshot_tags = normalized_tags.clone();

    let existing_manifest_path =
        crate::repo::snapshot_manifests::snapshot_metadata_path(&repo_paths, &snapshot_id)?;
    let snapshot_already_existed = existing_manifest_path.exists();
    if snapshot_already_existed {
        let existing_manifest =
            crate::repo::snapshot_manifests::load_snapshot_manifest(&repo_paths, &snapshot_id)?;

        // The content-derived ID already exists. Compare metadata without
        // mutating anything: identical metadata returns unchanged, differing
        // metadata returns a typed conflict for an explicit update.
        let mut differences = Vec::new();
        if existing_manifest.snapshot_tags != result.snapshot_manifest.snapshot_tags {
            differences.push(MetadataFieldDifference {
                field: String::from("snapshot_tags"),
                existing: existing_manifest
                    .snapshot_tags
                    .to_canonical_strings()
                    .join(" "),
                requested: result
                    .snapshot_manifest
                    .snapshot_tags
                    .to_canonical_strings()
                    .join(" "),
            });
        }
        if existing_manifest.product_metadata != result.snapshot_manifest.product_metadata {
            differences.push(MetadataFieldDifference {
                field: String::from("product_metadata"),
                existing: metadata_display_value(existing_manifest.product_metadata.as_ref()),
                requested: metadata_display_value(
                    result.snapshot_manifest.product_metadata.as_ref(),
                ),
            });
        }
        if existing_manifest.launch != result.snapshot_manifest.launch {
            differences.push(MetadataFieldDifference {
                field: String::from("launch"),
                existing: metadata_display_value(existing_manifest.launch.as_ref()),
                requested: metadata_display_value(result.snapshot_manifest.launch.as_ref()),
            });
        }
        if !differences.is_empty() {
            return Err(crate::Error::SnapshotCaptureConflict {
                outcome: SnapshotCaptureOutcome::MetadataConflict {
                    snapshot_id: snapshot_id.clone(),
                    differences,
                },
            });
        }
        if crate::repo::index::required_chunk_hashes(&existing_manifest)
            .iter()
            .all(|hash| reuse_state.chunk_locations.contains_key(*hash))
        {
            let required_chunk_count =
                crate::repo::index::required_chunk_hashes(&existing_manifest).len();
            let required_bundle_count = crate::repo::index::required_bundle_ids(
                &existing_manifest,
                &reuse_state.chunk_index,
            )?
            .len();
            crate::repo::index::write_chunk_index(&repo_paths, &reuse_state.chunk_index)?;
            return Ok(SnapshotCaptureResult {
                repo_root: repo_paths.repo_root,
                snapshot_id: existing_manifest.snapshot_id.clone(),
                outcome: SnapshotCaptureOutcome::Unchanged {
                    snapshot_id: snapshot_id.clone(),
                },
                manifest_path: existing_manifest_path,
                version_tag: normalized_tags
                    .get("version")
                    .unwrap_or_default()
                    .to_string(),
                os: normalized_tags.get("os").unwrap_or_default().to_string(),
                arch: normalized_tags.get("arch").unwrap_or_default().to_string(),
                file_count: existing_manifest.file_count(),
                total_logical_size: existing_manifest.total_logical_size,
                total_compressed_size_estimate: existing_manifest.total_compressed_size_estimate,
                required_chunk_count,
                required_bundle_count,
                generated_bundle_count: 0,
                generated_chunk_count: 0,
                reused_chunk_count: required_chunk_count,
                new_chunk_count: 0,
                build_timings: timings,
            });
        }
        // A damaged snapshot may be rebuilt with a different chunk layout. Publish
        // its new blueprint with its artifacts, retaining descriptive metadata.
        let rebuilt_entries = std::mem::take(&mut result.snapshot_manifest.entries);
        let rebuilt_estimate = result.snapshot_manifest.total_compressed_size_estimate;
        result.snapshot_manifest = existing_manifest;
        result.snapshot_manifest.chunker = request.chunker;
        result.snapshot_manifest.entries = rebuilt_entries;
        result.snapshot_manifest.total_compressed_size_estimate = rebuilt_estimate;
    }

    let manifest_path = super::publish::publish_snapshot_artifacts(
        &request.control,
        &repo_paths,
        &result,
        reuse_state,
        build_pool,
    )?;
    let required_chunks =
        crate::repo::index::required_chunk_hashes_typed(&result.snapshot_manifest)?;
    let required_chunk_count = required_chunks.len();
    let required_bundle_count = count_required_bundles_for_build(
        &result.snapshot_manifest,
        &reuse_state.chunk_locations,
        &result.bundles,
    );
    let reused_chunk_count = required_chunks
        .iter()
        .filter(|chunk_id| existing_chunk_ids.contains(*chunk_id))
        .count();

    let capture_result = SnapshotCaptureResult {
        repo_root: repo_paths.repo_root,
        snapshot_id: result.snapshot_manifest.snapshot_id.clone(),
        outcome: if snapshot_already_existed {
            SnapshotCaptureOutcome::Unchanged {
                snapshot_id: result.snapshot_manifest.snapshot_id.clone(),
            }
        } else {
            SnapshotCaptureOutcome::Created {
                snapshot_id: result.snapshot_manifest.snapshot_id.clone(),
            }
        },
        manifest_path,
        version_tag: normalized_tags
            .get("version")
            .unwrap_or_default()
            .to_string(),
        os: normalized_tags.get("os").unwrap_or_default().to_string(),
        arch: normalized_tags.get("arch").unwrap_or_default().to_string(),
        file_count: result.snapshot_manifest.file_count(),
        total_logical_size: result.snapshot_manifest.total_logical_size,
        total_compressed_size_estimate: result.snapshot_manifest.total_compressed_size_estimate,
        required_chunk_count,
        required_bundle_count,
        generated_bundle_count: result.bundles.len(),
        generated_chunk_count: result.loose_chunks.len(),
        reused_chunk_count,
        new_chunk_count: required_chunk_count.saturating_sub(reused_chunk_count),
        build_timings: timings,
    };
    capture_result.emit_done(request.control.progress());
    Ok(capture_result)
}

impl SnapshotCaptureResult {
    /// Emit a done event if a progress sink was provided.
    pub(crate) fn emit_done(&self, progress: &Option<Arc<dyn GutchProgressSink>>) {
        progress::emit_progress(
            progress,
            GutchProgressEvent {
                feedback: None,
                operation: GutchOperation::SnapshotCapture,
                phase: GutchProgressPhase::Done,
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

fn prepare_input_path(path: &Path) -> Result<PreparedInputPath> {
    if path.is_dir() {
        return Ok(PreparedInputPath {
            dir: path.to_path_buf(),
            _tempdir: None,
        });
    }
    if path.is_file() {
        let tempdir = tempfile::Builder::new().prefix("gutch_input_").tempdir()?;
        let filename = path.file_name().ok_or_else(|| {
            crate::Error::manifest(format!("input file has no name: {}", path.display()))
        })?;
        let staged_path = tempdir.path().join(filename);
        std::fs::copy(path, &staged_path)?;
        return Ok(PreparedInputPath {
            dir: tempdir.path().to_path_buf(),
            _tempdir: Some(tempdir),
        });
    }
    Err(crate::Error::manifest(format!(
        "input path does not exist or is unsupported: {}",
        path.display()
    )))
}

fn count_required_bundles_for_build(
    snapshot_manifest: &crate::manifest::snapshot::SnapshotManifest,
    existing_chunk_locations: &HashMap<String, crate::capture::types::ExistingChunkLocation>,
    generated_bundles: &[crate::capture::types::BuiltBundle],
) -> usize {
    let existing_chunk_locations_by_hash = existing_chunk_locations
        .iter()
        .filter_map(|(chunk_hash, location)| {
            ChunkHash::parse_hex(chunk_hash)
                .ok()
                .map(|chunk_hash| (chunk_hash, location))
        })
        .collect::<HashMap<_, _>>();
    let mut required_bundle_ids = HashSet::new();
    let Ok(required_chunks) = crate::repo::index::required_chunk_hashes_typed(snapshot_manifest)
    else {
        return generated_bundles.len();
    };
    for chunk_hash in required_chunks {
        if let Some(existing) = existing_chunk_locations_by_hash.get(&chunk_hash)
            && existing.storage_mode == crate::manifest::ArtifactStorageMode::Bundled
        {
            required_bundle_ids.insert(existing.bundle_id.clone());
        }
    }
    required_bundle_ids.len() + generated_bundles.len()
}

struct PreparedInputPath {
    dir: PathBuf,
    _tempdir: Option<tempfile::TempDir>,
}

#[cfg(test)]
#[path = "execute_tests.rs"]
mod tests;
