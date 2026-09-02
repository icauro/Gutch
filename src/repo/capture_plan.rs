use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{
    ExistingStoreState, Result, SourceSnapshotOperationCandidate,
    check::build_source_snapshot_ops_candidate, check_gutch_repo, remove_snapshot_from_repo,
    scan_existing_store_state,
};
use crate::hash::ChunkHash;
use crate::manifest::SnapshotTagSet;
use crate::progress::{
    self, GutchOperation, GutchProgressEvent, GutchProgressPhase, GutchProgressSink,
};
use crate::runtime::RuntimeResourceHints;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotCaptureKind {
    CaptureNew,
    Relabel,
    ReplaceSlot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotCapturePlan {
    pub kind: SnapshotCaptureKind,
    pub snapshot_id: String,
    pub folder_name: String,
    pub path: String,
    pub version_id: String,
    pub os: String,
    pub arch: String,
    pub snapshot_tags: SnapshotTagSet,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_tags: Option<SnapshotTagSet>,
    pub replaced_snapshot_ids: Vec<String>,
    pub replaces_existing_slots: bool,
}

#[derive(Debug, Clone)]
pub struct RepoSnapshotCaptureRequest {
    pub repo_root: PathBuf,
    pub source_dir: PathBuf,
    pub explicit_tags: SnapshotTagSet,
    /// Whether name-based tag inference may fill tags missing from
    /// `explicit_tags`. False in explicit ingestion mode.
    pub allow_name_inference: bool,
    pub chunk_profile: crate::manifest::ArtifactSizingPreset,
    pub bundle_profile: crate::manifest::BundleSizingPreset,
    pub compression_profile: crate::manifest::CompressionPreset,
    pub storage_mode: crate::manifest::ArtifactStorageMode,
    pub runtime_hints: RuntimeResourceHints,
    pub control: crate::GutchOperationControl,
}

#[derive(Debug, Clone)]
pub struct RepoSnapshotCaptureResult {
    pub repo_root: PathBuf,
    pub snapshot_id: String,
    pub manifest_path: PathBuf,
    pub version_id: String,
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
    pub(crate) build_timings: crate::BuildTimings,
}

impl SnapshotCapturePlan {
    pub fn from_candidate(
        candidate: SourceSnapshotOperationCandidate,
        kind: SnapshotCaptureKind,
        from_tags: Option<SnapshotTagSet>,
        replaced_snapshot_ids: Vec<String>,
    ) -> Self {
        let replaces_existing_slots = kind == SnapshotCaptureKind::ReplaceSlot;
        Self {
            kind,
            snapshot_id: candidate.snapshot_id,
            folder_name: candidate.folder_name,
            path: candidate.path,
            version_id: candidate.version_id,
            os: candidate.os,
            arch: candidate.arch,
            snapshot_tags: candidate.snapshot_tags,
            from_tags,
            replaced_snapshot_ids,
            replaces_existing_slots,
        }
    }

    pub fn requires_confirmation(&self) -> bool {
        true
    }
}

pub fn plan_snapshot_capture(
    repo_root: &Path,
    source_dir: &Path,
    explicit_tags: &SnapshotTagSet,
    allow_name_inference: bool,
) -> Result<SnapshotCapturePlan> {
    let (_, repo_manifest) = crate::load_gutch_repo_manifest(repo_root)?;
    let rules = repo_manifest
        .snapshot_tag_schema
        .clone()
        .unwrap_or_default();

    let candidate = build_source_snapshot_ops_candidate(
        source_dir,
        &rules,
        explicit_tags,
        allow_name_inference,
    )?;
    let status = check_gutch_repo(repo_root)?;

    let replaced_snapshot_ids: Vec<String> = status
        .snapshots
        .iter()
        .filter(|snapshot| snapshot.snapshot_tags == candidate.snapshot_tags)
        .map(|snapshot| snapshot.snapshot_id.clone())
        .collect();

    if !replaced_snapshot_ids.is_empty() {
        return Ok(SnapshotCapturePlan::from_candidate(
            candidate,
            SnapshotCaptureKind::ReplaceSlot,
            None,
            replaced_snapshot_ids,
        ));
    }

    Ok(SnapshotCapturePlan::from_candidate(
        candidate,
        SnapshotCaptureKind::CaptureNew,
        None,
        Vec::new(),
    ))
}

pub(crate) fn delete_repo_snapshots(repo_root: &Path, snapshot_ids: &[String]) -> Result<()> {
    for snapshot_id in snapshot_ids {
        remove_snapshot_from_repo(repo_root, snapshot_id)?;
    }
    Ok(())
}

pub fn delete_repo_snapshot_and_cleanup(
    repo_root: &Path,
    snapshot_id: &str,
    control: &crate::GutchOperationControl,
) -> Result<usize> {
    control.checkpoint()?;
    // Total matches cleanup_orphan_bundles_in_repo.
    const TOTAL: usize = 1000;

    progress::emit_progress(
        control.progress(),
        GutchProgressEvent {
            operation: GutchOperation::SnapshotDelete,
            phase: GutchProgressPhase::RemoveManifest,
            message: format!("Removing snapshot '{}'", snapshot_id),
            current: 0,
            total: TOTAL,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: Some(snapshot_id.to_string()),
            done: false,
        },
    );
    remove_snapshot_from_repo(repo_root, snapshot_id)?;

    let orphan_count = crate::repo::cleanup_orphan_bundles_in_repo(repo_root, control)?;

    progress::emit_progress(
        control.progress(),
        GutchProgressEvent {
            operation: GutchOperation::SnapshotDelete,
            phase: GutchProgressPhase::Done,
            message: format!(
                "Deleted snapshot, removed {} orphan bundle(s)",
                orphan_count
            ),
            current: TOTAL,
            total: TOTAL,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: true,
        },
    );
    Ok(orphan_count)
}

pub(crate) fn delete_repo_snapshots_with_matching_tags(
    repo_root: &Path,
    snapshot_tags: &SnapshotTagSet,
) -> Result<Vec<String>> {
    let status = check_gutch_repo(repo_root)?;
    let mut removed = Vec::new();
    for snapshot in status.snapshots {
        if snapshot.snapshot_tags != *snapshot_tags {
            continue;
        }
        remove_snapshot_from_repo(repo_root, &snapshot.snapshot_id)?;
        removed.push(snapshot.snapshot_id);
    }
    Ok(removed)
}

pub fn capture_repo_snapshot(
    request: RepoSnapshotCaptureRequest,
) -> Result<RepoSnapshotCaptureResult> {
    request.control.checkpoint()?;
    let _prepare_span = request.control.profile_span("gutch.capture.prepare");
    let (repo_paths, repo_manifest) = crate::load_gutch_repo_manifest(&request.repo_root)?;
    let rules = repo_manifest
        .snapshot_tag_schema
        .clone()
        .unwrap_or_default();
    let capture_plan = plan_snapshot_capture(
        request.repo_root.as_path(),
        request.source_dir.as_path(),
        &request.explicit_tags,
        request.allow_name_inference,
    )?;

    let mut store_state =
        MutableCaptureStoreState::from_existing(scan_existing_store_state(&repo_paths)?);
    if capture_plan.kind == SnapshotCaptureKind::ReplaceSlot {
        request.control.checkpoint()?;
        delete_repo_snapshots(
            request.repo_root.as_path(),
            &capture_plan.replaced_snapshot_ids,
        )?;
        let _ =
            crate::cleanup_orphan_bundles_in_repo(request.repo_root.as_path(), &request.control);
    }
    if capture_plan.kind == SnapshotCaptureKind::Relabel {
        request.control.checkpoint()?;
        let existing_manifest =
            crate::load_snapshot_manifest(&repo_paths, &capture_plan.snapshot_id)?;
        let mut updated_manifest = existing_manifest;
        updated_manifest.snapshot_tags = capture_plan.snapshot_tags.clone();
        let (manifest_path, _) =
            crate::write_snapshot_manifest_artifacts(&repo_paths, &updated_manifest)?;
        let chunk_index = crate::load_chunk_index(&repo_paths)?;
        return Ok(RepoSnapshotCaptureResult {
            repo_root: repo_paths.repo_root,
            snapshot_id: updated_manifest.snapshot_id.clone(),
            manifest_path,
            version_id: capture_plan.version_id,
            os: capture_plan.os,
            arch: capture_plan.arch,
            file_count: updated_manifest.file_count(),
            total_logical_size: updated_manifest.total_logical_size,
            total_compressed_size_estimate: updated_manifest.total_compressed_size_estimate,
            required_chunk_count: crate::repo::required_chunk_hashes(&updated_manifest).len(),
            required_bundle_count: crate::repo::required_bundle_ids(
                &updated_manifest,
                &chunk_index,
            )
            .len(),
            generated_bundle_count: 0,
            generated_chunk_count: 0,
            reused_chunk_count: crate::repo::required_chunk_hashes(&updated_manifest).len(),
            new_chunk_count: 0,
            build_timings: crate::BuildTimings::default(),
        });
    }

    let prepared_input = prepare_input_path(&request.source_dir)?;
    let resolved_identity = crate::resolve_snapshot_identity_with_inference(
        request.source_dir.as_path(),
        &request.explicit_tags,
        &rules,
        request.allow_name_inference,
        &format!("snapshot capture input '{}'", request.source_dir.display()),
    )?;

    let existing_chunk_ids = store_state
        .chunk_locations
        .keys()
        .map(|chunk_hash| ChunkHash::parse_hex(chunk_hash))
        .collect::<Result<HashSet<_>>>()?;
    let mut build_config = crate::BuildConfig::for_snapshot("", &resolved_identity.snapshot_tags)
        .with_existing_chunks(&store_state.chunk_locations)
        .with_existing_bundles(&store_state.bundle_ids)
        .with_previous_manifests(&store_state.previous_file_manifests)
        .with_chunk_profile(request.chunk_profile)
        .with_bundle_profile(request.bundle_profile)
        .with_compression_profile(request.compression_profile)
        .with_storage_mode(request.storage_mode)
        .with_runtime_hints(request.runtime_hints);
    let compression_profile =
        crate::manifest::CompressionProfile::for_preset(request.compression_profile);
    build_config.control = request.control.clone();

    drop(_prepare_span);
    let (result, timings) = crate::build_with_config_timed(&prepared_input.dir, &build_config)?;
    let _finalize_span = request.control.profile_span("gutch.capture.finalize");
    request.control.checkpoint()?;
    let mut result = result;
    let snapshot_id = crate::compute_snapshot_id_from_entries(&result.snapshot_manifest.entries);
    result.snapshot_manifest.snapshot_id = snapshot_id.clone();
    result.snapshot_manifest.source_modified_at =
        crate::repo::source_modified_at_from_directory(request.source_dir.as_path())?;
    crate::snapshot::identity::apply_resolved_snapshot_identity(
        &mut result.snapshot_manifest,
        &resolved_identity,
    );

    let existing_manifest_path = crate::snapshot_metadata_path(&repo_paths, &snapshot_id)?;
    if existing_manifest_path.exists() {
        let existing_manifest = crate::load_snapshot_manifest(&repo_paths, &snapshot_id)?;
        if existing_manifest.snapshot_tags != result.snapshot_manifest.snapshot_tags {
            let mut updated_manifest = existing_manifest;
            updated_manifest.snapshot_tags = result.snapshot_manifest.snapshot_tags.clone();
            updated_manifest.source_modified_at =
                result.snapshot_manifest.source_modified_at.clone();
            let (manifest_path, _) =
                crate::write_snapshot_manifest_artifacts(&repo_paths, &updated_manifest)?;
            let required_chunk_count = crate::repo::required_chunk_hashes(&updated_manifest).len();
            let required_bundle_count =
                crate::repo::required_bundle_ids(&updated_manifest, &store_state.chunk_index).len();
            return Ok(RepoSnapshotCaptureResult {
                repo_root: repo_paths.repo_root,
                snapshot_id: updated_manifest.snapshot_id.clone(),
                manifest_path,
                version_id: resolved_identity
                    .snapshot_tags
                    .get_exclusive("version")
                    .unwrap_or_default()
                    .to_string(),
                os: resolved_identity
                    .snapshot_tags
                    .get_exclusive("os")
                    .unwrap_or_default()
                    .to_string(),
                arch: resolved_identity
                    .snapshot_tags
                    .get_exclusive("arch")
                    .unwrap_or_default()
                    .to_string(),
                file_count: updated_manifest.file_count(),
                total_logical_size: updated_manifest.total_logical_size,
                total_compressed_size_estimate: updated_manifest.total_compressed_size_estimate,
                required_chunk_count,
                required_bundle_count,
                generated_bundle_count: 0,
                generated_chunk_count: 0,
                reused_chunk_count: required_chunk_count,
                new_chunk_count: 0,
                build_timings: timings,
            });
        }
    }

    // Artifact publication below persists the captured snapshot. Check once before it and then complete the
    // internally consistent manifest/bundle/index write set.
    request.control.checkpoint()?;
    let (manifest_path, _) =
        crate::write_snapshot_manifest_artifacts(&repo_paths, &result.snapshot_manifest)?;
    write_bundles(&repo_paths.bundles_root, &result.bundles)?;
    write_loose_chunks(&repo_paths.chunks_root, &result.loose_chunks)?;
    crate::add_built_bundles_to_chunk_index(
        &mut store_state.chunk_index,
        &result.bundles,
        &compression_profile,
    )?;
    crate::add_built_loose_chunks_to_chunk_index(
        &mut store_state.chunk_index,
        &result.loose_chunks,
    )?;

    for bundle in &result.bundles {
        store_state.bundle_ids.insert(bundle.bundle_id.clone());
    }
    store_state.chunk_locations =
        crate::repo::existing_chunk_locations_from_index(&store_state.chunk_index, &repo_paths)?;
    for file in result.snapshot_manifest.files() {
        let candidates = store_state
            .previous_file_manifests
            .entry(file.path.clone())
            .or_default();
        if !candidates.iter().any(|candidate| {
            candidate.size == file.size && candidate.full_file_hash == file.full_file_hash
        }) {
            candidates.push(file.clone());
        }
    }
    crate::write_chunk_index(&repo_paths, &store_state.chunk_index)?;
    let required_chunks = crate::repo::required_chunk_hashes_typed(&result.snapshot_manifest)?;
    let required_chunk_count = required_chunks.len();
    let required_bundle_count = count_required_bundles_for_build(
        &result.snapshot_manifest,
        &store_state.chunk_locations,
        &result.bundles,
    );
    let reused_chunk_count = required_chunks
        .iter()
        .filter(|chunk_id| existing_chunk_ids.contains(*chunk_id))
        .count();

    let capture_result = RepoSnapshotCaptureResult {
        repo_root: repo_paths.repo_root,
        snapshot_id: result.snapshot_manifest.snapshot_id.clone(),
        manifest_path,
        version_id: resolved_identity
            .snapshot_tags
            .get_exclusive("version")
            .unwrap_or_default()
            .to_string(),
        os: resolved_identity
            .snapshot_tags
            .get_exclusive("os")
            .unwrap_or_default()
            .to_string(),
        arch: resolved_identity
            .snapshot_tags
            .get_exclusive("arch")
            .unwrap_or_default()
            .to_string(),
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

impl RepoSnapshotCaptureResult {
    /// Emit a done event if a progress sink was provided.
    pub(crate) fn emit_done(&self, progress: &Option<Arc<dyn GutchProgressSink>>) {
        progress::emit_progress(
            progress,
            GutchProgressEvent {
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

struct MutableCaptureStoreState {
    bundle_ids: HashSet<String>,
    chunk_index: crate::manifest::ChunkIndex,
    chunk_locations: HashMap<String, crate::ExistingChunkLocation>,
    previous_file_manifests: HashMap<String, Vec<crate::manifest::FileManifest>>,
}

impl MutableCaptureStoreState {
    fn from_existing(state: ExistingStoreState) -> Self {
        Self {
            bundle_ids: state.bundle_ids,
            chunk_index: state.chunk_index,
            chunk_locations: state.chunk_locations,
            previous_file_manifests: state.previous_file_manifests,
        }
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

fn write_bundles(bundles_root: &Path, bundles: &[crate::BuiltBundle]) -> Result<()> {
    for bundle in bundles {
        let bundle_path = crate::bundle_path_in_root(bundles_root, &bundle.bundle_id)?;
        crate::helpers::write(&bundle_path, &bundle.data)?;
    }
    Ok(())
}

fn write_loose_chunks(chunks_root: &Path, chunks: &[crate::BuiltLooseChunk]) -> Result<()> {
    for chunk in chunks {
        let chunk_hash = chunk.chunk_hash.to_string();
        let chunk_path = crate::chunk_path_in_root(chunks_root, &chunk_hash)?;
        if let Some(parent) = chunk_path.parent() {
            crate::helpers::create_dir_all(parent)?;
        }
        crate::helpers::write(&chunk_path, &chunk.data)?;
    }
    Ok(())
}

fn count_required_bundles_for_build(
    snapshot_manifest: &crate::manifest::snapshot::SnapshotManifest,
    existing_chunk_locations: &HashMap<String, crate::ExistingChunkLocation>,
    generated_bundles: &[crate::BuiltBundle],
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
    let Ok(required_chunks) = crate::repo::required_chunk_hashes_typed(snapshot_manifest) else {
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

pub fn describe_snapshot_capture_plan(plan: &SnapshotCapturePlan) -> String {
    match plan.kind {
        SnapshotCaptureKind::CaptureNew => format!(
            "Capture new snapshot slot {} ({})",
            plan.folder_name, plan.version_id
        ),
        SnapshotCaptureKind::Relabel => format!(
            "Update tags for existing snapshot {} ({}): folder rename or tag inference changed; snapshot unchanged",
            plan.snapshot_id, plan.folder_name
        ),
        SnapshotCaptureKind::ReplaceSlot => format!(
            "Replace snapshot slot for {} ({}): same tags, new snapshot id {}. Removes repo snapshot(s) {}. Existing snapshot references to the old snapshot id must be updated.",
            plan.folder_name,
            plan.version_id,
            plan.snapshot_id,
            plan.replaced_snapshot_ids.join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GutchRepoPaths;
    use crate::manifest::{self, snapshot::SnapshotManifest};
    use crate::repo::{
        compute_snapshot_id_from_entries, ensure_gutch_repo_layout,
        write_snapshot_manifest_artifacts,
    };

    #[test]
    fn plan_snapshot_capture_detects_replace_same_tags() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let source_root = temp.path().join("sources");
        let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
        ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

        let new_source = source_root.join("game-v1.0.0-windows-x86_64-base-2026_07_14");
        std::fs::create_dir_all(&new_source).unwrap();
        std::fs::write(new_source.join("game.exe"), b"old-content").unwrap();

        let (_, repo_manifest) = crate::load_gutch_repo_manifest(&repo_root).unwrap();
        let rules = repo_manifest
            .snapshot_tag_schema
            .clone()
            .unwrap_or_default();
        let old_candidate = build_source_snapshot_ops_candidate(
            &new_source,
            &rules,
            &manifest::SnapshotTagSet::default(),
            true,
        )
        .unwrap();
        let old_files = vec![manifest::FileManifest {
            path: "game.exe".into(),
            size: 11,
            executable: false,
            read_only: false,
            full_file_hash: crate::hash::hash_hex(
                &crate::hash::hash_file(new_source.join("game.exe").as_path()).unwrap(),
            ),
            chunks: Vec::new(),
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
            crate::repo::source_modified_at_from_directory(&new_source).unwrap();
        old_manifest.set_files(old_files);
        write_snapshot_manifest_artifacts(&repo_paths, &old_manifest).unwrap();

        std::fs::write(new_source.join("game.exe"), b"new-content").unwrap();

        let plan = plan_snapshot_capture(
            &repo_root,
            &new_source,
            &manifest::SnapshotTagSet::default(),
            true,
        )
        .unwrap();
        assert_eq!(plan.kind, SnapshotCaptureKind::ReplaceSlot);
        assert_eq!(plan.replaced_snapshot_ids, vec![old_snapshot_id]);
        assert!(plan.replaces_existing_slots);
    }
}
