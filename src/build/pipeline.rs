use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use rayon::prelude::*;
use tempfile::TempDir;

use crate::Result;
use crate::build_scan::{ScanDirectoryOptions, collect_scanned_chunks};
use crate::build_types::{
    BuildConfig, BuildProgressSink, BuildResult, BuildTimings, BuiltBundle, BuiltLooseChunk,
    ChunkPayload, ExistingChunkLocation,
};
use crate::bundle::BundleBuilder;
use crate::compress::compress_with_level;
use crate::hash::ChunkHash;
use crate::manifest::{self, ArtifactStorageMode, TargetTagSet, target::TargetManifest};
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
#[path = "tests.rs"]
mod tests;

impl Default for BundleSizingPolicy {
    fn default() -> Self {
        Self::new(MAX_BUNDLE_SIZE, PREFERRED_BUNDLE_SIZE)
    }
}

type ChunkPayloadEntry = (ChunkHash, ChunkPayload);
type PlannedChunkGroup = Vec<ChunkPayloadEntry>;

fn estimate_bundle_compression_ratio_from_probe(
    chunks: &[(ChunkHash, ChunkPayload)],
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

    if compression_profile.compression == manifest::CompressionKind::None {
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
}

struct PlannedBundleGroups {
    groups: Vec<PlannedChunkGroup>,
    sort_duration: std::time::Duration,
    sort_bytes: u64,
    probe_duration: std::time::Duration,
    probe_raw_bytes: u64,
}

fn plan_bundle_compressed_groups(
    mut chunks: Vec<ChunkPayloadEntry>,
    compression_profile: &manifest::CompressionProfile,
    bundle_profile: &manifest::BundleSizingProfile,
    bundle_policy: BundleSizingPolicy,
) -> Result<PlannedBundleGroups> {
    if chunks.is_empty() {
        return Ok(PlannedBundleGroups {
            groups: Vec::new(),
            sort_duration: std::time::Duration::ZERO,
            sort_bytes: 0,
            probe_duration: std::time::Duration::ZERO,
            probe_raw_bytes: 0,
        });
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

    Ok(PlannedBundleGroups {
        groups: planned_groups,
        sort_duration,
        sort_bytes,
        probe_duration,
        probe_raw_bytes,
    })
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

fn bundle_compression_worker_count(
    planned_bundle_count: usize,
    runtime_worker_count: usize,
) -> usize {
    if planned_bundle_count == 0 {
        1
    } else {
        runtime_worker_count.max(1).min(planned_bundle_count)
    }
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

fn build_bundled_chunks(
    planned_bundle_groups: PlannedBundleGroups,
    compression_profile: &manifest::CompressionProfile,
    runtime_worker_count: usize,
    progress: Option<&std::sync::Arc<dyn BuildProgressSink>>,
) -> Result<Vec<BuiltBundleCompressedGroup>> {
    if let Some(progress) = progress {
        progress.on_bundle_build_started(planned_bundle_groups.groups.len());
    }
    let compression_profile_for_work = compression_profile.clone();
    let progress_for_work = progress.cloned();
    let bundle_worker_count =
        bundle_compression_worker_count(planned_bundle_groups.groups.len(), runtime_worker_count);
    let build_bundle_work = || {
        planned_bundle_groups
            .groups
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

    install_in_thread_pool(bundle_worker_count, build_bundle_work)?
}

fn build_loose_chunks(
    chunks_to_compress: Vec<ChunkPayloadEntry>,
    compression_profile: &manifest::CompressionProfile,
    runtime_worker_count: usize,
    progress: Option<&std::sync::Arc<dyn BuildProgressSink>>,
) -> Result<Vec<BuiltLooseChunkResult>> {
    if let Some(progress) = progress {
        progress.on_bundle_build_started(chunks_to_compress.len());
    }
    let compression_profile_for_work = compression_profile.clone();
    let progress_for_work = progress.cloned();
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

    install_in_thread_pool(runtime_worker_count, build_loose_work)?
}

fn base_build_config(
    gutch_repo_id: &str,
    target_id: &str,
    target_tags: &TargetTagSet,
) -> BuildConfig<'static> {
    BuildConfig::with_ids(gutch_repo_id, target_id, target_tags)
}

fn build_loose_chunk(
    chunk_hash: ChunkHash,
    payload: ChunkPayload,
    compression_profile: &manifest::CompressionProfile,
    progress: Option<&dyn BuildProgressSink>,
) -> Result<BuiltLooseChunkResult> {
    let load_start = Instant::now();
    let raw_size = payload.raw_size();
    let bytes = payload.load_bytes()?;
    let payload_load = load_start.elapsed();

    let data = if compression_profile.compression == manifest::CompressionKind::None {
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
    planned_bundle: Vec<(ChunkHash, ChunkPayload)>,
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
    Ok(build_with_config(
        dir,
        &base_build_config(gutch_repo_id, target_id, target_tags),
    )?
    .0)
}

#[allow(clippy::too_many_arguments)]
pub fn build_with_config(
    dir: &Path,
    config: &BuildConfig<'_>,
) -> Result<(BuildResult, BuildTimings)> {
    let total_start = Instant::now();
    let compression_profile = config.compression_profile.clone();
    let mut timings = BuildTimings::default();
    let runtime_plan =
        plan_build_runtime(&config.runtime_hints, compression_profile.compression_unit);
    timings.runtime_plan = Some(runtime_plan.clone());
    let chunk_spool_dir = prepare_chunk_spool_dir(runtime_plan.chunk_storage_mode)?;

    let empty_previous_file_manifests = HashMap::new();
    let previous_file_manifests = config
        .previous_file_manifests
        .unwrap_or(&empty_previous_file_manifests);
    let empty_existing_chunk_locations = HashMap::new();
    let existing_chunk_locations = config
        .existing_chunk_locations
        .unwrap_or(&empty_existing_chunk_locations);
    let existing_chunk_locations_by_hash =
        parse_existing_chunk_locations(existing_chunk_locations)?;
    let empty_existing_bundle_ids = HashSet::new();
    let existing_bundle_ids = config
        .existing_bundle_ids
        .unwrap_or(&empty_existing_bundle_ids);

    let (mut file_manifests, unique_chunks, total_logical_size) = collect_scanned_chunks(
        dir,
        ScanDirectoryOptions {
            previous_file_manifests,
            chunking_profile: &config.chunk_profile,
            existing_chunk_locations: &existing_chunk_locations_by_hash,
            chunk_storage_mode: runtime_plan.chunk_storage_mode,
            chunk_spool_root: chunk_spool_dir.as_ref().map(|dir| dir.path()),
            worker_count: runtime_plan.worker_count,
            progress: config.progress.clone(),
        },
        &mut timings,
    )?;

    let mut total_compressed: u64 = 0;
    let mut compressed_estimate_seen: HashSet<ChunkHash> = HashSet::new();
    for fm in &file_manifests {
        for chunk_ref in &fm.chunks {
            let chunk_hash = ChunkHash::parse_hex(&chunk_ref.chunk_hash)?;
            if let Some(existing) = existing_chunk_locations_by_hash.get(&chunk_hash)
                && compressed_estimate_seen.insert(chunk_hash)
            {
                total_compressed += existing.compressed_size;
            }
        }
    }

    let mut chunks_to_compress: Vec<ChunkPayloadEntry> = Vec::new();

    let t = Instant::now();
    for (hash, payload) in unique_chunks {
        let raw_size = payload.raw_size();
        timings.dedupe_lookup_bytes += raw_size;

        if let Some(existing) = existing_chunk_locations_by_hash.get(&hash) {
            if compressed_estimate_seen.insert(hash) {
                total_compressed += existing.compressed_size;
            }
        } else {
            chunks_to_compress.push((hash, payload));
        }
    }
    timings.dedupe_lookup += t.elapsed();

    timings.compression_input_bytes = chunks_to_compress
        .iter()
        .map(|(_, payload)| payload.raw_size())
        .sum();

    let t = Instant::now();
    let mut bundles = Vec::new();
    let mut loose_chunks = Vec::new();
    let bundle_target_size_policy = BundleSizingPolicy::new(
        config.bundle_profile.bundle_max_size_bytes(),
        config.bundle_profile.bundle_preferred_size_bytes(),
    );

    match config.storage_mode {
        ArtifactStorageMode::Bundled => {
            if compression_profile.compression_unit != manifest::CompressionUnit::Bundle {
                return Err(crate::Error::manifest(
                    "gutch only supports bundle-compressed storage".to_string(),
                ));
            }

            let planned_bundle_groups = plan_bundle_compressed_groups(
                chunks_to_compress,
                &compression_profile,
                &config.bundle_profile,
                bundle_target_size_policy,
            )?;
            timings.bundle_compressed_sort += planned_bundle_groups.sort_duration;
            timings.bundle_compressed_sort_bytes += planned_bundle_groups.sort_bytes;
            timings.bundle_compressed_ratio_probe += planned_bundle_groups.probe_duration;
            timings.bundle_compressed_ratio_probe_bytes += planned_bundle_groups.probe_raw_bytes;
            let built_bundle_groups = build_bundled_chunks(
                planned_bundle_groups,
                &compression_profile,
                runtime_plan.worker_count,
                config.progress.as_ref(),
            )?;

            for built_bundle in built_bundle_groups {
                timings.bundle_compressed_payload_load += built_bundle.payload_load;
                timings.bundle_compressed_payload_load_bytes += built_bundle.payload_load_bytes;
                if !existing_bundle_ids.contains(&built_bundle.bundle_id) {
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
            let built_chunks = build_loose_chunks(
                chunks_to_compress,
                &compression_profile,
                runtime_plan.worker_count,
                config.progress.as_ref(),
            )?;

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

fn parse_existing_chunk_locations(
    existing_chunk_locations: &HashMap<String, ExistingChunkLocation>,
) -> Result<HashMap<ChunkHash, ExistingChunkLocation>> {
    existing_chunk_locations
        .iter()
        .map(|(chunk_hash, location)| Ok((ChunkHash::parse_hex(chunk_hash)?, location.clone())))
        .collect()
}

fn prepare_chunk_spool_dir(chunk_storage_mode: ChunkStorageMode) -> Result<Option<TempDir>> {
    match chunk_storage_mode {
        ChunkStorageMode::Memory => Ok(None),
        ChunkStorageMode::TempSpill => Ok(Some(tempfile::tempdir().map_err(|err| {
            crate::Error::manifest(format!("failed to create chunk spool directory: {}", err))
        })?)),
    }
}
