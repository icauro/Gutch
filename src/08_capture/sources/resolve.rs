//! Resolves capture sources and the tags each one is captured with.

use super::config;
use crate::Result;
use crate::manifest::SnapshotTagSet;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug)]
pub struct ResolvedCaptureSource {
    pub path: PathBuf,
    pub tags: SnapshotTagSet,
}

pub fn resolve_capture_sources(
    discover_from: Option<PathBuf>,
    sources: Vec<PathBuf>,
    tags: SnapshotTagSet,
) -> Result<Vec<ResolvedCaptureSource>> {
    let single = discover_from.is_none();
    let sources = collect_capture_sources(discover_from, sources)?;
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

fn collect_capture_sources(
    discover_from: Option<PathBuf>,
    explicit_sources: Vec<PathBuf>,
) -> crate::Result<Vec<config::ResolvedSourceSnapshot>> {
    let mut sources: Vec<config::ResolvedSourceSnapshot> = Vec::new();
    if let Some(discover_from) = discover_from {
        if !discover_from.is_dir() {
            return Err(crate::Error::invalid_input(format!(
                "discovery folder is not a directory: {}",
                discover_from.display()
            )));
        }
        let config = config::SourceConfig::load_or_default(&discover_from)?;
        sources.extend(config.discover_snapshot_sources(&discover_from)?);
    }
    for path in explicit_sources {
        sources.push(config::ResolvedSourceSnapshot {
            path,
            config_tags: None,
        });
    }

    sources.sort_by(|left, right| left.path.cmp(&right.path));
    sources.dedup_by(|left, right| left.path == right.path);
    if sources.is_empty() {
        return Err(crate::Error::invalid_input(
            "capture requires at least one source or a discovery folder with snapshot folders",
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
        crate::Error::invalid_input(format!(
            "snapshot input has no parent directory: {}",
            dir.display()
        ))
    })?;
    let Some(config) = config::SourceConfig::load(parent)? else {
        return Ok(None);
    };
    let Some(name) = dir.file_name().and_then(|value| value.to_str()) else {
        return Err(crate::Error::invalid_input(format!(
            "snapshot input has no usable folder name: {}",
            dir.display()
        )));
    };
    match config.ingestion_mode {
        config::IngestionMode::Auto => Ok(None),
        config::IngestionMode::Explicit => {
            let entry = config
                .explicit_ingestion_rules
                .snapshots
                .iter()
                .find(|rule| {
                    config::normalize_relative_path(&rule.path).ok().as_deref() == Some(name)
                });
            match entry {
                Some(rule) => {
                    let tags = SnapshotTagSet::from_compact_strings(&rule.tags).map_err(|err| {
                        crate::Error::invalid_input(format!(
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
