use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use rayon::prelude::*;
use serde::Deserialize;
use serde::Serialize;

use crate::artifact::hash::ChunkHash;
use crate::capture::reuse::CaptureReuseState;
use crate::capture::reuse::load_capture_reuse_state;
use crate::capture::scan::SourceInventory;
use crate::capture::scan::scan_source_inventory;
use crate::manifest::SnapshotTagSet;
use crate::progress;
use crate::progress::GutchOperation;
use crate::progress::GutchProgressEvent;
use crate::progress::GutchProgressPhase;
use crate::progress::GutchProgressSink;
use crate::repo::Result;
use crate::runtime::RuntimeResourceHints;

use super::plan::*;
/// One differing metadata field between a capture candidate and the stored
/// snapshot with the same content-derived ID.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MetadataFieldDifference {
    pub field: String,
    pub existing: String,
    pub requested: String,
}

/// Typed outcome when capture computes a content-derived snapshot ID that
/// already exists in the repo.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotCaptureOutcome {
    Created {
        snapshot_id: String,
    },
    Unchanged {
        snapshot_id: String,
    },
    MetadataConflict {
        snapshot_id: String,
        differences: Vec<MetadataFieldDifference>,
    },
    IntegrityConflict {
        snapshot_id: String,
    },
}

impl SnapshotCaptureOutcome {
    /// Caller-facing wording for the outcome. Adapters render this message
    /// without inferring their own tag-merging or overwrite policy.
    pub fn message(&self) -> String {
        match self {
            SnapshotCaptureOutcome::Created { snapshot_id } => {
                format!("Captured snapshot {snapshot_id}.")
            }
            SnapshotCaptureOutcome::Unchanged { .. } => {
                String::from("Snapshot content already exists; no changes were made.")
            }
            SnapshotCaptureOutcome::MetadataConflict {
                snapshot_id,
                differences,
            } => {
                let mut message = format!("snapshot_id: {snapshot_id}");
                for difference in differences {
                    message.push_str(&format!(
                        "\nfield '{}': existing '{}', requested '{}'",
                        difference.field, difference.existing, difference.requested
                    ));
                }
                message.push_str(
                    "\nCapture did not change the existing snapshot. Use an explicit metadata update for this snapshot ID.",
                );
                message
            }
            SnapshotCaptureOutcome::IntegrityConflict { snapshot_id } => {
                format!(
                    "Existing snapshot data does not match its content-derived ID. The repository may be corrupt.\nsnapshot_id: {snapshot_id}"
                )
            }
        }
    }
}

impl std::fmt::Display for SnapshotCaptureOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotCapturePlan {
    pub folder_name: String,
    pub path: String,
    pub version_tag: String,
    pub os: String,
    pub arch: String,
    #[serde(rename = "tags")]
    pub snapshot_tags: SnapshotTagSet,
}

#[derive(Debug, Clone)]
pub struct SnapshotCaptureRequest {
    pub repo_root: PathBuf,
    pub source_dir: PathBuf,
    pub explicit_tags: SnapshotTagSet,
    pub chunk_profile: crate::manifest::ChunkSizingPreset,
    pub bundle_profile: crate::manifest::BundleSizingPreset,
    pub compression_algorithm: crate::manifest::CompressionKind,
    pub compression_level: Option<i32>,
    pub storage_mode: crate::manifest::ArtifactStorageMode,
    pub runtime_hints: RuntimeResourceHints,
    pub control: crate::progress::GutchOperationControl,
}

