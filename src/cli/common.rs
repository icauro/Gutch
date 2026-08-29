use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::PathBuf;

use serde::Serialize;
use tempfile::TempDir;

use crate::cli::reporting::{
    BuildTimingsSummary, OptionalOutputPaths, ReconstructTimingsSummary, VerifyTimingsSummary,
    write_stats_json_output,
};
use crate::cli::verbose_output_enabled;
use crate::hash::ChunkHash;
use g::repo::{required_bundle_ids as resolve_required_bundle_ids, required_chunk_hashes};
use gutch as g;

pub(crate) struct PreparedInputPath {
    pub(crate) dir: PathBuf,
    pub(crate) _tempdir: Option<TempDir>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SnapshotCaptureStats {
    pub(crate) command: &'static str,
    pub(crate) repo_root: String,
    pub(crate) input_path: String,
    pub(crate) snapshot_id: String,
    pub(crate) manifest_path: String,
    pub(crate) product_id: String,
    pub(crate) remote_repo_id: String,
    pub(crate) version_id: String,
    pub(crate) os: String,
    pub(crate) arch: String,
    pub(crate) storage_mode: String,
    pub(crate) file_count: usize,
    pub(crate) total_logical_size: u64,
    pub(crate) total_compressed_size_estimate: u64,
    pub(crate) required_chunk_count: usize,
    pub(crate) required_bundle_count: usize,
    pub(crate) generated_bundle_count: usize,
    pub(crate) generated_bundle_bytes: u64,
    pub(crate) generated_chunk_count: usize,
    pub(crate) generated_chunk_bytes: u64,
    pub(crate) reused_chunk_count: usize,
    pub(crate) new_chunk_count: usize,
    pub(crate) reused_chunk_percent: f64,
    pub(crate) reused_bundle_count: usize,
    pub(crate) reused_bundle_percent: f64,
    pub(crate) timings: BuildTimingsSummary,
    pub(crate) outputs: OptionalOutputPaths,
}

#[derive(Debug, Serialize)]
pub(crate) struct SnapshotCaptureManyStats {
    pub(crate) command: &'static str,
    pub(crate) repo_root: String,
    pub(crate) snapshot_count: usize,
    pub(crate) total_logical_size: u64,
    pub(crate) total_compressed_size_estimate: u64,
    pub(crate) generated_bundle_count: usize,
    pub(crate) generated_bundle_bytes: u64,
    pub(crate) generated_chunk_count: usize,
    pub(crate) generated_chunk_bytes: u64,
    pub(crate) total_build_seconds: f64,
    pub(crate) snapshots: Vec<SnapshotCaptureStats>,
    pub(crate) outputs: OptionalOutputPaths,
}

#[derive(Debug, Serialize)]
pub(crate) struct SnapshotVerifyStats {
    pub(crate) command: &'static str,
    pub(crate) repo_root: String,
    pub(crate) snapshot_id: String,
    pub(crate) destination_dir: String,
    pub(crate) verify_mode: String,
    pub(crate) expected_file_count: usize,
    pub(crate) missing_file_count: usize,
    pub(crate) corrupt_file_count: usize,
    pub(crate) extra_file_count: usize,
    pub(crate) verification_passed: bool,
    pub(crate) errors: Vec<String>,
    pub(crate) timings: VerifyTimingsSummary,
    pub(crate) outputs: OptionalOutputPaths,
}

#[derive(Debug, Serialize)]
pub(crate) struct SnapshotReconstructStats {
    pub(crate) command: &'static str,
    pub(crate) repo_root: String,
    pub(crate) snapshot_id: String,
    pub(crate) destination_dir: String,
    pub(crate) staging_dir: String,
    pub(crate) source_snapshot_id: Option<String>,
    pub(crate) source_destination_dir: Option<String>,
    pub(crate) file_count: usize,
    pub(crate) required_chunk_count: usize,
    pub(crate) required_bundle_count: usize,
    pub(crate) resolved_bundle_file_count: usize,
    pub(crate) reused_source_chunk_count: usize,
    pub(crate) reused_source_chunk_percent: f64,
    pub(crate) total_logical_size: u64,
    pub(crate) total_compressed_size_estimate: u64,
    pub(crate) reconstruct_timings: ReconstructTimingsSummary,
    pub(crate) verify_timings: VerifyTimingsSummary,
    pub(crate) outputs: OptionalOutputPaths,
}

pub(crate) fn write_report<T: Serialize>(
    stats_json_out: &Option<PathBuf>,
    stats: &T,
) -> g::Result<()> {
    write_stats_json_output(stats_json_out, stats)
}

pub(crate) fn prepare_input_path(path: &PathBuf) -> g::Result<PreparedInputPath> {
    if path.is_dir() {
        return Ok(PreparedInputPath {
            dir: path.clone(),
            _tempdir: None,
        });
    }

    if path.is_file() {
        let tempdir = tempfile::Builder::new().prefix("gutch_input_").tempdir()?;
        let filename = path.file_name().ok_or_else(|| {
            g::Error::manifest(format!("input file has no name: {}", path.display()))
        })?;
        let staged_path = tempdir.path().join(filename);
        std::fs::copy(path, &staged_path)?;
        return Ok(PreparedInputPath {
            dir: tempdir.path().to_path_buf(),
            _tempdir: Some(tempdir),
        });
    }

    Err(g::Error::manifest(format!(
        "input path does not exist or is unsupported: {}",
        path.display()
    )))
}

pub(crate) fn load_optional_rules(
    rules_path: &Option<PathBuf>,
) -> g::Result<Option<g::manifest::AutoTaggingRules>> {
    let Some(rules_path) = rules_path else {
        return Ok(None);
    };

    let rules_json = crate::helpers::read_to_string(rules_path)?;
    let rules: g::manifest::AutoTaggingRules = serde_json::from_str(&rules_json)?;
    Ok(Some(rules))
}

pub(crate) fn load_snapshot_manifest_from_repo(
    repo_root: &std::path::Path,
    snapshot_id: &str,
) -> g::Result<(
    g::GutchRepoPaths,
    g::manifest::gutch_repo::GutchRepoManifest,
    PathBuf,
    g::manifest::snapshot::SnapshotManifest,
)> {
    let (repo_paths, repo_manifest) = g::load_gutch_repo_manifest(repo_root)?;
    let manifest_path = g::snapshot_metadata_path(&repo_paths, snapshot_id)?;
    if !manifest_path.exists() {
        return Err(g::Error::manifest(format!(
            "snapshot manifest not found at {}",
            manifest_path.display()
        )));
    }

    let snapshot_manifest = g::load_snapshot_manifest(&repo_paths, snapshot_id)?;
    Ok((repo_paths, repo_manifest, manifest_path, snapshot_manifest))
}

pub(crate) fn count_required_chunks(
    snapshot_manifest: &g::manifest::snapshot::SnapshotManifest,
) -> usize {
    required_chunk_hashes(snapshot_manifest).len()
}

pub(crate) fn count_required_bundles_from_chunk_index(
    snapshot_manifest: &g::manifest::snapshot::SnapshotManifest,
    chunk_index: &g::manifest::ChunkIndex,
) -> usize {
    resolve_required_bundle_ids(snapshot_manifest, chunk_index).len()
}

pub(crate) fn count_required_bundles_for_build(
    snapshot_manifest: &g::manifest::snapshot::SnapshotManifest,
    existing_chunk_locations: &HashMap<String, g::ExistingChunkLocation>,
    generated_bundles: &[g::BuiltBundle],
    _generated_loose_chunks: &[g::BuiltLooseChunk],
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
    let Ok(required_chunks) = g::repo::required_chunk_hashes_typed(snapshot_manifest) else {
        return generated_bundles.len();
    };
    for chunk_hash in required_chunks {
        if let Some(existing) = existing_chunk_locations_by_hash.get(&chunk_hash)
            && existing.storage_mode == g::manifest::ArtifactStorageMode::Bundled
        {
            required_bundle_ids.insert(existing.bundle_id.clone());
        }
    }

    required_bundle_ids.len() + generated_bundles.len()
}

pub(crate) fn parse_bundle_downloads(bundle_args: &[String]) -> g::Result<Vec<g::BundleDownload>> {
    bundle_args
        .iter()
        .map(|bundle_arg| {
            let Some((bundle_id, url)) = bundle_arg.split_once('=') else {
                return Err(g::Error::manifest(format!(
                    "invalid --bundle value '{}'; use <bundle-id>=<url>",
                    bundle_arg
                )));
            };
            let bundle_id = bundle_id.trim();
            let url = url.trim();
            if bundle_id.is_empty() || url.is_empty() {
                return Err(g::Error::manifest(format!(
                    "invalid --bundle value '{}'; use <bundle-id>=<url>",
                    bundle_arg
                )));
            }

            Ok(g::BundleDownload {
                bundle_id: bundle_id.to_string(),
                url: url.to_string(),
            })
        })
        .collect()
}

pub(crate) fn write_snapshot_manifest(
    repo_paths: &g::GutchRepoPaths,
    snapshot_manifest: &g::manifest::snapshot::SnapshotManifest,
) -> g::Result<(PathBuf, u64)> {
    let (manifest_path, manifest_size) =
        g::write_snapshot_manifest_artifacts(repo_paths, snapshot_manifest)?;
    if verbose_output_enabled() {
        println!("wrote snapshot manifest: {}", manifest_path.display());
    }
    Ok((manifest_path, manifest_size))
}

pub(crate) fn write_bundles(
    bundles_root: &std::path::Path,
    bundles: &[g::BuiltBundle],
) -> g::Result<std::collections::HashMap<String, u64>> {
    let mut bundle_sizes = std::collections::HashMap::with_capacity(bundles.len());
    for bundle in bundles {
        let bundle_path = g::bundle_path_in_root(bundles_root, &bundle.bundle_id)?;
        crate::helpers::write(&bundle_path, &bundle.data)?;
        let bundle_size = bundle.data.len() as u64;
        bundle_sizes.insert(bundle.bundle_id.clone(), bundle_size);
        if verbose_output_enabled() {
            println!(
                "wrote bundle: {} ({} bytes)",
                bundle_path.display(),
                bundle.data.len()
            );
        }
    }
    Ok(bundle_sizes)
}

pub(crate) fn write_loose_chunks(
    chunks_root: &std::path::Path,
    chunks: &[g::BuiltLooseChunk],
) -> g::Result<std::collections::HashMap<String, u64>> {
    let mut chunk_sizes = std::collections::HashMap::with_capacity(chunks.len());
    for chunk in chunks {
        let chunk_hash = chunk.chunk_hash.to_string();
        let chunk_path = g::chunk_path_in_root(chunks_root, &chunk_hash)?;
        if let Some(parent) = chunk_path.parent() {
            crate::helpers::create_dir_all(parent)?;
        }
        crate::helpers::write(&chunk_path, &chunk.data)?;
        let chunk_size = chunk.data.len() as u64;
        chunk_sizes.insert(chunk_hash, chunk_size);
        if verbose_output_enabled() {
            println!(
                "wrote chunk: {} ({} bytes)",
                chunk_path.display(),
                chunk.data.len()
            );
        }
    }
    Ok(chunk_sizes)
}

pub fn prompt_yes_no(prompt: &str, default_yes: bool) -> g::Result<bool> {
    loop {
        let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
        print!("{} {}: ", prompt, suffix);
        io::stdout()
            .flush()
            .map_err(|err| g::Error::manifest(format!("io error: {}", err)))?;
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .map_err(|err| g::Error::manifest(format!("io error: {}", err)))?;
        let trimmed = line.trim().to_ascii_lowercase();
        if trimmed.is_empty() {
            return Ok(default_yes);
        }
        if trimmed == "y" || trimmed == "yes" {
            return Ok(true);
        }
        if trimmed == "n" || trimmed == "no" {
            return Ok(false);
        }
        eprintln!("enter y or n");
    }
}
