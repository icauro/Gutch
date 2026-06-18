use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use rayon::prelude::*;
use tempfile::TempDir;

use crate::Result;
use crate::build_scan::collect_scanned_chunks;
use crate::build_types::{
    BuildConfig, BuildProgressSink, BuildResult, BuildTimings, BuiltBundle, BuiltLooseChunk,
    ChunkPayload, ExistingChunkLocation,
};
use crate::bundle::BundleBuilder;
use crate::compress::compress_with_level;
use crate::manifest::{
    self, ArtifactStorageMode, FileManifest, TargetTagSet, target::TargetManifest,
};
use crate::runtime::{ChunkStorageMode, install_in_thread_pool, plan_build_runtime};

const MAX_BUNDLE_SIZE: u64 = 64 * 1024 * 1024;
const PREFERRED_BUNDLE_SIZE: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
struct BundleSizingPolicy {
    max_size: u64,
    preferred_size: u64,
}

impl BundleSizingPolicy {
    fn new(max_size: u64, preferred_size: u64) -> Self {
        Self {
            max_size,
            preferred_size,
        }
    }

    fn target_size_for(&self, compressed_bytes: u64) -> u64 {
        if compressed_bytes == 0 {
            return self.preferred_size.min(self.max_size);
        }

        if compressed_bytes <= self.preferred_size {
            return compressed_bytes.max(self.preferred_size).min(self.max_size);
        }

        let bundle_count = compressed_bytes.div_ceil(self.preferred_size);
        compressed_bytes
            .div_ceil(bundle_count)
            .max(self.preferred_size)
            .min(self.max_size)
    }
}

#[cfg(test)]
#[path = "build_tests.rs"]
mod tests;

impl Default for BundleSizingPolicy {
    fn default() -> Self {
        Self::new(MAX_BUNDLE_SIZE, PREFERRED_BUNDLE_SIZE)
    }
}

fn estimate_bundle_compression_ratio_from_probe(
    chunks: &[(String, ChunkPayload)],
    compression_profile: &manifest::CompressionProfile,
    probe_raw_target_size: u64,
) -> Result<(f64, u64)> {
    if chunks.is_empty() {
        return Ok((1.0, 0));
    }

    let mut raw_probe_size: u64 = 0;
    let mut probe_payload = Vec::new();
    for (_, payload) in chunks {
        let bytes_added = payload.with_bytes(|data| {
            probe_payload.extend_from_slice(data);
            data.len() as u64
        })?;
        raw_probe_size += bytes_added;
        if raw_probe_size >= probe_raw_target_size {
            break;
        }
    }

    if raw_probe_size == 0 {
        return Ok((1.0, 0));
    }

    if compression_profile.compression == "none" {
        return Ok((1.0, raw_probe_size));
    }

    let compressed_probe = compress_with_level(&probe_payload, compression_profile.zstd_level)?;
    Ok((
        (compressed_probe.len() as f64 / raw_probe_size as f64).max(0.01),
        raw_probe_size,
    ))
}

fn raw_target_size_for_bundle_compression(
    compressed_target_size: u64,
    compression_ratio: f64,
    bundle_policy: BundleSizingPolicy,
) -> u64 {
    let normalized_ratio = compression_ratio.max(0.01);
    ((compressed_target_size as f64 / normalized_ratio).ceil() as u64)
        .max(bundle_policy.preferred_size)
        .min(u64::MAX)
}