#[derive(Debug, Clone)]
pub struct SnapshotCaptureResult {
    pub repo_root: PathBuf,
    pub snapshot_id: String,
    pub outcome: SnapshotCaptureOutcome,
    pub manifest_path: PathBuf,
    pub version_tag: String,
    pub os: String,
    pub arch: String,
    pub file_count: usize,
    pub total_logical_size: u64,
    pub total_compressed_size_estimate: u64,
    pub required_chunk_count: usize,
    pub required_bundle_count: usize,
    pub generated_bundle_count: usize,
    pub generated_chunk_count: usize,
    pub reused_chunk_count: usize,
    pub new_chunk_count: usize,
    pub(crate) build_timings: crate::capture::types::BuildTimings,
}

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
        let result = capture_repo_snapshot_with_existing_reuse_state(
            request,
            &mut reuse_state,
            &build_pool,
        )?;
        crate::repo::paths::write_gutch_repo_manifest(&repo_paths, &manifest)?;
        Ok(result)
    })
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
    let validate_inventory = |inventory: &SourceInventory| {
        crate::filesystem::constraints::validate_paths(
            &constraints,
            inventory
                .metadata_entries
                .iter()
                .map(|entry| entry.path())
                .chain(
                    inventory
                        .files
                        .iter()
                        .map(|file| file.normalized_relative.as_str()),
                ),
            "capture",
            None,
        )
    };
    validate_inventory(&source_inventory)?;

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
        .with_bundle_profile(request.bundle_profile)
        .with_compression(request.compression_algorithm, request.compression_level)
        .with_storage_mode(request.storage_mode)
        .with_runtime_hints(request.runtime_hints)
        .with_artifact_staging_root(repo_paths.bundles_root.clone());
    build_config.control = request.control.clone();
    drop(_config_span);

    drop(_prepare_span);
    let (mut result, mut timings) =
        crate::capture::pipeline::build_with_config_timed_on_pool_with_inventory(
            &prepared_input.dir,
            &build_config,
            build_pool,
            &source_inventory,
        )?;
    // The source may have changed while its bytes were read. Compare fresh
    // observations against the pre-read inventory, retry once with fresh
    // observations, and fail without publishing anything on a second mismatch.
    let mut source_inventory = source_inventory;
    let stable_after_build = scan_source_inventory(
        &prepared_input.dir,
        &request.control,
        runtime_plan.worker_count,
        build_pool,
    )
    .and_then(|fresh| {
        crate::capture::scan::verify_source_inventory_stable(&source_inventory, &fresh)?;
        Ok(fresh)
    });
    source_inventory = match stable_after_build {
        Ok(fresh) => fresh,
        Err(_) => {
            source_inventory = scan_source_inventory(
                &prepared_input.dir,
                &request.control,
                runtime_plan.worker_count,
                build_pool,
            )?;
            validate_inventory(&source_inventory)?;
            (result, timings) =
                crate::capture::pipeline::build_with_config_timed_on_pool_with_inventory(
                    &prepared_input.dir,
                    &build_config,
                    build_pool,
                    &source_inventory,
                )?;
            let fresh = scan_source_inventory(
                &prepared_input.dir,
                &request.control,
                runtime_plan.worker_count,
                build_pool,
            )?;
            crate::capture::scan::verify_source_inventory_stable(&source_inventory, &fresh)?;
            fresh
        }
    };
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
        result.snapshot_manifest.entries = rebuilt_entries;
        result.snapshot_manifest.total_compressed_size_estimate = rebuilt_estimate;
    }

    // Snapshot publication below persists the captured snapshot. The snapshot
    // manifest is published last as the commit marker: immutable bundles and
    // loose chunks go first, then the additive chunk index, then the snapshot
    // directory. A failure before the manifest write leaves only unreferenced
    // immutable artifacts, never a visible snapshot that cannot be
    // reconstructed. Check once before the write set.
    request.control.checkpoint()?;
    let _artifact_write_span = request
        .control
        .profile_span("capture.publish_snapshot.write_artifacts");
    write_bundles(
        &repo_paths.bundles_root,
        &result.bundles,
        build_pool,
        &request.control,
    )?;
    write_loose_chunks(
        &repo_paths.chunks_root,
        &result.loose_chunks,
        build_pool,
        &request.control,
    )?;
    drop(_artifact_write_span);
    let _index_span = request
        .control
        .profile_span("capture.publish_snapshot.update_chunk_index");
    {
        let _bundle_index_span = request
            .control
            .profile_span("capture.publish_snapshot.index_bundle_entries");
        crate::capture::reuse::add_built_bundles_to_chunk_index(
            &mut reuse_state.chunk_index,
            &result.bundles,
        )?;
    }
    {
        let _chunk_index_span = request
            .control
            .profile_span("capture.publish_snapshot.index_loose_chunk_entries");
        crate::capture::reuse::add_built_loose_chunks_to_chunk_index(
            &mut reuse_state.chunk_index,
            &result.loose_chunks,
        )?;
    }
    drop(_index_span);

    let _state_span = request
        .control
        .profile_span("capture.publish_snapshot.update_capture_state");
    for bundle in &result.bundles {
        reuse_state.bundle_ids.insert(bundle.bundle_id.clone());
        for chunk in &bundle.entries {
            reuse_state.chunk_locations.insert(
                chunk.chunk_hash.to_hex(),
                crate::capture::types::ExistingChunkLocation {
                    bundle_id: bundle.bundle_id.clone(),
                    compressed_size: u64::from(chunk.compressed_size),
                    storage_mode: crate::manifest::ArtifactStorageMode::Bundled,
                },
            );
        }
    }
    for chunk in &result.loose_chunks {
        let chunk_hash = chunk.chunk_hash.to_string();
        reuse_state.chunk_locations.insert(
            chunk_hash.clone(),
            crate::capture::types::ExistingChunkLocation {
                bundle_id: chunk_hash,
                compressed_size: chunk.compressed_size,
                storage_mode: crate::manifest::ArtifactStorageMode::LooseChunks,
            },
        );
    }
    for file in result.snapshot_manifest.files() {
        let candidates = reuse_state
            .previous_file_manifests
            .entry(file.path.clone())
            .or_default();
        if !candidates.iter().any(|candidate| {
            candidate.size == file.size && candidate.full_file_hash == file.full_file_hash
        }) {
            candidates.push(file.clone());
        }
    }
    let _chunk_index_write_span = request
        .control
        .profile_span("capture.publish_snapshot.write_chunk_index");
    crate::repo::index::write_chunk_index(&repo_paths, &reuse_state.chunk_index)?;
    drop(_chunk_index_write_span);
    for artifact in crate::repo::artifacts::required_artifacts(
        &result.snapshot_manifest,
        &reuse_state.chunk_index,
    )? {
        if !artifact.local_path(&repo_paths)?.is_file() {
            return Err(crate::Error::manifest(format!(
                "capture is missing required artifact '{}'",
                artifact.id()
            )));
        }
    }
    let _manifest_write_span = request
        .control
        .profile_span("capture.publish_snapshot.write_snapshot_manifest");
    let (manifest_path, _) = crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts(
        &repo_paths,
        &result.snapshot_manifest,
    )?;
    drop(_manifest_write_span);
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

