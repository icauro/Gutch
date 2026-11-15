//! Source selection and batch capture shared by hosts and the CLI.

pub(crate) mod config;
pub(crate) mod configured;
pub(crate) mod discovery;

use crate::Result;
use crate::artifact::profiles::BundleSizingProfile;
use crate::artifact::profiles::ChunkSizingProfile;
use crate::artifact::profiles::validate_packaging_sizes;
use crate::capture::contracts::SnapshotCapturePlan;
use crate::capture::contracts::SnapshotCaptureRequest;
use crate::capture::contracts::SnapshotCaptureResult;
use crate::capture::execute::capture_error;
use crate::capture::execute::capture_snapshot;
use crate::capture::plan::plan_snapshot_capture;
use crate::manifest::SnapshotTagSet;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug)]
pub struct ResolvedCaptureSource {
    pub path: PathBuf,
    pub tags: SnapshotTagSet,
}

pub fn resolve_capture_sources(
    source_root: Option<PathBuf>,
    source_dirs: Vec<PathBuf>,
    tags: SnapshotTagSet,
) -> Result<Vec<ResolvedCaptureSource>> {
    let single = source_root.is_none();
    let sources = collect_capture_sources(source_root, source_dirs)?;
    let single = single && sources.len() == 1;
    sources
        .into_iter()
        .map(|source| {
            let config_tags = if single {
                resolve_single_source_config_tags(&source.path)?
            } else {
                source.config_tags
            };
            let mut effective_tags = tags.clone();
            if let Some(config_tags) = config_tags {
                effective_tags.extend_missing_from(&config_tags);
            }
            Ok(ResolvedCaptureSource {
                path: source.path,
                tags: effective_tags,
            })
        })
        .collect()
}

pub fn plan_snapshot_captures(
    requests: &[SnapshotCaptureRequest],
) -> Result<Vec<SnapshotCapturePlan>> {
    requests
        .iter()
        .map(|request| {
            validate_packaging_sizes(
                &ChunkSizingProfile::for_preset(request.chunk_profile),
                &BundleSizingProfile::for_preset(request.bundle_profile),
                request.storage_mode,
            )?;
            plan_snapshot_capture(
                &request.repo_root,
                &request.source_dir,
                &request.explicit_tags,
                &request.control,
            )
        })
        .collect()
}

/// Validate every source first. Successful captures remain committed if a later capture fails.
pub fn capture_snapshots(
    requests: Vec<SnapshotCaptureRequest>,
) -> Result<Vec<SnapshotCaptureResult>> {
    plan_snapshot_captures(&requests).map_err(capture_error)?;
    requests.into_iter().map(capture_snapshot).collect()
}

fn collect_capture_sources(
    source_root: Option<PathBuf>,
    explicit_dirs: Vec<PathBuf>,
) -> crate::Result<Vec<self::config::ResolvedSourceSnapshot>> {
    let mut sources: Vec<self::config::ResolvedSourceSnapshot> = Vec::new();
    if let Some(source_root) = source_root {
        if !source_root.is_dir() {
            return Err(crate::Error::manifest(format!(
                "source root is not a directory: {}",
                source_root.display()
            )));
        }
        let config = self::config::SourceConfig::load_or_default(&source_root)?;
        sources.extend(config.discover_snapshot_sources(&source_root)?);
    }
    for dir in explicit_dirs {
        sources.push(self::config::ResolvedSourceSnapshot {
            path: dir.clone(),
            relative_path: dir,
            config_tags: None,
        });
    }

    sources.sort_by(|left, right| left.path.cmp(&right.path));
    sources.dedup_by(|left, right| left.path == right.path);
    if sources.is_empty() {
        return Err(crate::Error::manifest(
            "capture requires at least one source directory or a source root with snapshot folders",
        ));
    }

    Ok(sources)
}

/// Resolve the config contribution for a single `snapshot capture --source-dir` operation.
/// The config is read from the directory's parent when present; it is never
/// generated automatically for a single capture. Returns the entry's explicit
/// tags when the source is listed in explicit mode.
fn resolve_single_source_config_tags(dir: &Path) -> crate::Result<Option<SnapshotTagSet>> {
    let parent = dir.parent().ok_or_else(|| {
        crate::Error::manifest(format!(
            "snapshot input has no parent directory: {}",
            dir.display()
        ))
    })?;
    let Some(config) = self::config::SourceConfig::load(parent)? else {
        return Ok(None);
    };
    let Some(name) = dir.file_name().and_then(|value| value.to_str()) else {
        return Err(crate::Error::manifest(format!(
            "snapshot input has no usable folder name: {}",
            dir.display()
        )));
    };
    match config.ingestion_mode {
        self::config::IngestionMode::Auto => Ok(None),
        self::config::IngestionMode::Explicit => {
            let entry = config
                .explicit_ingestion_rules
                .snapshots
                .iter()
                .find(|rule| {
                    self::config::normalize_relative_path(&rule.path)
                        .ok()
                        .as_deref()
                        == Some(name)
                });
            match entry {
                Some(rule) => {
                    let tags = SnapshotTagSet::from_compact_strings(&rule.tags).map_err(|err| {
                        crate::Error::manifest(format!(
                            "invalid tags for explicit snapshot '{}': {err}",
                            rule.path
                        ))
                    })?;
                    Ok(Some(tags))
                }
                None => Ok(None),
            }
        }
    }
}