fn plan_bundle_compressed_groups(
    mut chunks: Vec<(String, ChunkPayload)>,
    compression_profile: &manifest::CompressionProfile,
    bundle_profile: &manifest::BundleSizingProfile,
    bundle_policy: BundleSizingPolicy,
) -> Result<(
    Vec<Vec<(String, ChunkPayload)>>,
    std::time::Duration,
    u64,
    std::time::Duration,
    u64,
)> {
    if chunks.is_empty() {
        return Ok((
            Vec::new(),
            std::time::Duration::ZERO,
            0,
            std::time::Duration::ZERO,
            0,
        ));
    }

    let sort_start = Instant::now();
    let sort_bytes: u64 = chunks.iter().map(|(_, payload)| payload.raw_size()).sum();
    chunks.sort_by(|a, b| a.0.cmp(&b.0));
    let sort_duration = sort_start.elapsed();
    let probe_start = Instant::now();
    let (bundle_compression_ratio, probe_raw_bytes) = estimate_bundle_compression_ratio_from_probe(
        &chunks,
        compression_profile,
        bundle_profile.bundle_preferred_size_bytes(),
    )?;
    let probe_duration = probe_start.elapsed();
    let new_raw_bytes = sort_bytes;
    let estimated_total_compressed_bytes =
        ((new_raw_bytes as f64) * bundle_compression_ratio).ceil() as u64;
    let bundle_target_compressed_size =
        bundle_policy.target_size_for(estimated_total_compressed_bytes);
    let bundle_raw_target_size = raw_target_size_for_bundle_compression(
        bundle_target_compressed_size,
        bundle_compression_ratio,
        bundle_policy,
    );

    let mut planned_groups = Vec::new();
    let mut current_group = Vec::new();
    let mut current_group_raw_size: u64 = 0;

    for (hash, payload) in chunks {
        let raw_size = payload.raw_size();
        if !current_group.is_empty() && current_group_raw_size + raw_size > bundle_raw_target_size {
            planned_groups.push(current_group);
            current_group = Vec::new();
            current_group_raw_size = 0;
        }

        current_group.push((hash, payload));
        current_group_raw_size += raw_size;
    }

    if !current_group.is_empty() {
        planned_groups.push(current_group);
    }

    Ok((
        planned_groups,
        sort_duration,
        sort_bytes,
        probe_duration,
        probe_raw_bytes,
    ))
}

fn record_bundle_compressed_bundle_metrics(
    timings: &mut BuildTimings,
    total_compressed: &mut u64,
    bundle_data_len: u64,
    bundle_metrics: crate::bundle::BundleBuildMetrics,
) {
    timings.compression_output_bytes += bundle_data_len;
    *total_compressed += bundle_data_len;
    timings.bundle_compressed_payload_assembly += bundle_metrics.payload_assembly;
    timings.bundle_compressed_payload_assembly_bytes += bundle_metrics.payload_assembly_bytes;
    timings.bundle_compressed_stream_compression += bundle_metrics.stream_compression;
    timings.bundle_compressed_stream_input_bytes += bundle_metrics.stream_input_bytes;
    timings.bundle_compressed_stream_output_bytes += bundle_metrics.stream_output_bytes;
    timings.bundle_compressed_serialization += bundle_metrics.serialization;
    timings.bundle_compressed_serialization_bytes += bundle_metrics.serialization_bytes;
}

struct BuiltBundleCompressedGroup {
    bundle_id: String,
    bundle_data: Vec<u8>,
    bundle_metrics: crate::bundle::BundleBuildMetrics,
    payload_load: std::time::Duration,
    payload_load_bytes: u64,
}

struct BuiltLooseChunkResult {
    chunk: BuiltLooseChunk,
    payload_load: std::time::Duration,
    payload_load_bytes: u64,
}

fn build_loose_chunk(
    chunk_hash: String,
    payload: ChunkPayload,
    compression_profile: &manifest::CompressionProfile,
    progress: Option<&dyn BuildProgressSink>,
) -> Result<BuiltLooseChunkResult> {
    let load_start = Instant::now();
    let raw_size = payload.raw_size();
    let bytes = payload.load_bytes()?;
    let payload_load = load_start.elapsed();

    let data = if compression_profile.compression == "none" {
        bytes
    } else {
        compress_with_level(&bytes, compression_profile.zstd_level)?
    };
    let compressed_size = data.len() as u64;
    if let Some(progress) = progress {
        progress.on_bundle_built();
    }

    Ok(BuiltLooseChunkResult {
        chunk: BuiltLooseChunk {
            chunk_hash,
            data,
            raw_size,
            compressed_size,
        },
        payload_load,
        payload_load_bytes: raw_size,
    })
}