fn write_bundles(
    bundles_root: &Path,
    bundles: &[crate::capture::types::BuiltBundle],
    pool: &rayon::ThreadPool,
    control: &crate::progress::GutchOperationControl,
) -> Result<()> {
    let _phase_span =
        control.profile_span_with_args("capture.publish_snapshot.write_bundle_artifacts", || {
            capture_count_bytes_trace_args(
                bundles.len(),
                bundles.iter().map(|bundle| bundle.size).sum(),
            )
        });
    let control_for_work = control.clone();
    pool.install(|| {
        bundles
            .par_iter()
            .map(|bundle| {
                control_for_work.checkpoint()?;
                let bundle_path =
                    crate::repo::paths::bundle_path_in_root(bundles_root, &bundle.bundle_id)?;
                let _bundle_span =
                    control_for_work.profile_span_with_args("capture.bundle.publish", || {
                        capture_artifact_write_trace_args(
                            "bundle",
                            &bundle.bundle_id,
                            &bundle_path,
                            bundle.size,
                        )
                    });
                if let Some(parent) = bundle_path.parent() {
                    crate::filesystem::create_dir_all(parent)?;
                }
                if bundle_path.exists() {
                    bundle.discard_staged_file();
                } else {
                    bundle.staged_file.promote_to(&bundle_path)?;
                }
                Ok(())
            })
            .collect::<Result<Vec<_>>>()
    })?;
    Ok(())
}

