use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{
    RepoCheckStatus, Result, SourceSnapshotOperationCandidate, check_gutch_repo,
    discover_source_snapshot_operations, is_source_modified_at_newer,
    scan_existing_store_state_with_pool,
};
use crate::manifest::SnapshotTagSet;
use crate::progress::{
    self, GutchOperation, GutchProgressEvent, GutchProgressPhase, GutchProgressSink,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSourcesRefresh {
    pub folder_name: String,
    pub path: String,
    pub snapshot_tags: SnapshotTagSet,
    pub replaced_snapshot_ids: Vec<String>,
    pub source_modified_at: String,
}

#[derive(Debug, Clone, Copy)]
pub struct SnapshotSourcesCaptureOptions {
    pub prune_removed_snapshots: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSourcesCaptureResult {
    pub captured_count: usize,
    pub refreshed_count: usize,
    pub removed_count: usize,
    pub pruned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSourcesCapturePlan {
    pub sources_ready: bool,
    pub candidate_count: usize,
    pub capturable_count: usize,
    pub refresh_count: usize,
    pub removable_count: usize,
    #[serde(default)]
    pub to_capture: Vec<SourceSnapshotOperationCandidate>,
    #[serde(default)]
    pub to_refresh: Vec<SnapshotSourcesRefresh>,
    #[serde(default)]
    pub to_apply: Vec<SourceSnapshotOperationCandidate>,
    #[serde(default)]
    pub to_remove: Vec<String>,
}

impl SnapshotSourcesCapturePlan {
    fn empty() -> Self {
        Self {
            sources_ready: false,
            candidate_count: 0,
            capturable_count: 0,
            refresh_count: 0,
            removable_count: 0,
            to_capture: Vec::new(),
            to_refresh: Vec::new(),
            to_apply: Vec::new(),
            to_remove: Vec::new(),
        }
    }

    pub fn has_changes(&self) -> bool {
        self.capturable_count > 0 || self.refresh_count > 0 || self.removable_count > 0
    }

    pub fn up_to_date_count(&self) -> usize {
        self.candidate_count
            .saturating_sub(self.capturable_count)
            .saturating_sub(self.refresh_count)
    }
}

pub fn plan_snapshot_sources_capture(
    repo_root: &Path,
    sources_path: Option<&Path>,
) -> Result<SnapshotSourcesCapturePlan> {
    plan_snapshot_sources_capture_with_context(repo_root, sources_path, None, None)
}

pub fn plan_snapshot_sources_capture_with_context(
    repo_root: &Path,
    sources_path: Option<&Path>,
    candidates: Option<&[SourceSnapshotOperationCandidate]>,
    repo_status: Option<&RepoCheckStatus>,
) -> Result<SnapshotSourcesCapturePlan> {
    let Some(sources_path) = normalized_sources_path(sources_path) else {
        return Ok(SnapshotSourcesCapturePlan::empty());
    };

    let mut plan = SnapshotSourcesCapturePlan {
        sources_ready: true,
        ..SnapshotSourcesCapturePlan::empty()
    };

    let candidates = match candidates {
        Some(candidates) => candidates.to_vec(),
        None => match discover_source_snapshot_operations(repo_root, &sources_path) {
            Ok(candidates) => candidates,
            Err(_) => return Ok(plan),
        },
    };

    let status = match repo_status {
        Some(status) => status.clone(),
        None => check_gutch_repo(repo_root)?,
    };

    let valid_candidates: Vec<_> = candidates
        .into_iter()
        .filter(|candidate| candidate.error.is_none())
        .collect();

    let candidate_tag_sets: Vec<SnapshotTagSet> = valid_candidates
        .iter()
        .map(|candidate| candidate.snapshot_tags.clone())
        .collect();

    plan.candidate_count = valid_candidates.len();
    plan.to_remove = status
        .snapshots
        .iter()
        .filter(|snapshot| {
            !candidate_tag_sets
                .iter()
                .any(|tags| tags == &snapshot.snapshot_tags)
        })
        .map(|snapshot| snapshot.snapshot_id.clone())
        .collect();
    plan.removable_count = plan.to_remove.len();

    for candidate in valid_candidates {
        let matching_snapshots: Vec<_> = status
            .snapshots
            .iter()
            .filter(|snapshot| snapshot.snapshot_tags == candidate.snapshot_tags)
            .collect();

        if matching_snapshots.is_empty() {
            plan.to_capture.push(candidate.clone());
            plan.to_apply.push(candidate);
            continue;
        }

        let recorded_source_modified_at = matching_snapshots
            .first()
            .map(|snapshot| snapshot.source_modified_at.as_str())
            .unwrap_or_default();
        if !is_source_modified_at_newer(&candidate.source_modified_at, recorded_source_modified_at)
        {
            continue;
        }

        plan.to_refresh.push(SnapshotSourcesRefresh {
            folder_name: candidate.folder_name.clone(),
            path: candidate.path.clone(),
            snapshot_tags: candidate.snapshot_tags.clone(),
            replaced_snapshot_ids: matching_snapshots
                .iter()
                .map(|snapshot| snapshot.snapshot_id.clone())
                .collect(),
            source_modified_at: candidate.source_modified_at.clone(),
        });
        plan.to_apply.push(candidate);
    }

    plan.capturable_count = plan.to_capture.len();
    plan.refresh_count = plan.to_refresh.len();

    Ok(plan)
}

fn normalized_sources_path(sources_path: Option<&Path>) -> Option<std::path::PathBuf> {
    let sources_path = sources_path?;
    let trimmed = sources_path.to_string_lossy().trim().to_string();
    if trimmed.is_empty() {
        return None;
    }
    Some(std::path::PathBuf::from(trimmed))
}

pub fn describe_snapshot_sources_capture_plan(plan: &SnapshotSourcesCapturePlan) -> Vec<String> {
    let mut lines = Vec::new();
    for refresh in &plan.to_refresh {
        lines.push(format!(
            "Refresh snapshot slot for {} ({}): source modified at {}",
            refresh.folder_name, refresh.path, refresh.source_modified_at
        ));
    }
    for snapshot_id in &plan.to_remove {
        lines.push(format!(
            "Remove repo snapshot {} (missing from sources)",
            snapshot_id
        ));
    }
    for candidate in &plan.to_capture {
        lines.push(format!(
            "Capture new snapshot slot {} ({})",
            candidate.snapshot_id, candidate.folder_name
        ));
    }
    lines
}

pub fn capture_snapshot_sources(
    repo_root: &Path,
    sources_path: &Path,
    options: SnapshotSourcesCaptureOptions,
    control: &crate::GutchOperationControl,
) -> Result<SnapshotSourcesCaptureResult> {
    control.checkpoint()?;
    let plan = plan_snapshot_sources_capture(repo_root, Some(sources_path))?;
    if !plan.sources_ready {
        return Err(crate::Error::manifest(
            "snapshot sources path is missing or invalid",
        ));
    }

    if !plan.has_changes() {
        return Ok(SnapshotSourcesCaptureResult {
            captured_count: 0,
            refreshed_count: 0,
            removed_count: 0,
            pruned: options.prune_removed_snapshots,
        });
    }

    let total_steps = plan.to_apply.len() + plan.to_remove.len();
    let mut current_step = 0usize;
    let mut removed_count = 0usize;

    let runtime_plan = crate::plan_build_runtime(&crate::RuntimeResourceHints::default());
    let _pool_span = control.profile_span_with_args(
        "capture.create_worker_pool",
        std::collections::BTreeMap::from([(
            "worker_count".to_string(),
            serde_json::json!(runtime_plan.worker_count),
        )]),
    );
    let build_pool = crate::build_thread_pool(runtime_plan.worker_count, "gutch-capture")?;
    drop(_pool_span);

    let repo_paths = crate::GutchRepoPaths::from_repo_root(repo_root);
    let _existing_state_span = control.profile_span_with_args(
        "capture.prepare_source.load_existing_store_state",
        std::collections::BTreeMap::from([(
            "source_count".to_string(),
            serde_json::json!(plan.to_apply.len()),
        )]),
    );
    let mut store_state = scan_existing_store_state_with_pool(&repo_paths, &build_pool, control)?;
    drop(_existing_state_span);

    if options.prune_removed_snapshots {
        for snapshot_id in &plan.to_remove {
            control.checkpoint()?;
            crate::remove_snapshot_from_repo(repo_root, snapshot_id)?;
            removed_count += 1;
            current_step += 1;
            progress::emit_progress(
                control.progress(),
                GutchProgressEvent {
                    operation: GutchOperation::SnapshotSourcesCapture,
                    phase: GutchProgressPhase::Prune,
                    message: format!("Removing snapshot '{}'", snapshot_id),
                    current: current_step,
                    total: total_steps,
                    transferred_bytes: 0,
                    total_bytes: 0,
                    detail: Some(snapshot_id.to_string()),
                    done: false,
                },
            );
        }
    }

    let refresh_paths: HashSet<String> = plan
        .to_refresh
        .iter()
        .map(|refresh| refresh.path.clone())
        .collect();

    let mut captured_count = 0usize;
    let mut refreshed_count = 0usize;
    for candidate in &plan.to_apply {
        control.checkpoint()?;
        current_step += 1;
        progress::emit_progress(
            control.progress(),
            GutchProgressEvent {
                operation: GutchOperation::SnapshotSourcesCapture,
                phase: GutchProgressPhase::Capture,
                message: format!("Capturing snapshot '{}'", candidate.folder_name),
                current: current_step,
                total: total_steps,
                transferred_bytes: 0,
                total_bytes: 0,
                detail: Some(candidate.folder_name.clone()),
                done: false,
            },
        );
        let removed_for_replacement =
            crate::delete_repo_snapshots_with_matching_tags(repo_root, &candidate.snapshot_tags)?;
        removed_count += removed_for_replacement.len();
        if !removed_for_replacement.is_empty() {
            let _refresh_span =
                control.profile_span("capture.prepare_source.refresh_store_state_after_cleanup");
            store_state = scan_existing_store_state_with_pool(&repo_paths, &build_pool, control)?;
            drop(_refresh_span);
        }
        capture_source_snapshot(repo_root, candidate, control, &mut store_state, &build_pool)?;
        if refresh_paths.contains(&candidate.path) {
            refreshed_count += 1;
        } else {
            captured_count += 1;
        }
    }

    let _ = crate::cleanup_orphan_bundles_in_repo(repo_root, control)?;

    progress::emit_progress(
        control.progress(),
        GutchProgressEvent {
            operation: GutchOperation::SnapshotSourcesCapture,
            phase: GutchProgressPhase::Done,
            message: "Snapshot sources capture complete".to_string(),
            current: total_steps,
            total: total_steps,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: true,
        },
    );

    Ok(SnapshotSourcesCaptureResult {
        captured_count,
        refreshed_count,
        removed_count,
        pruned: options.prune_removed_snapshots,
    })
}

fn capture_source_snapshot(
    repo_root: &Path,
    candidate: &SourceSnapshotOperationCandidate,
    control: &crate::GutchOperationControl,
    store_state: &mut super::ExistingStoreState,
    build_pool: &rayon::ThreadPool,
) -> Result<()> {
    let candidate_control = if let Some(sink) = control.progress() {
        control.with_replaced_progress(Arc::new(SnapshotSourcesCandidateProgress {
            sink: sink.clone(),
            folder_name: candidate.folder_name.clone(),
        }))
    } else {
        control.clone()
    };
    crate::capture_repo_snapshot_with_existing_store_state(
        crate::RepoSnapshotCaptureRequest {
            repo_root: repo_root.to_path_buf(),
            source_dir: std::path::PathBuf::from(&candidate.path),
            explicit_tags: candidate.snapshot_tags.clone(),
            allow_name_inference: true,
            chunk_profile: crate::manifest::ArtifactSizingPreset::Chunk128KiB,
            bundle_profile: crate::manifest::BundleSizingPreset::Bundle8MiB,
            compression_algorithm: crate::manifest::CompressionKind::Zstd,
            compression_level: Some(3),
            storage_mode: crate::manifest::ArtifactStorageMode::Bundled,
            runtime_hints: crate::RuntimeResourceHints::default(),
            control: candidate_control,
        },
        store_state,
        build_pool,
    )
    .map(|_| ())
}

#[derive(Debug)]
struct SnapshotSourcesCandidateProgress {
    sink: Arc<dyn GutchProgressSink>,
    folder_name: String,
}

impl GutchProgressSink for SnapshotSourcesCandidateProgress {
    fn emit(&self, mut event: GutchProgressEvent) {
        event.operation = GutchOperation::SnapshotSourcesCapture;
        event.message = format!("{}: {}", self.folder_name, event.message);
        event.detail = Some(self.folder_name.clone());
        event.done = false;
        self.sink.emit(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GutchRepoPaths;
    use crate::hash::hash_hex;
    use crate::manifest::{self, snapshot::SnapshotManifest};
    use crate::repo::{
        discover_source_snapshot_operations, ensure_gutch_repo_layout, format_system_time_utc,
        write_snapshot_manifest_artifacts,
    };
    use std::time::{Duration, SystemTime};

    fn test_hash(label: &str) -> String {
        hash_hex(&crate::hash::hash_chunk(label.as_bytes()))
    }

    #[test]
    fn plan_snapshot_sources_capture_returns_empty_plan_without_sources_path() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
        ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

        let plan = plan_snapshot_sources_capture(&repo_root, None).unwrap();
        assert!(!plan.sources_ready);
        assert_eq!(plan.capturable_count, 0);
        assert_eq!(plan.refresh_count, 0);
        assert_eq!(plan.removable_count, 0);
    }

    #[test]
    fn plan_snapshot_sources_capture_detects_refresh_when_source_is_newer() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let source_root = temp.path().join("sources");
        let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
        ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

        let source_dir = source_root.join("game-v2.0.0-windows-x86_64-base-2026_07_14");
        std::fs::create_dir_all(&source_dir).unwrap();
        let game_path = source_dir.join("game.exe");
        std::fs::write(&game_path, b"same-content").unwrap();

        let candidate = discover_source_snapshot_operations(&repo_root, &source_root).unwrap();
        assert_eq!(candidate.len(), 1);

        let tags = candidate[0].snapshot_tags.clone();
        let snapshot_id = test_hash("existing-snapshot-id");
        let mut manifest = SnapshotManifest::new(snapshot_id.clone(), tags);
        manifest.source_modified_at =
            format_system_time_utc(SystemTime::now() - Duration::from_secs(3600));
        manifest.set_files(vec![manifest::FileManifest {
            path: "game.exe".into(),
            size: 12,
            executable: false,
            read_only: false,
            full_file_hash: test_hash("game.exe"),
            chunks: Vec::new(),
        }]);
        write_snapshot_manifest_artifacts(&repo_paths, &manifest).unwrap();

        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(&game_path, b"new-content").unwrap();

        let plan = plan_snapshot_sources_capture(&repo_root, Some(&source_root)).unwrap();
        assert_eq!(plan.refresh_count, 1);
        assert_eq!(plan.capturable_count, 0);
        assert_eq!(
            plan.to_refresh[0].folder_name,
            "game-v2.0.0-windows-x86_64-base-2026_07_14"
        );
    }
}