fn build_bundle_compressed_group(
    planned_bundle: Vec<(String, ChunkPayload)>,
    compression_profile: &manifest::CompressionProfile,
    progress: Option<&dyn BuildProgressSink>,
) -> Result<BuiltBundleCompressedGroup> {
    let mut builder = BundleBuilder::new(compression_profile.clone());
    let mut payload_load = std::time::Duration::ZERO;
    let mut payload_load_bytes: u64 = 0;

    for (hash, payload) in planned_bundle {
        let load_start = Instant::now();
        match payload {
            ChunkPayload::InMemory { bytes } => {
                payload_load += load_start.elapsed();
                payload_load_bytes += bytes.len() as u64;
                builder.add_shared_chunk(hash, bytes);
            }
            ChunkPayload::TempFile { raw_size, .. } => {
                let data = payload.load_bytes()?;
                payload_load += load_start.elapsed();
                payload_load_bytes += raw_size;
                builder.add_chunk(hash, data);
            }
        }
    }

    let (bundle_id, bundle_data, bundle_metrics) = builder.build_with_id_and_metrics()?;
    if let Some(progress) = progress {
        progress.on_bundle_built();
    }
    Ok(BuiltBundleCompressedGroup {
        bundle_id,
        bundle_data,
        bundle_metrics,
        payload_load,
        payload_load_bytes,
    })
}

pub fn build_from_directory(
    dir: &Path,
    gutch_repo_id: &str,
    target_id: &str,
    target_tags: &TargetTagSet,
) -> Result<BuildResult> {
    let (result, _) =
        build_from_directory_with_timings(dir, gutch_repo_id, target_id, target_tags)?;
    Ok(result)
}

pub fn build_from_directory_with_timings(
    dir: &Path,
    gutch_repo_id: &str,
    target_id: &str,
    target_tags: &TargetTagSet,
) -> Result<(BuildResult, BuildTimings)> {
    let config = BuildConfig::with_ids(gutch_repo_id, target_id, target_tags);
    build_with_config(dir, &config)
}

pub fn build_from_directory_with_timings_against_existing_bundles(
    dir: &Path,
    gutch_repo_id: &str,
    target_id: &str,
    target_tags: &TargetTagSet,
    existing_bundle_ids: &HashSet<String>,
) -> Result<(BuildResult, BuildTimings)> {
    let mut config = BuildConfig::with_ids(gutch_repo_id, target_id, target_tags);
    config.existing_bundle_ids = existing_bundle_ids.clone();
    build_with_config(dir, &config)
}

pub fn build_from_directory_with_timings_against_existing_store(
    dir: &Path,
    gutch_repo_id: &str,
    target_id: &str,
    target_tags: &TargetTagSet,
    existing_chunk_locations: &HashMap<String, ExistingChunkLocation>,
    existing_bundle_ids: &HashSet<String>,
) -> Result<(BuildResult, BuildTimings)> {
    let mut config = BuildConfig::with_ids(gutch_repo_id, target_id, target_tags);
    config.existing_chunk_locations = existing_chunk_locations.clone();
    config.existing_bundle_ids = existing_bundle_ids.clone();
    build_with_config(dir, &config)
}

pub fn build_from_directory_with_timings_against_existing_store_and_manifests(
    dir: &Path,
    gutch_repo_id: &str,
    target_id: &str,
    target_tags: &TargetTagSet,
    existing_chunk_locations: &HashMap<String, ExistingChunkLocation>,
    existing_bundle_ids: &HashSet<String>,
    previous_file_manifests: &HashMap<String, FileManifest>,
) -> Result<(BuildResult, BuildTimings)> {
    let mut config = BuildConfig::with_ids(gutch_repo_id, target_id, target_tags);
    config.existing_chunk_locations = existing_chunk_locations.clone();
    config.existing_bundle_ids = existing_bundle_ids.clone();
    config.previous_file_manifests = previous_file_manifests.clone();
    build_with_config(dir, &config)
}

pub fn build_from_directory_with_timings_against_existing_store_and_manifests_and_profile(
    dir: &Path,
    gutch_repo_id: &str,
    target_id: &str,
    target_tags: &TargetTagSet,
    existing_chunk_locations: &HashMap<String, ExistingChunkLocation>,
    existing_bundle_ids: &HashSet<String>,
    previous_file_manifests: &HashMap<String, FileManifest>,
    chunk_profile: &manifest::ChunkSizingProfile,
    bundle_profile: &manifest::BundleSizingProfile,
) -> Result<(BuildResult, BuildTimings)> {
    let mut config = BuildConfig::with_ids(gutch_repo_id, target_id, target_tags);
    config.existing_chunk_locations = existing_chunk_locations.clone();
    config.existing_bundle_ids = existing_bundle_ids.clone();
    config.previous_file_manifests = previous_file_manifests.clone();
    config.chunk_profile = chunk_profile.clone();
    config.bundle_profile = bundle_profile.clone();
    build_with_config(dir, &config)
}