fn write_loose_chunks(
    chunks_root: &Path,
    chunks: &[crate::capture::types::BuiltLooseChunk],
    pool: &rayon::ThreadPool,
    control: &crate::progress::GutchOperationControl,
) -> Result<()> {
    let total_bytes = chunks
        .iter()
        .map(|chunk| chunk.compressed_size)
        .sum::<u64>();
    let _phase_span = control.profile_span_with_args(
        "capture.publish_snapshot.write_loose_chunk_artifacts",
        || capture_count_bytes_trace_args(chunks.len(), total_bytes),
    );
    let control_for_work = control.clone();
    pool.install(|| {
        chunks
            .par_iter()
            .map(|chunk| {
                control_for_work.checkpoint()?;
                let chunk_hash = chunk.chunk_hash.to_string();
                let chunk_path = crate::repo::paths::chunk_path_in_root(chunks_root, &chunk_hash)?;
                if let Some(parent) = chunk_path.parent() {
                    crate::filesystem::create_dir_all(parent)?;
                }

                chunk.staged_file.promote_to(&chunk_path)?;
                Ok(())
            })
            .collect::<Result<Vec<_>>>()
    })?;
    Ok(())
}

fn capture_count_bytes_trace_args(
    count: usize,
    bytes: u64,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    std::collections::BTreeMap::from([
        ("count".to_string(), serde_json::json!(count)),
        ("size_bytes".to_string(), serde_json::json!(bytes)),
    ])
}

