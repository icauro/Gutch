use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};

use super::{
    ExistingStoreState, Result, SourceTargetInstallCandidate,
    check::build_source_target_install_candidate, check_gutch_repo, remove_target_from_repo,
    scan_existing_store_state,
};
use crate::hash::ChunkHash;
use crate::manifest::TargetTagSet;
use crate::progress::{
    self, GutchOperation, GutchProgressEvent, GutchProgressPhase, GutchProgressSink,
};
use crate::runtime::RuntimeResourceHints;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TargetInsertKind {
    InsertNew,
    UpdateTags,
    ReplaceSameTags,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetInsertPlan {
    pub kind: TargetInsertKind,
    pub content_target_id: String,
    pub folder_name: String,
    pub path: String,
    pub version_id: String,
    pub os: String,
    pub arch: String,
    pub target_tags: TargetTagSet,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_tags: Option<TargetTagSet>,
    pub replaced_target_ids: Vec<String>,
    pub invalidates_existing_installs: bool,
}

#[derive(Debug, Clone)]
pub struct RepoTargetInsertRequest {
    pub repo_root: PathBuf,
    pub source_dir: PathBuf,
    pub explicit_tags: TargetTagSet,
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
pub struct RepoTargetInsertResult {
    pub repo_root: PathBuf,
    pub target_id: String,
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
    pub build_timings: crate::BuildTimings,
}

impl TargetInsertPlan {
    pub fn from_candidate(
        candidate: SourceTargetInstallCandidate,
        kind: TargetInsertKind,
        from_tags: Option<TargetTagSet>,
        replaced_target_ids: Vec<String>,
    ) -> Self {
        let invalidates_existing_installs = kind == TargetInsertKind::ReplaceSameTags;
        Self {
            kind,
            content_target_id: candidate.content_target_id,
            folder_name: candidate.folder_name,
            path: candidate.path,
            version_id: candidate.version_id,
            os: candidate.os,
            arch: candidate.arch,
            target_tags: candidate.target_tags,
            from_tags,
            replaced_target_ids,
            invalidates_existing_installs,
        }
    }

    pub fn requires_confirmation(&self) -> bool {
        true
    }
}

pub fn plan_target_insert(
    repo_root: &Path,
    source_dir: &Path,
    explicit_tags: &TargetTagSet,
    allow_name_inference: bool,
) -> Result<TargetInsertPlan> {
    let (_, repo_manifest) = crate::load_gutch_repo_manifest(repo_root)?;
    let rules = repo_manifest.target_schema.clone().unwrap_or_default();

    let candidate = build_source_target_install_candidate(
        source_dir,
        &rules,
        explicit_tags,
        allow_name_inference,
    )?;
    let status = check_gutch_repo(repo_root)?;

    let replaced_target_ids: Vec<String> = status
        .targets
        .iter()
        .filter(|target| target.target_tags == candidate.target_tags)
        .map(|target| target.target_id.clone())
        .collect();

    if !replaced_target_ids.is_empty() {
        return Ok(TargetInsertPlan::from_candidate(
            candidate,
            TargetInsertKind::ReplaceSameTags,
            None,
            replaced_target_ids,
        ));
    }

    Ok(TargetInsertPlan::from_candidate(
        candidate,
        TargetInsertKind::InsertNew,
        None,
        Vec::new(),
    ))
}

pub fn delete_repo_targets(repo_root: &Path, target_ids: &[String]) -> Result<()> {
    for target_id in target_ids {
        remove_target_from_repo(repo_root, target_id)?;
    }
    Ok(())
}

pub fn delete_repo_target_and_cleanup(
    repo_root: &Path,
    target_id: &str,
    control: &crate::GutchOperationControl,
) -> Result<usize> {
    control.checkpoint()?;
    // Total matches cleanup_orphan_bundles_in_repo.
    const TOTAL: usize = 1000;

    progress::emit_progress(
        control.progress(),
        GutchProgressEvent {
            operation: GutchOperation::TargetDelete,
            phase: GutchProgressPhase::RemoveManifest,
            message: format!("Removing target '{}'", target_id),
            current: 0,
            total: TOTAL,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: Some(target_id.to_string()),
            done: false,
        },
    );
    remove_target_from_repo(repo_root, target_id)?;

    let orphan_count = crate::repo::cleanup_orphan_bundles_in_repo(repo_root, control)?;

    progress::emit_progress(
        control.progress(),
        GutchProgressEvent {
            operation: GutchOperation::TargetDelete,
            phase: GutchProgressPhase::Done,
            message: format!("Deleted target, removed {} orphan bundle(s)", orphan_count),
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

pub fn delete_repo_targets_with_matching_tags(
    repo_root: &Path,
    target_tags: &TargetTagSet,
) -> Result<Vec<String>> {
    let status = check_gutch_repo(repo_root)?;
    let mut removed = Vec::new();
    for target in status.targets {
        if target.target_tags != *target_tags {
            continue;
        }
        remove_target_from_repo(repo_root, &target.target_id)?;
        removed.push(target.target_id);
    }
    Ok(removed)
}

/// Bridge that adapts a `GutchProgressSink` to `BuildProgressSink` for the build phase.
struct BuildProgressBridge {
    sink: Arc<dyn GutchProgressSink>,
    scanned_files: AtomicUsize,
    total_files: AtomicUsize,
    built_bundles: AtomicUsize,
    total_bundles: AtomicUsize,
}

impl std::fmt::Debug for BuildProgressBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuildProgressBridge").finish()
    }
}

impl crate::build_types::BuildProgressSink for BuildProgressBridge {
    fn on_file_scan_started(&self, total_files: usize) {
        self.scanned_files.store(0, Ordering::Relaxed);
        self.total_files.store(total_files, Ordering::Relaxed);
        self.sink.emit(GutchProgressEvent {
            operation: GutchOperation::TargetInsert,
            phase: GutchProgressPhase::Scan,
            message: "Scanning files".to_string(),
            current: 0,
            total: total_files,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: false,
        });
    }

    fn on_file_scanned(&self) {
        let current = self.scanned_files.fetch_add(1, Ordering::Relaxed) + 1;
        self.sink.emit(GutchProgressEvent {
            operation: GutchOperation::TargetInsert,
            phase: GutchProgressPhase::Scan,
            message: "Scanning files".to_string(),
            current,
            total: self.total_files.load(Ordering::Relaxed),
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: false,
        });
    }

    fn on_bundle_build_started(&self, total_bundles: usize) {
        self.built_bundles.store(0, Ordering::Relaxed);
        self.total_bundles.store(total_bundles, Ordering::Relaxed);
        self.sink.emit(GutchProgressEvent {
            operation: GutchOperation::TargetInsert,
            phase: GutchProgressPhase::Build,
            message: "Building bundles".to_string(),
            current: 0,
            total: total_bundles,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: false,
        });
    }

    fn on_bundle_built(&self) {
        let current = self.built_bundles.fetch_add(1, Ordering::Relaxed) + 1;
        self.sink.emit(GutchProgressEvent {
            operation: GutchOperation::TargetInsert,
            phase: GutchProgressPhase::Build,
            message: "Building bundles".to_string(),
            current,
            total: self.total_bundles.load(Ordering::Relaxed),
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: false,
        });
    }
}

pub fn insert_repo_target(request: RepoTargetInsertRequest) -> Result<RepoTargetInsertResult> {
    request.control.checkpoint()?;
    let (repo_paths, repo_manifest) = crate::load_gutch_repo_manifest(&request.repo_root)?;
    let rules = repo_manifest.target_schema.clone().unwrap_or_default();
    let insert_plan = plan_target_insert(
        request.repo_root.as_path(),
        request.source_dir.as_path(),
        &request.explicit_tags,
        request.allow_name_inference,
    )?;

    let mut store_state =
        MutableInsertStoreState::from_existing(scan_existing_store_state(&repo_paths)?);
    if insert_plan.kind == TargetInsertKind::ReplaceSameTags {
        request.control.checkpoint()?;
        delete_repo_targets(
            request.repo_root.as_path(),
            &insert_plan.replaced_target_ids,
        )?;
        let _ =
            crate::cleanup_orphan_bundles_in_repo(request.repo_root.as_path(), &request.control);
    }
    if insert_plan.kind == TargetInsertKind::UpdateTags {
        request.control.checkpoint()?;
        let existing_manifest =
            crate::load_target_manifest(&repo_paths, &insert_plan.content_target_id)?;
        let mut updated_manifest = existing_manifest;
        updated_manifest.target_tags = insert_plan.target_tags.clone();
        let (manifest_path, _) =
            crate::write_target_manifest_artifacts(&repo_paths, &updated_manifest)?;
        let chunk_index = crate::load_chunk_index(&repo_paths)?;
        return Ok(RepoTargetInsertResult {
            repo_root: repo_paths.repo_root,
            target_id: updated_manifest.target_id.clone(),
            manifest_path,
            version_id: insert_plan.version_id,
            os: insert_plan.os,
            arch: insert_plan.arch,
            file_count: updated_manifest.files.len(),
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
    let resolved_identity = crate::resolve_target_identity_with_inference(
        request.source_dir.as_path(),
        &request.explicit_tags,
        &rules,
        request.allow_name_inference,
        &format!("target commit input '{}'", request.source_dir.display()),
    )?;

    let existing_chunk_ids = store_state
        .chunk_locations
        .keys()
        .map(|chunk_hash| ChunkHash::parse_hex(chunk_hash))
        .collect::<Result<HashSet<_>>>()?;
    let chunk_profile = crate::manifest::ChunkSizingProfile::for_preset(request.chunk_profile);
    let bundle_profile = crate::manifest::BundleSizingProfile::for_preset(request.bundle_profile);
    let compression_profile =
        crate::manifest::CompressionProfile::for_preset(request.compression_profile);
    let mut build_config =
        crate::BuildConfig::with_ids(&repo_manifest.name, "", &resolved_identity.target_tags)
            .with_existing_chunks(&store_state.chunk_locations)
            .with_existing_bundles(&store_state.bundle_ids)
            .with_previous_manifests(&store_state.previous_file_manifests)
            .with_chunk_profile(&chunk_profile)
            .with_bundle_profile(&bundle_profile)
            .with_compression_profile(&compression_profile);
    build_config.storage_mode = request.storage_mode;
    build_config.runtime_hints = request.runtime_hints;
    build_config.control = request.control.clone();
    if let Some(sink) = request.control.progress() {
        build_config.progress = Some(Arc::new(BuildProgressBridge {
            sink: sink.clone(),
            scanned_files: AtomicUsize::new(0),
            total_files: AtomicUsize::new(0),
            built_bundles: AtomicUsize::new(0),
            total_bundles: AtomicUsize::new(0),
        }));
    }

    let (result, timings) = crate::build_with_config(&prepared_input.dir, &build_config)?;
    request.control.checkpoint()?;
    let mut result = result;
    let target_id = crate::compute_target_id_from_files(&result.target_manifest.files);
    result.target_manifest.target_id = target_id.clone();
    result.target_manifest.source_modified_at =
        crate::repo::source_modified_at_from_directory(request.source_dir.as_path())?;
    crate::core::target_identity::apply_resolved_target_identity(
        &mut result.target_manifest,
        &resolved_identity,
    );

    let existing_manifest_path = crate::target_manifest_path(&repo_paths, &target_id);
    if existing_manifest_path.exists() {
        let existing_manifest = crate::load_target_manifest(&repo_paths, &target_id)?;
        if existing_manifest.target_tags != result.target_manifest.target_tags {
            let mut updated_manifest = existing_manifest;
            updated_manifest.target_tags = result.target_manifest.target_tags.clone();
            updated_manifest.source_modified_at = result.target_manifest.source_modified_at.clone();
            let (manifest_path, _) =
                crate::write_target_manifest_artifacts(&repo_paths, &updated_manifest)?;
            let required_chunk_count = crate::repo::required_chunk_hashes(&updated_manifest).len();
            let required_bundle_count =
                crate::repo::required_bundle_ids(&updated_manifest, &store_state.chunk_index).len();
            return Ok(RepoTargetInsertResult {
                repo_root: repo_paths.repo_root,
                target_id: updated_manifest.target_id.clone(),
                manifest_path,
                version_id: resolved_identity
                    .target_tags
                    .get_exclusive("version")
                    .unwrap_or_default()
                    .to_string(),
                os: resolved_identity
                    .target_tags
                    .get_exclusive("os")
                    .unwrap_or_default()
                    .to_string(),
                arch: resolved_identity
                    .target_tags
                    .get_exclusive("arch")
                    .unwrap_or_default()
                    .to_string(),
                file_count: updated_manifest.files.len(),
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

    // Artifact publication below is the insert commit. Check once before it and then complete the
    // internally consistent manifest/bundle/index write set.
    request.control.checkpoint()?;
    let (manifest_path, _) =
        crate::write_target_manifest_artifacts(&repo_paths, &result.target_manifest)?;
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
        crate::repo::existing_chunk_locations_from_index(&store_state.chunk_index, &repo_paths);
    for file in &result.target_manifest.files {
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
    crate::repo::assign_new_target_to_default(&repo_paths, &target_id)?;

    let required_chunks = crate::repo::required_chunk_hashes_typed(&result.target_manifest)?;
    let required_chunk_count = required_chunks.len();
    let required_bundle_count = count_required_bundles_for_build(
        &result.target_manifest,
        &store_state.chunk_locations,
        &result.bundles,
    );
    let reused_chunk_count = required_chunks
        .iter()
        .filter(|chunk_id| existing_chunk_ids.contains(*chunk_id))
        .count();

    let insert_result = RepoTargetInsertResult {
        repo_root: repo_paths.repo_root,
        target_id: result.target_manifest.target_id.clone(),
        manifest_path,
        version_id: resolved_identity
            .target_tags
            .get_exclusive("version")
            .unwrap_or_default()
            .to_string(),
        os: resolved_identity
            .target_tags
            .get_exclusive("os")
            .unwrap_or_default()
            .to_string(),
        arch: resolved_identity
            .target_tags
            .get_exclusive("arch")
            .unwrap_or_default()
            .to_string(),
        file_count: result.target_manifest.files.len(),
        total_logical_size: result.target_manifest.total_logical_size,
        total_compressed_size_estimate: result.target_manifest.total_compressed_size_estimate,
        required_chunk_count,
        required_bundle_count,
        generated_bundle_count: result.bundles.len(),
        generated_chunk_count: result.loose_chunks.len(),
        reused_chunk_count,
        new_chunk_count: required_chunk_count.saturating_sub(reused_chunk_count),
        build_timings: timings,
    };
    insert_result.emit_done(request.control.progress());
    Ok(insert_result)
}

impl RepoTargetInsertResult {
    /// Emit a done event if a progress sink was provided.
    pub(crate) fn emit_done(&self, progress: &Option<Arc<dyn GutchProgressSink>>) {
        progress::emit_progress(
            progress,
            GutchProgressEvent {
                operation: GutchOperation::TargetInsert,
                phase: GutchProgressPhase::Done,
                message: "Target commit complete".to_string(),
                current: 1,
                total: 1,
                transferred_bytes: 0,
                total_bytes: 0,
                detail: Some(format!("target_id={}", self.target_id)),
                done: true,
            },
        );
    }
}

struct MutableInsertStoreState {
    bundle_ids: HashSet<String>,
    chunk_index: crate::manifest::ChunkIndex,
    chunk_locations: HashMap<String, crate::ExistingChunkLocation>,
    previous_file_manifests: HashMap<String, Vec<crate::manifest::FileManifest>>,
}

impl MutableInsertStoreState {
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
        let bundle_path = crate::bundle_path_in_root(bundles_root, &bundle.bundle_id);
        crate::helpers::write(&bundle_path, &bundle.data)?;
    }
    Ok(())
}

fn write_loose_chunks(chunks_root: &Path, chunks: &[crate::BuiltLooseChunk]) -> Result<()> {
    for chunk in chunks {
        let chunk_hash = chunk.chunk_hash.to_string();
        let chunk_path = crate::chunk_path_in_root(chunks_root, &chunk_hash);
        if let Some(parent) = chunk_path.parent() {
            crate::helpers::create_dir_all(parent)?;
        }
        crate::helpers::write(&chunk_path, &chunk.data)?;
    }
    Ok(())
}

fn count_required_bundles_for_build(
    target_manifest: &crate::manifest::target::TargetManifest,
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
    let Ok(required_chunks) = crate::repo::required_chunk_hashes_typed(target_manifest) else {
        return generated_bundles.len();
    };
    for chunk_hash in required_chunks {
        if let Some(existing) = existing_chunk_locations_by_hash.get(&chunk_hash) {
            if existing.storage_mode == crate::manifest::ArtifactStorageMode::Bundled {
                required_bundle_ids.insert(existing.bundle_id.clone());
            }
        }
    }
    required_bundle_ids.len() + generated_bundles.len()
}

struct PreparedInputPath {
    dir: PathBuf,
    _tempdir: Option<tempfile::TempDir>,
}

pub fn describe_target_insert_plan(plan: &TargetInsertPlan) -> String {
    match plan.kind {
        TargetInsertKind::InsertNew => format!(
            "Commit new target slot {} ({})",
            plan.folder_name, plan.version_id
        ),
        TargetInsertKind::UpdateTags => format!(
            "Update tags for existing content {} ({}): folder rename or tag inference changed; content unchanged",
            plan.content_target_id, plan.folder_name
        ),
        TargetInsertKind::ReplaceSameTags => format!(
            "Replace target slot for {} ({}): same tags, new content id {}. Removes repo target(s) {}. User installs pinned to the old content id become invalid until updated.",
            plan.folder_name,
            plan.version_id,
            plan.content_target_id,
            plan.replaced_target_ids.join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GutchRepoPaths;
    use crate::manifest::{self, target::TargetManifest};
    use crate::repo::{
        compute_target_id_from_files, ensure_gutch_repo_layout, write_target_manifest_artifacts,
    };

    #[test]
    fn plan_target_insert_detects_replace_same_tags() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let source_root = temp.path().join("sources");
        let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
        ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

        let new_source = source_root.join("game-v1.0.0-windows-x86_64-base-2026_07_14");
        std::fs::create_dir_all(&new_source).unwrap();
        std::fs::write(new_source.join("game.exe"), b"old-content").unwrap();

        let (_, repo_manifest) = crate::load_gutch_repo_manifest(&repo_root).unwrap();
        let rules = repo_manifest.target_schema.clone().unwrap_or_default();
        let old_candidate = build_source_target_install_candidate(
            &new_source,
            &rules,
            &manifest::TargetTagSet::default(),
            true,
        )
        .unwrap();
        let old_files = vec![manifest::FileManifest {
            path: "game.exe".into(),
            size: 11,
            executable: false,
            full_file_hash: crate::hash::hash_hex(
                &crate::hash::hash_file(new_source.join("game.exe").as_path()).unwrap(),
            ),
            chunks: Vec::new(),
        }];
        let old_content_id = compute_target_id_from_files(&old_files);
        let mut old_manifest =
            TargetManifest::new(old_content_id.clone(), old_candidate.target_tags.clone());
        old_manifest.source_modified_at =
            crate::repo::source_modified_at_from_directory(&new_source).unwrap();
        old_manifest.files = old_files;
        write_target_manifest_artifacts(&repo_paths, &old_manifest).unwrap();
        crate::repo::assign_new_target_to_default(&repo_paths, &old_content_id).unwrap();

        std::fs::write(new_source.join("game.exe"), b"new-content").unwrap();

        let plan = plan_target_insert(
            &repo_root,
            &new_source,
            &manifest::TargetTagSet::default(),
            true,
        )
        .unwrap();
        assert_eq!(plan.kind, TargetInsertKind::ReplaceSameTags);
        assert_eq!(plan.replaced_target_ids, vec![old_content_id]);
        assert!(plan.invalidates_existing_installs);
    }
}
