use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{
    RepoCheckStatus, Result, SourceTargetInstallCandidate, check_gutch_repo,
    discover_source_target_installs, is_source_modified_at_newer,
};
use crate::manifest::TargetTagSet;
use crate::progress::{
    self, GutchOperation, GutchProgressEvent, GutchProgressPhase, GutchProgressSink,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetSourcesRefresh {
    pub folder_name: String,
    pub path: String,
    pub target_tags: TargetTagSet,
    pub replaced_target_ids: Vec<String>,
    pub source_modified_at: String,
}

#[derive(Debug, Clone, Copy)]
pub struct TargetSourcesSyncOptions {
    pub prune_removed_targets: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetSourcesSyncResult {
    pub inserted_count: usize,
    pub refreshed_count: usize,
    pub removed_count: usize,
    pub pruned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetSourcesSyncPlan {
    pub sources_ready: bool,
    pub candidate_count: usize,
    pub insertable_count: usize,
    pub refresh_count: usize,
    pub removable_count: usize,
    #[serde(default)]
    pub to_insert: Vec<SourceTargetInstallCandidate>,
    #[serde(default)]
    pub to_refresh: Vec<TargetSourcesRefresh>,
    #[serde(default)]
    pub to_apply: Vec<SourceTargetInstallCandidate>,
    #[serde(default)]
    pub to_remove: Vec<String>,
}

impl TargetSourcesSyncPlan {
    fn empty() -> Self {
        Self {
            sources_ready: false,
            candidate_count: 0,
            insertable_count: 0,
            refresh_count: 0,
            removable_count: 0,
            to_insert: Vec::new(),
            to_refresh: Vec::new(),
            to_apply: Vec::new(),
            to_remove: Vec::new(),
        }
    }

    pub fn has_changes(&self) -> bool {
        self.insertable_count > 0 || self.refresh_count > 0 || self.removable_count > 0
    }

    pub fn up_to_date_count(&self) -> usize {
        self.candidate_count
            .saturating_sub(self.insertable_count)
            .saturating_sub(self.refresh_count)
    }
}

pub fn plan_target_sources_sync(
    repo_root: &Path,
    sources_path: Option<&Path>,
) -> Result<TargetSourcesSyncPlan> {
    plan_target_sources_sync_with_context(repo_root, sources_path, None, None)
}

pub(crate) fn plan_target_sources_sync_with_context(
    repo_root: &Path,
    sources_path: Option<&Path>,
    candidates: Option<&[SourceTargetInstallCandidate]>,
    repo_status: Option<&RepoCheckStatus>,
) -> Result<TargetSourcesSyncPlan> {
    let Some(sources_path) = normalized_sources_path(sources_path) else {
        return Ok(TargetSourcesSyncPlan::empty());
    };

    let mut plan = TargetSourcesSyncPlan {
        sources_ready: true,
        ..TargetSourcesSyncPlan::empty()
    };

    let candidates = match candidates {
        Some(candidates) => candidates.to_vec(),
        None => match discover_source_target_installs(repo_root, &sources_path) {
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

    let candidate_tag_sets: Vec<TargetTagSet> = valid_candidates
        .iter()
        .map(|candidate| candidate.target_tags.clone())
        .collect();

    plan.candidate_count = valid_candidates.len();
    plan.to_remove = status
        .targets
        .iter()
        .filter(|target| {
            !candidate_tag_sets
                .iter()
                .any(|tags| tags == &target.target_tags)
        })
        .map(|target| target.target_id.clone())
        .collect();
    plan.removable_count = plan.to_remove.len();

    for candidate in valid_candidates {
        let matching_targets: Vec<_> = status
            .targets
            .iter()
            .filter(|target| target.target_tags == candidate.target_tags)
            .collect();

        if matching_targets.is_empty() {
            plan.to_insert.push(candidate.clone());
            plan.to_apply.push(candidate);
            continue;
        }

        let recorded_source_modified_at = matching_targets
            .first()
            .map(|target| target.source_modified_at.as_str())
            .unwrap_or_default();
        if !is_source_modified_at_newer(&candidate.source_modified_at, recorded_source_modified_at)
        {
            continue;
        }

        plan.to_refresh.push(TargetSourcesRefresh {
            folder_name: candidate.folder_name.clone(),
            path: candidate.path.clone(),
            target_tags: candidate.target_tags.clone(),
            replaced_target_ids: matching_targets
                .iter()
                .map(|target| target.target_id.clone())
                .collect(),
            source_modified_at: candidate.source_modified_at.clone(),
        });
        plan.to_apply.push(candidate);
    }

    plan.insertable_count = plan.to_insert.len();
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

pub fn describe_target_sources_sync_plan(plan: &TargetSourcesSyncPlan) -> Vec<String> {
    let mut lines = Vec::new();
    for refresh in &plan.to_refresh {
        lines.push(format!(
            "Refresh target slot for {} ({}): source modified at {}",
            refresh.folder_name, refresh.path, refresh.source_modified_at
        ));
    }
    for target_id in &plan.to_remove {
        lines.push(format!(
            "Remove repo target {} (missing from sources)",
            target_id
        ));
    }
    for candidate in &plan.to_insert {
        lines.push(format!(
            "Insert new target slot {} ({})",
            candidate.target_id, candidate.folder_name
        ));
    }
    lines
}

pub fn sync_target_sources(
    repo_root: &Path,
    sources_path: &Path,
    options: TargetSourcesSyncOptions,
    control: &crate::GutchOperationControl,
) -> Result<TargetSourcesSyncResult> {
    control.checkpoint()?;
    let plan = plan_target_sources_sync(repo_root, Some(sources_path))?;
    if !plan.sources_ready {
        return Err(crate::Error::manifest(
            "target sources path is missing or invalid",
        ));
    }

    if !plan.has_changes() {
        return Ok(TargetSourcesSyncResult {
            inserted_count: 0,
            refreshed_count: 0,
            removed_count: 0,
            pruned: options.prune_removed_targets,
        });
    }

    let total_steps = plan.to_apply.len() + plan.to_remove.len();
    let mut current_step = 0usize;
    let mut removed_count = 0usize;

    if options.prune_removed_targets {
        for target_id in &plan.to_remove {
            control.checkpoint()?;
            crate::remove_target_from_repo(repo_root, target_id)?;
            removed_count += 1;
            current_step += 1;
            progress::emit_progress(
                control.progress(),
                GutchProgressEvent {
                    operation: GutchOperation::TargetSourcesSync,
                    phase: GutchProgressPhase::Prune,
                    message: format!("Removing target '{}'", target_id),
                    current: current_step,
                    total: total_steps,
                    transferred_bytes: 0,
                    total_bytes: 0,
                    detail: Some(target_id.to_string()),
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

    let mut inserted_count = 0usize;
    let mut refreshed_count = 0usize;
    for candidate in &plan.to_apply {
        control.checkpoint()?;
        current_step += 1;
        progress::emit_progress(
            control.progress(),
            GutchProgressEvent {
                operation: GutchOperation::TargetSourcesSync,
                phase: GutchProgressPhase::Insert,
                message: format!("Committing target '{}'", candidate.folder_name),
                current: current_step,
                total: total_steps,
                transferred_bytes: 0,
                total_bytes: 0,
                detail: Some(candidate.folder_name.clone()),
                done: false,
            },
        );
        removed_count +=
            crate::delete_repo_targets_with_matching_tags(repo_root, &candidate.target_tags)?.len();
        insert_source_target_candidate(repo_root, candidate, control)?;
        if refresh_paths.contains(&candidate.path) {
            refreshed_count += 1;
        } else {
            inserted_count += 1;
        }
    }

    let _ = crate::cleanup_orphan_bundles_in_repo(repo_root, control)?;

    progress::emit_progress(
        control.progress(),
        GutchProgressEvent {
            operation: GutchOperation::TargetSourcesSync,
            phase: GutchProgressPhase::Done,
            message: "Target sources sync complete".to_string(),
            current: total_steps,
            total: total_steps,
            transferred_bytes: 0,
            total_bytes: 0,
            detail: None,
            done: true,
        },
    );

    Ok(TargetSourcesSyncResult {
        inserted_count,
        refreshed_count,
        removed_count,
        pruned: options.prune_removed_targets,
    })
}

fn insert_source_target_candidate(
    repo_root: &Path,
    candidate: &SourceTargetInstallCandidate,
    control: &crate::GutchOperationControl,
) -> Result<()> {
    let candidate_control = if let Some(sink) = control.progress() {
        control.with_replaced_progress(Arc::new(TargetSourcesCandidateProgress {
            sink: sink.clone(),
            folder_name: candidate.folder_name.clone(),
        }))
    } else {
        control.clone()
    };
    crate::insert_repo_target(crate::RepoTargetInsertRequest {
        repo_root: repo_root.to_path_buf(),
        source_dir: std::path::PathBuf::from(&candidate.path),
        explicit_tags: candidate.target_tags.clone(),
        allow_name_inference: true,
        chunk_profile: crate::manifest::ArtifactSizingPreset::Chunk128KiB,
        bundle_profile: crate::manifest::BundleSizingPreset::Bundle16MiB,
        compression_profile: crate::manifest::CompressionPreset::Zstd3,
        storage_mode: crate::manifest::ArtifactStorageMode::Bundled,
        runtime_hints: crate::RuntimeResourceHints::default(),
        control: candidate_control,
    })
    .map(|_| ())
}

#[derive(Debug)]
struct TargetSourcesCandidateProgress {
    sink: Arc<dyn GutchProgressSink>,
    folder_name: String,
}

impl GutchProgressSink for TargetSourcesCandidateProgress {
    fn emit(&self, mut event: GutchProgressEvent) {
        event.operation = GutchOperation::TargetSourcesSync;
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
    use crate::manifest::{self, target::TargetManifest};
    use crate::repo::{
        discover_source_target_installs, ensure_gutch_repo_layout, format_system_time_utc,
        write_target_manifest_artifacts,
    };
    use std::time::{Duration, SystemTime};

    fn test_hash(label: &str) -> String {
        hash_hex(&crate::hash::hash_chunk(label.as_bytes()))
    }

    #[test]
    fn plan_target_sources_sync_returns_empty_plan_without_sources_path() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
        ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

        let plan = plan_target_sources_sync(&repo_root, None).unwrap();
        assert!(!plan.sources_ready);
        assert_eq!(plan.insertable_count, 0);
        assert_eq!(plan.refresh_count, 0);
        assert_eq!(plan.removable_count, 0);
    }

    #[test]
    fn plan_target_sources_sync_detects_refresh_when_source_is_newer() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let source_root = temp.path().join("sources");
        let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
        ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

        let source_dir = source_root.join("game-v2.0.0-windows-x86_64-base-2026_07_14");
        std::fs::create_dir_all(&source_dir).unwrap();
        let game_path = source_dir.join("game.exe");
        std::fs::write(&game_path, b"same-content").unwrap();

        let candidate = discover_source_target_installs(&repo_root, &source_root).unwrap();
        assert_eq!(candidate.len(), 1);

        let tags = candidate[0].target_tags.clone();
        let target_id = test_hash("existing-content-id");
        let mut manifest = TargetManifest::new(target_id.clone(), tags);
        manifest.source_modified_at =
            format_system_time_utc(SystemTime::now() - Duration::from_secs(3600));
        manifest.files = vec![manifest::FileManifest {
            path: "game.exe".into(),
            size: 12,
            executable: false,
            full_file_hash: test_hash("game.exe"),
            chunks: Vec::new(),
        }];
        write_target_manifest_artifacts(&repo_paths, &manifest).unwrap();

        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(&game_path, b"new-content").unwrap();

        let plan = plan_target_sources_sync(&repo_root, Some(&source_root)).unwrap();
        assert_eq!(plan.refresh_count, 1);
        assert_eq!(plan.insertable_count, 0);
        assert_eq!(
            plan.to_refresh[0].folder_name,
            "game-v2.0.0-windows-x86_64-base-2026_07_14"
        );
    }
}
