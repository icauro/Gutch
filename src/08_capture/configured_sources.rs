use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;

use crate::Result;
use crate::capture::reuse::load_capture_reuse_state;
use crate::capture::source_operations::SourceSnapshotOperationCandidate;
use crate::capture::source_operations::discover_source_snapshot_operations;
use crate::progress;
use crate::progress::GutchOperation;
use crate::progress::GutchProgressEvent;
use crate::progress::GutchProgressPhase;
use crate::progress::GutchProgressSink;
use crate::repo::status::RepoStatus;
use crate::repo::status::read_repo_status;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSourcesCaptureResult {
    pub captured_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSourcesCapturePlan {
    pub sources_ready: bool,
    pub candidate_count: usize,
    pub capturable_count: usize,
    #[serde(default)]
    pub to_capture: Vec<SourceSnapshotOperationCandidate>,
}

impl SnapshotSourcesCapturePlan {
    fn empty() -> Self {
        Self {
            sources_ready: false,
            candidate_count: 0,
            capturable_count: 0,
            to_capture: Vec::new(),
        }
    }

    pub fn has_changes(&self) -> bool {
        self.capturable_count > 0
    }

    pub fn up_to_date_count(&self) -> usize {
        self.candidate_count.saturating_sub(self.capturable_count)
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
    repo_status: Option<&RepoStatus>,
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

    // Multi-source capture only adds content-derived snapshots. Every
    // discovered candidate is captured, including ones whose tags equal stored
    // snapshots and ones discovery flagged with an error (the capture attempt
    // then reports that error instead of silently dropping the source).
    // Nothing is ever derived into removals: callers delete exact snapshot
    // IDs separately.
    // Validate the repo; stored snapshots no longer influence the plan.
    if repo_status.is_none() {
        read_repo_status(repo_root)?;
    }

    plan.candidate_count = candidates.len();
    plan.to_capture = candidates;
    plan.capturable_count = plan.to_capture.len();

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
    for candidate in &plan.to_capture {
        let tags = crate::repo::snapshot_tag_summary::format_snapshot_tag_summary(
            &candidate.snapshot_tags,
        );
        if candidate.error.is_some() {
            lines.push(format!(
                "Capture new snapshot from {} (tags {}): previously reported {}",
                candidate.folder_name,
                tags,
                candidate.error.as_deref().unwrap_or("an error")
            ));
        } else {
            lines.push(format!(
                "Capture new snapshot from {} (tags {})",
                candidate.folder_name, tags
            ));
        }
    }
    lines
}

pub fn capture_snapshot_sources(
    repo_root: &Path,
    sources_path: &Path,
    control: &crate::progress::GutchOperationControl,
) -> Result<SnapshotSourcesCaptureResult> {
    let operation_root = (repo_root).to_path_buf();
    crate::repo::coordination::write(&operation_root, || {
        control.checkpoint()?;
        let plan = plan_snapshot_sources_capture(repo_root, Some(sources_path))?;
        if !plan.sources_ready {
            return Err(crate::Error::manifest(
                "snapshot sources path is missing or invalid",
            ));
        }

        if !plan.has_changes() {
            return Ok(SnapshotSourcesCaptureResult { captured_count: 0 });
        }

        let total_steps = plan.to_capture.len();

        let build_pool = crate::capture::execute::build_capture_pool(
            control,
            &crate::runtime::RuntimeResourceHints::default(),
        )?;

        let repo_paths = crate::repo::paths::GutchRepoPaths::from_repo_root(repo_root);
        let _existing_state_span = control.profile_span_with_args(
            "capture.prepare_source.load_existing_store_state",
            || {
                std::collections::BTreeMap::from([(
                    "source_count".to_string(),
                    serde_json::json!(plan.to_capture.len()),
                )])
            },
        );
        let mut reuse_state = load_capture_reuse_state(&repo_paths, &build_pool, control)?;
        drop(_existing_state_span);

        let mut captured_count = 0usize;
        for (step_index, candidate) in plan.to_capture.iter().enumerate() {
            control.checkpoint()?;
            progress::emit_progress(
                control.progress(),
                GutchProgressEvent {
                    feedback: None,
                    operation: GutchOperation::SnapshotSourcesCapture,
                    phase: GutchProgressPhase::Capture,
                    message: format!("Capturing snapshot '{}'", candidate.folder_name),
                    current: step_index + 1,
                    total: total_steps,
                    transferred_bytes: 0,
                    total_bytes: 0,
                    detail: Some(candidate.folder_name.clone()),
                    done: false,
                },
            );
            capture_source_snapshot(repo_root, candidate, control, &mut reuse_state, &build_pool)?;
            captured_count += 1;
        }

        let _ = crate::repo::cleanup::cleanup_orphan_artifacts_in_repo(repo_root, control)?;

        progress::emit_progress(
            control.progress(),
            GutchProgressEvent {
                feedback: None,
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

        Ok(SnapshotSourcesCaptureResult { captured_count })
    })
    .map_err(super::execute::capture_error)
}

fn capture_source_snapshot(
    repo_root: &Path,
    candidate: &SourceSnapshotOperationCandidate,
    control: &crate::progress::GutchOperationControl,
    reuse_state: &mut crate::capture::reuse::CaptureReuseState,
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
    crate::capture::execute::capture_repo_snapshot_with_existing_reuse_state(
        crate::capture::contracts::SnapshotCaptureRequest {
            repo_root: repo_root.to_path_buf(),
            source_dir: std::path::PathBuf::from(&candidate.path),
            explicit_tags: candidate.snapshot_tags.clone(),
            chunk_profile: crate::artifact::profiles::ChunkSizingPreset::Chunk128KiB,
            chunker: crate::artifact::profiles::Chunker::FastCdc,
            bundle_profile: crate::artifact::profiles::BundleSizingPreset::Bundle8MiB,
            compression_algorithm: crate::artifact::profiles::CompressionKind::Zstd,
            compression_level: Some(3),
            storage_mode: crate::artifact::profiles::ArtifactStorageMode::Bundled,
            runtime_hints: crate::runtime::RuntimeResourceHints::default(),
            control: candidate_control,
        },
        reuse_state,
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
    use crate::capture::source_operations::discover_source_snapshot_operations;
    use crate::manifest;
    use crate::manifest::snapshot::SnapshotManifest;
    use crate::repo::paths::GutchRepoPaths;
    use crate::repo::paths::ensure_gutch_repo_layout;
    use crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts;
    use crate::snapshot::identity::format_system_time_utc;
    use std::time::Duration;
    use std::time::SystemTime;

    fn test_hash(label: &str) -> String {
        crate::artifact::hash::ChunkHash::of(label.as_bytes()).to_hex()
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
        assert!(plan.to_capture.is_empty());
    }

    #[test]
    fn plan_snapshot_sources_capture_keeps_tagless_sources_distinct() {
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
            chunks: vec![crate::manifest::ChunkRef {
                chunk_hash: crate::artifact::hash::ChunkHash::parse_hex(&test_hash("game.exe"))
                    .unwrap(),
                raw_size: 12,
            }],
        }]);
        manifest.snapshot_id =
            crate::snapshot::identity::compute_snapshot_id_from_entries(&manifest.entries);
        write_snapshot_manifest_artifacts(&repo_paths, &manifest).unwrap();
        crate::test_util::index_manifest(&repo_paths, &manifest);

        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(&game_path, b"new-content").unwrap();

        let plan = plan_snapshot_sources_capture(&repo_root, Some(&source_root)).unwrap();
        assert_eq!(plan.capturable_count, 1);
        assert_eq!(plan.to_capture.len(), 1);
    }

    #[test]
    fn equal_tag_sources_plan_without_removals() {
        use crate::capture::source_operations::SourceSnapshotOperationDisposition;

        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let source_root = temp.path().join("sources");
        let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
        ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

        let tags = manifest::SnapshotTagSet::from_compact_strings(&[
            "version=1.0.0".to_string(),
            "os=windows".to_string(),
        ])
        .unwrap();
        let candidate =
            |folder: &str| crate::capture::source_operations::SourceSnapshotOperationCandidate {
                path: source_root.join(folder).display().to_string(),
                folder_name: folder.to_string(),
                source_modified_at: String::new(),
                version_tag: String::from("1.0.0"),
                os: String::from("windows"),
                arch: String::new(),
                snapshot_tags: tags.clone(),
                disposition: SourceSnapshotOperationDisposition::Unchanged,
                error: None,
            };
        let plan = plan_snapshot_sources_capture_with_context(
            &repo_root,
            Some(&source_root),
            Some(&[candidate("game-a"), candidate("game-b")]),
            None,
        )
        .unwrap();
        assert_eq!(plan.candidate_count, 2);
        assert_eq!(plan.capturable_count, 2);
        assert_eq!(plan.to_capture.len(), 2);
    }

    #[test]
    fn pre_scan_contracts_carry_no_snapshot_id() {
        use crate::capture::contracts::SnapshotCapturePlan;

        let candidate = crate::capture::source_operations::SourceSnapshotOperationCandidate {
            path: "game-a".to_string(),
            folder_name: String::from("game-a"),
            source_modified_at: String::new(),
            version_tag: String::from("1.0.0"),
            os: String::from("windows"),
            arch: String::new(),
            snapshot_tags: manifest::SnapshotTagSet::from_compact_strings(&[
                "version=1.0.0".to_string()
            ])
            .unwrap(),
            disposition:
                crate::capture::source_operations::SourceSnapshotOperationDisposition::Unchanged,
            error: None,
        };
        let candidate_json = serde_json::to_value(&candidate).unwrap();
        assert!(
            candidate_json.get("snapshotId").is_none(),
            "candidate exposes tags as an ID: {candidate_json}"
        );

        let plan = SnapshotCapturePlan::from_candidate(candidate);
        let plan_json = serde_json::to_value(&plan).unwrap();
        assert!(
            plan_json.get("snapshotId").is_none(),
            "plan exposes tags as an ID: {plan_json}"
        );
    }
}