fn capture_artifact_write_trace_args(
    kind: &str,
    artifact_id: &str,
    path: &Path,
    bytes: u64,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut args = crate::profile::file_trace_args(path.display().to_string(), bytes);
    args.insert("artifact_kind".to_string(), serde_json::json!(kind));
    args.insert("artifact_id".to_string(), serde_json::json!(artifact_id));
    args
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
mod tests {
    use super::*;
    use crate::manifest;
    use crate::manifest::snapshot::SnapshotManifest;
    use crate::repo::paths::GutchRepoPaths;
    use crate::repo::paths::ensure_gutch_repo_layout;
    use crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts;
    use crate::repo::snapshots::retag_snapshot_by_id;
    use crate::repo::status::build_source_snapshot_ops_candidate;
    use crate::snapshot::identity::compute_snapshot_id_from_entries;

    #[test]
    fn plan_snapshot_capture_keeps_tagless_snapshots_distinct() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let source_root = temp.path().join("sources");
        let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
        ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

        let new_source = source_root.join("game-v1.0.0-windows-x86_64-base-2026_07_14");
        std::fs::create_dir_all(&new_source).unwrap();
        std::fs::write(new_source.join("game.exe"), b"old-content").unwrap();

        let old_candidate =
            build_source_snapshot_ops_candidate(&new_source, &manifest::SnapshotTagSet::default())
                .unwrap();
        let old_files = vec![manifest::FileManifest {
            path: "game.exe".into(),
            size: 11,
            executable: false,
            read_only: false,
            full_file_hash: crate::artifact::hash::hash_hex(
                &crate::file_integrity::hash_file(new_source.join("game.exe").as_path()).unwrap(),
            ),
            chunks: vec![crate::manifest::ChunkRef {
                chunk_hash: crate::artifact::hash::hash_hex(
                    &crate::file_integrity::hash_file(new_source.join("game.exe").as_path())
                        .unwrap(),
                ),
                raw_size: 11,
            }],
        }];
        let old_entries = old_files
            .iter()
            .cloned()
            .map(Into::into)
            .collect::<Vec<_>>();
        let old_snapshot_id = compute_snapshot_id_from_entries(&old_entries);
        let mut old_manifest =
            SnapshotManifest::new(old_snapshot_id.clone(), old_candidate.snapshot_tags.clone());
        old_manifest.source_modified_at =
            crate::snapshot::identity::source_modified_at_from_directory(&new_source).unwrap();
        old_manifest.set_files(old_files);
        write_snapshot_manifest_artifacts(&repo_paths, &old_manifest).unwrap();

        std::fs::write(new_source.join("game.exe"), b"new-content").unwrap();

        let plan = plan_snapshot_capture(
            &repo_root,
            &new_source,
            &manifest::SnapshotTagSet::default(),
        )
        .unwrap();
        assert_eq!(plan.snapshot_tags, manifest::SnapshotTagSet::default());
    }

    #[derive(Debug)]
    struct CancelledHook;

    impl crate::progress::GutchCancellationHook for CancelledHook {
        fn is_cancelled(&self) -> bool {
            true
        }
    }

    fn test_tags() -> manifest::SnapshotTagSet {
        manifest::SnapshotTagSet::from_compact_strings(&[
            "version=1.0.0".to_string(),
            "os=windows".to_string(),
        ])
        .unwrap()
    }

    fn test_capture(
        repo_root: &std::path::Path,
        source_dir: &std::path::Path,
        tags: &manifest::SnapshotTagSet,
        control: crate::progress::GutchOperationControl,
    ) -> SnapshotCaptureResult {
        capture_snapshot(SnapshotCaptureRequest {
            repo_root: repo_root.to_path_buf(),
            source_dir: source_dir.to_path_buf(),
            explicit_tags: tags.clone(),
            chunk_profile: manifest::ChunkSizingPreset::Chunk128KiB,
            bundle_profile: manifest::BundleSizingPreset::Bundle8MiB,
            compression_algorithm: manifest::CompressionKind::Zstd,
            compression_level: None,
            storage_mode: manifest::ArtifactStorageMode::Bundled,
            runtime_hints: crate::runtime::RuntimeResourceHints::default(),
            control,
        })
        .unwrap()
    }

    fn test_repo_with_source(
        temp: &tempfile::TempDir,
        source_name: &str,
        content: &[u8],
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        let repo_root = temp.path().join("repo");
        let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
        crate::repo::paths::ensure_gutch_repo_layout(&repo_paths, "base").unwrap();
        let source_dir = temp.path().join(source_name);
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join("game.exe"), content).unwrap();
        (repo_root, source_dir)
    }

    fn test_reconstruct_and_strict_verify(repo_root: &std::path::Path, snapshot_id: &str) {
        let destination = repo_root.join(format!("reconstructed-{snapshot_id}"));
        crate::installation::reconstruct_snapshot(crate::installation::InstallationRequest {
            repo_path: repo_root.to_path_buf(),
            snapshot_id: snapshot_id.to_string(),
            destination_dir: destination.clone(),
            staging_dir: None,
            source_snapshot_id: None,
            source_dir: None,
            runtime_hints: crate::runtime::RuntimeResourceHints::default(),
            control: crate::progress::GutchOperationControl::default(),
        })
        .unwrap();
        assert!(destination.join("game.exe").is_file());
        let verification = crate::repo::verify::verify_gutch_repo_artifacts(
            repo_root,
            Some(snapshot_id),
            crate::installation::inspect::VerifyMode::Strict,
        )
        .unwrap();
        assert!(verification.ok, "strict verification failed");
    }

    #[test]
    fn same_tags_create_distinct_snapshots() {
        let temp = tempfile::tempdir().unwrap();
        let (repo_root, source_a) = test_repo_with_source(&temp, "game-a", b"content-a");
        let source_b = temp.path().join("game-b");
        std::fs::create_dir_all(&source_b).unwrap();
        std::fs::write(source_b.join("game.exe"), b"content-b").unwrap();

        let first = test_capture(
            &repo_root,
            &source_a,
            &test_tags(),
            crate::progress::GutchOperationControl::default(),
        );
        assert!(matches!(
            first.outcome,
            SnapshotCaptureOutcome::Created { .. }
        ));
        let second = test_capture(
            &repo_root,
            &source_b,
            &test_tags(),
            crate::progress::GutchOperationControl::default(),
        );
        assert!(matches!(
            second.outcome,
            SnapshotCaptureOutcome::Created { .. }
        ));
        assert_ne!(first.snapshot_id, second.snapshot_id);

        for snapshot_id in [&first.snapshot_id, &second.snapshot_id] {
            let (repo_paths, _) = crate::repo::paths::load_gutch_repo_manifest(&repo_root).unwrap();
            crate::repo::snapshot_manifests::load_snapshot_manifest(&repo_paths, snapshot_id)
                .unwrap();
            test_reconstruct_and_strict_verify(&repo_root, snapshot_id);
        }
    }

    #[test]
    fn cancelled_capture_retains_first_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let (repo_root, source_a) = test_repo_with_source(&temp, "game-a", b"content-a");
        let source_b = temp.path().join("game-b");
        std::fs::create_dir_all(&source_b).unwrap();
        std::fs::write(source_b.join("game.exe"), b"content-b").unwrap();

        let first = test_capture(
            &repo_root,
            &source_a,
            &test_tags(),
            crate::progress::GutchOperationControl::default(),
        );
        let cancelled = crate::progress::GutchOperationControl::default()
            .with_cancellation(std::sync::Arc::new(CancelledHook));
        let failed = capture_snapshot(SnapshotCaptureRequest {
            repo_root: repo_root.clone(),
            source_dir: source_b,
            explicit_tags: test_tags(),
            chunk_profile: manifest::ChunkSizingPreset::Chunk128KiB,
            bundle_profile: manifest::BundleSizingPreset::Bundle8MiB,
            compression_algorithm: manifest::CompressionKind::Zstd,
            compression_level: None,
            storage_mode: manifest::ArtifactStorageMode::Bundled,
            runtime_hints: crate::runtime::RuntimeResourceHints::default(),
            control: cancelled,
        });
        assert!(failed.is_err());

        let (repo_paths, _) = crate::repo::paths::load_gutch_repo_manifest(&repo_root).unwrap();
        let retained = crate::repo::snapshot_manifests::load_snapshot_manifest(
            &repo_paths,
            &first.snapshot_id,
        )
        .unwrap();
        assert_eq!(retained.snapshot_tags, test_tags());
        let status = crate::repo::read_repo_status(&repo_root).unwrap();
        assert_eq!(status.snapshots.len(), 1);
        test_reconstruct_and_strict_verify(&repo_root, &first.snapshot_id);
    }

    #[test]
    fn identical_recapture_returns_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let (repo_root, source) = test_repo_with_source(&temp, "game", b"content");

        let first = test_capture(
            &repo_root,
            &source,
            &test_tags(),
            crate::progress::GutchOperationControl::default(),
        );
        let second = test_capture(
            &repo_root,
            &source,
            &test_tags(),
            crate::progress::GutchOperationControl::default(),
        );
        assert_eq!(first.snapshot_id, second.snapshot_id);
        assert!(matches!(
            second.outcome,
            SnapshotCaptureOutcome::Unchanged { .. }
        ));
        assert_eq!(second.generated_bundle_count, 0);
        assert_eq!(second.generated_chunk_count, 0);

        let status = crate::repo::read_repo_status(&repo_root).unwrap();
        assert_eq!(status.snapshots.len(), 1);
    }

    #[test]
    fn same_id_different_tags_returns_metadata_conflict() {
        let temp = tempfile::tempdir().unwrap();
        let (repo_root, source) = test_repo_with_source(&temp, "game", b"content");

        let first = test_capture(
            &repo_root,
            &source,
            &test_tags(),
            crate::progress::GutchOperationControl::default(),
        );
        let other_tags = manifest::SnapshotTagSet::from_compact_strings(&[
            "version=2.0.0".to_string(),
            "os=windows".to_string(),
        ])
        .unwrap();
        let conflict = capture_snapshot(SnapshotCaptureRequest {
            repo_root: repo_root.clone(),
            source_dir: source,
            explicit_tags: other_tags,
            chunk_profile: manifest::ChunkSizingPreset::Chunk128KiB,
            bundle_profile: manifest::BundleSizingPreset::Bundle8MiB,
            compression_algorithm: manifest::CompressionKind::Zstd,
            compression_level: None,
            storage_mode: manifest::ArtifactStorageMode::Bundled,
            runtime_hints: crate::runtime::RuntimeResourceHints::default(),
            control: crate::progress::GutchOperationControl::default(),
        });
        let Err(crate::Error::SnapshotCaptureConflict { outcome }) = conflict else {
            panic!("expected a metadata conflict, got {conflict:?}");
        };
        let SnapshotCaptureOutcome::MetadataConflict {
            snapshot_id,
            differences,
        } = outcome
        else {
            panic!("expected a metadata conflict outcome");
        };
        assert_eq!(snapshot_id, first.snapshot_id);
        assert!(
            differences
                .iter()
                .any(|difference| difference.field == "snapshot_tags")
        );

        let (repo_paths, _) = crate::repo::paths::load_gutch_repo_manifest(&repo_root).unwrap();
        let stored = crate::repo::snapshot_manifests::load_snapshot_manifest(
            &repo_paths,
            &first.snapshot_id,
        )
        .unwrap();
        assert_eq!(stored.snapshot_tags, test_tags());
    }

    #[test]
    fn explicit_retag_reports_previous_and_new_tags() {
        let temp = tempfile::tempdir().unwrap();
        let (repo_root, source) = test_repo_with_source(&temp, "game", b"content");

        let first = test_capture(
            &repo_root,
            &source,
            &test_tags(),
            crate::progress::GutchOperationControl::default(),
        );
        let new_tags = manifest::SnapshotTagSet::from_compact_strings(&[
            "version=2.0.0".to_string(),
            "os=windows".to_string(),
        ])
        .unwrap();
        let retagged =
            retag_snapshot_by_id(&repo_root, &first.snapshot_id, new_tags.clone()).unwrap();
        assert_eq!(retagged.snapshot_id, first.snapshot_id);
        assert_eq!(retagged.previous_tags, test_tags());
        assert_eq!(retagged.new_tags, new_tags);

        let (repo_paths, _) = crate::repo::paths::load_gutch_repo_manifest(&repo_root).unwrap();
        let stored = crate::repo::snapshot_manifests::load_snapshot_manifest(
            &repo_paths,
            &first.snapshot_id,
        )
        .unwrap();
        assert_eq!(stored.snapshot_tags, new_tags);
    }

    #[test]
    fn tampered_blueprint_returns_integrity_conflict() {
        let temp = tempfile::tempdir().unwrap();
        let (repo_root, source) = test_repo_with_source(&temp, "game", b"content");

        let first = test_capture(
            &repo_root,
            &source,
            &test_tags(),
            crate::progress::GutchOperationControl::default(),
        );
        let (repo_paths, _) = crate::repo::paths::load_gutch_repo_manifest(&repo_root).unwrap();
        let mut stored = crate::repo::snapshot_manifests::load_snapshot_manifest(
            &repo_paths,
            &first.snapshot_id,
        )
        .unwrap();
        stored.entries.clear();
        crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts(&repo_paths, &stored)
            .unwrap();

        let conflict = capture_snapshot(SnapshotCaptureRequest {
            repo_root: repo_root.clone(),
            source_dir: source,
            explicit_tags: test_tags(),
            chunk_profile: manifest::ChunkSizingPreset::Chunk128KiB,
            bundle_profile: manifest::BundleSizingPreset::Bundle8MiB,
            compression_algorithm: manifest::CompressionKind::Zstd,
            compression_level: None,
            storage_mode: manifest::ArtifactStorageMode::Bundled,
            runtime_hints: crate::runtime::RuntimeResourceHints::default(),
            control: crate::progress::GutchOperationControl::default(),
        });
        let Err(crate::Error::SnapshotCaptureConflict { outcome }) = conflict else {
            panic!("expected an integrity conflict, got {conflict:?}");
        };
        assert!(matches!(
            outcome,
            SnapshotCaptureOutcome::IntegrityConflict { .. }
        ));
    }

    #[test]
    fn content_change_with_frozen_mtime_creates_new_snapshot() {
        use std::time::SystemTime;

        let temp = tempfile::tempdir().unwrap();
        let (repo_root, source_a) = test_repo_with_source(&temp, "game-a", b"content-a");
        let source_b = temp.path().join("game-b");
        std::fs::create_dir_all(&source_b).unwrap();
        std::fs::write(source_b.join("game.exe"), b"content-b").unwrap();

        // Freeze modification times: snapshot identity must follow content,
        // never timestamps.
        let frozen = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        for source in [&source_a, &source_b] {
            std::fs::File::options()
                .write(true)
                .open(source.join("game.exe"))
                .unwrap()
                .set_modified(frozen)
                .unwrap();
        }

        let first = test_capture(
            &repo_root,
            &source_a,
            &test_tags(),
            crate::progress::GutchOperationControl::default(),
        );
        let second = test_capture(
            &repo_root,
            &source_b,
            &test_tags(),
            crate::progress::GutchOperationControl::default(),
        );
        assert_ne!(first.snapshot_id, second.snapshot_id);

        let status = crate::repo::read_repo_status(&repo_root).unwrap();
        assert_eq!(status.snapshots.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn failed_artifact_write_publishes_no_snapshot() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let (repo_root, source_a) = test_repo_with_source(&temp, "game-a", b"content-a");
        let source_b = temp.path().join("game-b");
        std::fs::create_dir_all(&source_b).unwrap();
        std::fs::write(source_b.join("game.exe"), b"content-b").unwrap();

        let first = test_capture(
            &repo_root,
            &source_a,
            &test_tags(),
            crate::progress::GutchOperationControl::default(),
        );

        let repo_paths = crate::repo::paths::GutchRepoPaths::from_repo_root(&repo_root);
        let read_only = std::fs::Permissions::from_mode(0o555);
        std::fs::set_permissions(&repo_paths.bundles_root, read_only.clone()).unwrap();
        std::fs::set_permissions(&repo_paths.chunks_root, read_only).unwrap();
        let failed = capture_snapshot(SnapshotCaptureRequest {
            repo_root: repo_root.clone(),
            source_dir: source_b,
            explicit_tags: test_tags(),
            chunk_profile: manifest::ChunkSizingPreset::Chunk128KiB,
            bundle_profile: manifest::BundleSizingPreset::Bundle8MiB,
            compression_algorithm: manifest::CompressionKind::Zstd,
            compression_level: None,
            storage_mode: manifest::ArtifactStorageMode::Bundled,
            runtime_hints: crate::runtime::RuntimeResourceHints::default(),
            control: crate::progress::GutchOperationControl::default(),
        });
        let read_write = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&repo_paths.bundles_root, read_write.clone()).unwrap();
        std::fs::set_permissions(&repo_paths.chunks_root, read_write).unwrap();
        assert!(failed.is_err());

        // No partial snapshot became visible and the first snapshot still
        // verifies and reconstructs.
        let status = crate::repo::read_repo_status(&repo_root).unwrap();
        assert_eq!(status.snapshots.len(), 1);
        assert_eq!(status.snapshots[0].snapshot_id, first.snapshot_id);
        test_reconstruct_and_strict_verify(&repo_root, &first.snapshot_id);
    }
}