pub fn build_from_directory_with_timings_against_existing_store_and_manifests_and_profiles(
    dir: &Path,
    gutch_repo_id: &str,
    target_id: &str,
    target_tags: &TargetTagSet,
    existing_chunk_locations: &HashMap<String, ExistingChunkLocation>,
    existing_bundle_ids: &HashSet<String>,
    previous_file_manifests: &HashMap<String, FileManifest>,
    chunk_profile: &manifest::ChunkSizingProfile,
    bundle_profile: &manifest::BundleSizingProfile,
    compression_profile: &manifest::CompressionProfile,
) -> Result<(BuildResult, BuildTimings)> {
    let mut config = BuildConfig::with_ids(gutch_repo_id, target_id, target_tags);
    config.existing_chunk_locations = existing_chunk_locations.clone();
    config.existing_bundle_ids = existing_bundle_ids.clone();
    config.previous_file_manifests = previous_file_manifests.clone();
    config.chunk_profile = chunk_profile.clone();
    config.bundle_profile = bundle_profile.clone();
    config.compression_profile = compression_profile.clone();
    build_with_config(dir, &config)
}

#[allow(clippy::too_many_arguments)]
pub fn build_with_config(dir: &Path, config: &BuildConfig) -> Result<(BuildResult, BuildTimings)> {
    let total_start = Instant::now();
    let compression_profile = config.compression_profile.clone();
    let mut timings = BuildTimings::default();
    let runtime_plan =
        plan_build_runtime(&config.runtime_hints, &compression_profile.compression_unit);
    timings.runtime_plan = Some(runtime_plan.clone());
    let chunk_spool_dir = prepare_chunk_spool_dir(runtime_plan.chunk_storage_mode)?;

    let (mut file_manifests, unique_chunks, total_logical_size) = collect_scanned_chunks(
        dir,
        &config.previous_file_manifests,
        &config.chunk_profile,
        &config.existing_chunk_locations,
        runtime_plan.chunk_storage_mode,
        chunk_spool_dir.as_ref().map(|dir| dir.path()),
        runtime_plan.worker_count,
        config.progress.clone(),
        &mut timings,
    )?;

    let mut all_chunk_hashes: HashSet<String> = HashSet::new();
    let mut required_bundle_set = Vec::new();
    let mut total_compressed: u64 = 0;
    let mut compressed_estimate_seen: HashSet<String> = HashSet::new();
    for fm in &file_manifests {
        for chunk_ref in &fm.chunks {
            all_chunk_hashes.insert(chunk_ref.chunk_hash.clone());
            if let Some(existing) = config.existing_chunk_locations.get(&chunk_ref.chunk_hash) {
                required_bundle_set.push(existing.bundle_id.clone());
                if compressed_estimate_seen.insert(chunk_ref.chunk_hash.clone()) {
                    total_compressed += existing.compressed_size;
                }
            }
        }
    }

    let mut chunks_to_compress = Vec::new();

    let t = Instant::now();
    for (hash, payload) in unique_chunks {
        let raw_size = payload.raw_size();
        timings.dedupe_lookup_bytes += raw_size;

        if let Some(existing) = config.existing_chunk_locations.get(&hash) {
            if compressed_estimate_seen.insert(hash.clone()) {
                total_compressed += existing.compressed_size;
            }
        } else {
            chunks_to_compress.push((hash, payload));
        }
    }
    timings.dedupe_lookup += t.elapsed();

    for (_, payload) in &chunks_to_compress {
        timings.compression_input_bytes += payload.raw_size();
    }

    let t = Instant::now();
    let mut bundles = Vec::new();
    let mut loose_chunks = Vec::new();
    let bundle_target_size_policy = BundleSizingPolicy::new(
        config.bundle_profile.bundle_max_size_bytes(),
        config.bundle_profile.bundle_preferred_size_bytes(),
    );

    match config.storage_mode {
        ArtifactStorageMode::Bundled => {
            if compression_profile.compression_unit != manifest::COMPRESSION_UNIT_BUNDLE {
                return Err(crate::Error::manifest(
                    "gutch only supports bundle-compressed storage".to_string(),
                ));
            }

            let (planned_bundles, sort_duration, sort_bytes, probe_duration, probe_raw_bytes) =
                plan_bundle_compressed_groups(
                    chunks_to_compress,
                    &compression_profile,
                    &config.bundle_profile,
                    bundle_target_size_policy,
                )?;
            timings.bundle_compressed_sort += sort_duration;
            timings.bundle_compressed_sort_bytes += sort_bytes;
            timings.bundle_compressed_ratio_probe += probe_duration;
            timings.bundle_compressed_ratio_probe_bytes += probe_raw_bytes;
            if let Some(progress) = &config.progress {
                progress.on_bundle_build_started(planned_bundles.len());
            }
            let compression_profile_for_work = compression_profile.clone();
            let progress_for_work = config.progress.clone();
            let build_bundle_work = || {
                planned_bundles
                    .into_par_iter()
                    .map(|planned_bundle| {
                        build_bundle_compressed_group(
                            planned_bundle,
                            &compression_profile_for_work,
                            progress_for_work.as_deref(),
                        )
                    })
                    .collect::<Result<Vec<_>>>()
            };
            let built_bundle_groups: Vec<BuiltBundleCompressedGroup> =
                install_in_thread_pool(runtime_plan.worker_count, build_bundle_work)??;

            for built_bundle in built_bundle_groups {
                timings.bundle_compressed_payload_load += built_bundle.payload_load;
                timings.bundle_compressed_payload_load_bytes += built_bundle.payload_load_bytes;
                required_bundle_set.push(built_bundle.bundle_id.clone());
                if !config.existing_bundle_ids.contains(&built_bundle.bundle_id) {
                    record_bundle_compressed_bundle_metrics(
                        &mut timings,
                        &mut total_compressed,
                        built_bundle.bundle_data.len() as u64,
                        built_bundle.bundle_metrics,
                    );
                    bundles.push(BuiltBundle {
                        bundle_id: built_bundle.bundle_id,
                        data: built_bundle.bundle_data,
                    });
                }
            }
        }
        ArtifactStorageMode::LooseChunks => {
            if let Some(progress) = &config.progress {
                progress.on_bundle_build_started(chunks_to_compress.len());
            }
            let compression_profile_for_work = compression_profile.clone();
            let progress_for_work = config.progress.clone();
            let build_loose_work = || {
                chunks_to_compress
                    .into_par_iter()
                    .map(|(chunk_hash, payload)| {
                        build_loose_chunk(
                            chunk_hash,
                            payload,
                            &compression_profile_for_work,
                            progress_for_work.as_deref(),
                        )
                    })
                    .collect::<Result<Vec<_>>>()
            };
            let built_chunks: Vec<BuiltLooseChunkResult> =
                install_in_thread_pool(runtime_plan.worker_count, build_loose_work)??;

            for built_chunk in built_chunks {
                timings.bundle_compressed_payload_load += built_chunk.payload_load;
                timings.bundle_compressed_payload_load_bytes += built_chunk.payload_load_bytes;
                timings.compression_output_bytes += built_chunk.chunk.compressed_size;
                total_compressed += built_chunk.chunk.compressed_size;
                loose_chunks.push(built_chunk.chunk);
            }
        }
    }

    timings.compression += t.elapsed();
    timings.total_compressed_bytes = total_compressed;
    timings.bundle_build += t.elapsed();
    timings.bundle_build_bytes = bundles.iter().map(|b| b.data.len() as u64).sum::<u64>()
        + loose_chunks
            .iter()
            .map(|chunk| chunk.data.len() as u64)
            .sum::<u64>();

    let t = Instant::now();
    file_manifests.sort_by(|a, b| a.path.cmp(&b.path));
    let mut vm = TargetManifest::new(config.target_id.clone(), config.target_tags.clone());
    vm.total_logical_size = total_logical_size;
    vm.total_compressed_size_estimate = total_compressed;
    vm.files = file_manifests;
    timings.target_manifest_finalize += t.elapsed();
    timings.total = total_start.elapsed();

    Ok((
        BuildResult {
            target_manifest: vm,
            bundles,
            loose_chunks,
        },
        timings,
    ))
}

fn prepare_chunk_spool_dir(chunk_storage_mode: ChunkStorageMode) -> Result<Option<TempDir>> {
    match chunk_storage_mode {
        ChunkStorageMode::Memory => Ok(None),
        ChunkStorageMode::TempSpill => Ok(Some(tempfile::tempdir().map_err(|err| {
            crate::Error::manifest(format!("failed to create chunk spool directory: {}", err))
        })?)),
    }
}
