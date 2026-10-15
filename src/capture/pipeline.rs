use crate::profile::ProfileInstant;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

use rayon::ThreadPool;
use rayon::prelude::*;

use crate::Result;
use crate::artifact::bundle::BundleBuilder;
use crate::artifact::compression as compress;
use crate::artifact::hash::ChunkHash;
use crate::artifact::profiles::ArtifactStorageMode;
use crate::capture::progress::BuildProgressTracker;
use crate::capture::scan::ChunkPlacementKey;
use crate::capture::scan::ScanDirectoryOptions;
use crate::capture::scan::SourceInventory;
use crate::capture::scan::collect_scanned_chunks;
use crate::capture::scan::scan_source_inventory;
use crate::capture::types::BuildConfig;
use crate::capture::types::BuildResult;
use crate::capture::types::BuildTimings;
use crate::capture::types::BuiltBundle;
use crate::capture::types::BuiltLooseChunk;
use crate::capture::types::ChunkPayload;
use crate::capture::types::ChunkPayloadReader;
use crate::capture::types::ExistingChunkLocation;
use crate::capture::types::StagedArtifactFile;
use crate::manifest;
use crate::manifest::SnapshotTagSet;
use crate::manifest::snapshot::SnapshotManifest;
use crate::runtime::build_thread_pool;
use crate::runtime::plan_runtime;

const MAX_BUNDLE_SIZE: u64 = 64 * 1024 * 1024;
const PREFERRED_BUNDLE_SIZE: u64 = 8 * 1024 * 1024;
const MIN_FILE_LOCALITY_SEGMENT_BYTES: u64 = 1024 * 1024;

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

    fn encoded_target_size(&self) -> u64 {
        self.preferred_size.min(self.max_size)
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
    compression_profile: &crate::artifact::profiles::CompressionProfile,
    probe_target_bytes: u64,
) -> Result<(f64, u64)> {
    if chunks.is_empty() {
        return Ok((1.0, 0));
    }

    let uncompressed =
        compression_profile.compression == crate::artifact::profiles::CompressionKind::None;
    let mut probe_chunk_count = 0;
    let mut raw_probe_size: u64 = 0;
    let mut probe_payload = Vec::new();
    let mut reader = ChunkPayloadReader::default();
    for (_, payload) in chunks {
        raw_probe_size += payload.raw_size();
        probe_chunk_count += 1;
        if !uncompressed {
            probe_payload.push(reader.load(payload)?);
        }
        if raw_probe_size >= probe_target_bytes {
            break;
        }
    }

    if raw_probe_size == 0 {
        return Ok((1.0, 0));
    }

    let encoded_payload_size = if uncompressed {
        compress::validate_profile(compression_profile)?;
        // No codec framing: the payload length is already exact. Actual packing
        // still reads and verifies every chunk against its scan-time hash.
        raw_probe_size
    } else {
        let segments: Vec<&[u8]> = probe_payload.iter().map(Vec::as_slice).collect();
        compress::compressed_size(&segments, compression_profile)?
    };
    let encoded_size =
        encoded_payload_size + crate::artifact::bundle::encoded_metadata_size(probe_chunk_count);
    Ok((
        (encoded_size as f64 / raw_probe_size as f64).max(0.01),
        if uncompressed { 0 } else { raw_probe_size },
    ))
}

fn estimate_bundle_raw_capacity(
    compressed_target_bytes: u64,
    compression_ratio: f64,
    bundle_policy: BundleSizingPolicy,
) -> u64 {
    let normalized_ratio = compression_ratio.max(0.01);
    ((compressed_target_bytes as f64 / normalized_ratio).ceil() as u64)
        .max(bundle_policy.preferred_size)
        .min(crate::artifact::format::MAX_RAW_BUNDLE_SIZE)
}

struct PlannedBundleGroups {
    encoded_limit: u64,
    groups: Vec<PlannedChunkGroup>,
    sort_duration: std::time::Duration,
    sort_bytes: u64,
    probe_duration: std::time::Duration,
    probe_raw_bytes: u64,
}

fn plan_bundle_compressed_groups(
    mut chunks: Vec<ChunkPayloadEntry>,
    chunk_placements: &HashMap<ChunkHash, ChunkPlacementKey>,
    compression_profile: &crate::artifact::profiles::CompressionProfile,
    bundle_profile: &crate::artifact::profiles::BundleSizingProfile,
    bundle_policy: BundleSizingPolicy,
    pool: &ThreadPool,
    control: &crate::progress::GutchOperationControl,
) -> Result<PlannedBundleGroups> {
    if chunks.is_empty() {
        return Ok(PlannedBundleGroups {
            encoded_limit: bundle_policy.max_size,
            groups: Vec::new(),
            sort_duration: std::time::Duration::ZERO,
            sort_bytes: 0,
            probe_duration: std::time::Duration::ZERO,
            probe_raw_bytes: 0,
        });
    }

    let sort_bytes: u64 = chunks.iter().map(|(_, payload)| payload.raw_size()).sum();
    let hash_sort_duration = {
        let _sort_span = control
            .profile_span_with_args("capture.plan_artifact_groups.sort_chunks_for_probe", || {
                capture_bytes_trace_args("chunk_count", chunks.len(), sort_bytes)
            });
        let sort_start = ProfileInstant::detailed(control.profiler());
        pool.install(|| chunks.par_sort_unstable_by(|left, right| left.0.cmp(&right.0)));
        sort_start.elapsed()
    };
    let (bundle_compression_ratio, probe_raw_bytes, probe_duration) = {
        let _probe_span = control.profile_span_with_args(
            "capture.plan_artifact_groups.estimate_compression_ratio",
            || {
                BTreeMap::from([(
                    "probe_target_bytes".to_string(),
                    serde_json::json!(bundle_profile.bundle_preferred_size_bytes()),
                )])
            },
        );
        let probe_start = ProfileInstant::detailed(control.profiler());
        let (ratio, raw_bytes) = estimate_bundle_compression_ratio_from_probe(
            &chunks,
            compression_profile,
            bundle_profile.bundle_preferred_size_bytes(),
        )?;
        (ratio, raw_bytes, probe_start.elapsed())
    };
    let bundle_compressed_size = bundle_policy.encoded_target_size();
    let bundle_raw_capacity = estimate_bundle_raw_capacity(
        bundle_compressed_size,
        bundle_compression_ratio,
        bundle_policy,
    );
    let locality_segment_size_bytes = (bundle_raw_capacity / 4)
        .max(MIN_FILE_LOCALITY_SEGMENT_BYTES)
        .min(bundle_raw_capacity);
    let locality_sort_duration = {
        let _sort_span = control.profile_span_with_args(
            "capture.plan_artifact_groups.sort_chunks_by_file_locality",
            || {
                let mut args = capture_bytes_trace_args("chunk_count", chunks.len(), sort_bytes);
                args.insert(
                    "ordering".to_string(),
                    serde_json::json!(
                        "file_locality_segment,file_content_hash,file_offset,chunk_hash"
                    ),
                );
                args.insert(
                    "locality_segment_bytes".to_string(),
                    serde_json::json!(locality_segment_size_bytes),
                );
                args
            },
        );
        let sort_start = ProfileInstant::detailed(control.profiler());
        sort_chunks_by_file_locality(
            &mut chunks,
            chunk_placements,
            locality_segment_size_bytes,
            pool,
        )?;
        sort_start.elapsed()
    };

    let planned_groups = {
        let _group_span = control.profile_span_with_args(
            "capture.plan_artifact_groups.assign_chunks_to_bundles",
            || capture_bytes_trace_args("chunk_count", chunks.len(), sort_bytes),
        );
        let mut planned_groups = Vec::new();
        let mut current_group = Vec::new();
        let mut current_group_raw_size: u64 = 0;

        for (hash, payload) in chunks {
            let raw_size = payload.raw_size();

            if !current_group.is_empty() && current_group_raw_size + raw_size > bundle_raw_capacity
            {
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
        planned_groups
    };

    Ok(PlannedBundleGroups {
        encoded_limit: bundle_policy.max_size,
        groups: planned_groups,
        sort_duration: hash_sort_duration + locality_sort_duration,
        sort_bytes,
        probe_duration,
        probe_raw_bytes,
    })
}

fn sort_chunks_by_file_locality(
    chunks: &mut [ChunkPayloadEntry],
    chunk_placements: &HashMap<ChunkHash, ChunkPlacementKey>,
    locality_segment_size_bytes: u64,
    pool: &ThreadPool,
) -> Result<()> {
    if let Some((hash, _)) = chunks
        .iter()
        .find(|(hash, _)| !chunk_placements.contains_key(hash))
    {
        return Err(crate::Error::manifest(format!(
            "missing file placement metadata for new chunk '{hash}'"
        )));
    }

    pool.install(|| {
        chunks.par_sort_unstable_by(|left, right| {
            let left_placement = chunk_placements
                .get(&left.0)
                .expect("chunk placement metadata validated before sorting");
            let right_placement = chunk_placements
                .get(&right.0)
                .expect("chunk placement metadata validated before sorting");
            (left_placement.file_offset / locality_segment_size_bytes)
                .cmp(&(right_placement.file_offset / locality_segment_size_bytes))
                .then_with(|| {
                    left_placement
                        .file_content_hash
                        .cmp(&right_placement.file_content_hash)
                })
                .then_with(|| left_placement.file_offset.cmp(&right_placement.file_offset))
                .then_with(|| left.0.cmp(&right.0))
        });
    });
    Ok(())
}

fn record_bundle_compressed_bundle_metrics(
    timings: &mut BuildTimings,
    total_compressed: &mut u64,
    bundle_data_len: u64,
    bundle_metrics: crate::artifact::bundle::BundleBuildMetrics,
) {
    timings.compression_output_bytes += bundle_data_len;
    *total_compressed += bundle_data_len;
    timings.bundle_compressed_frame_compression += bundle_metrics.frame_compression;
    timings.bundle_compressed_frame_input_bytes += bundle_metrics.frame_input_bytes;
    timings.bundle_compressed_frame_output_bytes += bundle_metrics.frame_output_bytes;
    timings.bundle_compressed_footer_serialization += bundle_metrics.footer_serialization;
    timings.bundle_compressed_footer_serialization_bytes +=
        bundle_metrics.footer_serialization_bytes;
}

struct BuiltBundleCompressedGroup {
    bundle_id: String,
    staged_file: StagedArtifactFile,
    bundle_size: u64,
    bundle_entries: Vec<crate::artifact::bundle::BundleTableEntry>,
    bundle_metrics: crate::artifact::bundle::BundleBuildMetrics,
    payload_load: std::time::Duration,
    payload_load_bytes: u64,
}

struct BuiltLooseChunkResult {
    chunk: BuiltLooseChunk,
    payload_load: std::time::Duration,
    payload_load_bytes: u64,
}

fn run_parallel_chunk_work<T, U, F>(
    work_items: Vec<T>,
    pool: &ThreadPool,
    control: &crate::progress::GutchOperationControl,
    memory_budget: u64,
    working_bytes: impl Fn(&T) -> u64,
    work: F,
) -> Result<Vec<U>>
where
    T: Send,
    U: Send,
    F: Fn(T, &crate::progress::GutchOperationControl) -> Result<U> + Send + Sync,
{
    control.checkpoint()?;
    control.profile_event_with_args("runtime.memory.budget", || {
        BTreeMap::from([("budget_bytes".into(), serde_json::json!(memory_budget))])
    });
    let memory = std::sync::Arc::new(crate::runtime::MemoryBudget::new(memory_budget));
    let results = (0..work_items.len())
        .map(|_| std::sync::Mutex::new(None))
        .collect::<Vec<_>>();
    // in_place_scope keeps admission on the caller, not on a Rayon worker.
    pool.in_place_scope(|scope| -> Result<()> {
        for (item, slot) in work_items.into_iter().zip(&results) {
            let permit = memory.acquire(working_bytes(&item), control)?;
            let work = &work;
            scope.spawn(move |_| {
                let result = control.checkpoint().and_then(|_| work(item, control));
                *slot.lock().expect("capture result slot") = Some(result);
                drop(permit);
            });
        }
        Ok(())
    })?;
    results
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .expect("capture result slot")
                .expect("joined capture worker")
        })
        .collect()
}

fn build_bundled_chunks(
    planned_bundle_groups: PlannedBundleGroups,
    compression_profile: &crate::artifact::profiles::CompressionProfile,
    artifact_staging_root: &Path,
    pool: &ThreadPool,
    control: &crate::progress::GutchOperationControl,
    memory_budget: u64,
) -> Result<Vec<BuiltBundleCompressedGroup>> {
    control.checkpoint()?;
    let progress = BuildProgressTracker::new(
        control,
        crate::progress::GutchProgressPhase::Capture,
        "Building bundles",
    );
    progress.start(planned_bundle_groups.groups.len());
    let compression_profile_for_work = compression_profile.clone();
    let artifact_staging_root = artifact_staging_root.to_path_buf();
    let progress_for_work = progress.clone();
    let encoded_limit = planned_bundle_groups.encoded_limit;
    let work_items = planned_bundle_groups
        .groups
        .into_iter()
        .enumerate()
        .collect::<Vec<_>>();
    run_parallel_chunk_work(
        work_items,
        pool,
        control,
        memory_budget,
        |(_, bundle)| {
            bundle
                .iter()
                .map(|(_, payload)| payload.raw_size())
                .sum::<u64>()
                + crate::artifact::compression::ENCODE_WORKING_MEMORY_BYTES
        },
        move |(_bundle_index, planned_bundle), control| {
            build_bounded_bundle_groups(
                planned_bundle,
                encoded_limit,
                &compression_profile_for_work,
                &artifact_staging_root,
                control,
            )
            .inspect(|_| {
                progress_for_work.completed();
            })
        },
    )
    .map(|groups| groups.into_iter().flatten().collect())
}

fn build_loose_chunks(
    chunks_to_compress: Vec<ChunkPayloadEntry>,
    artifact_staging_root: &Path,
    compression_profile: &crate::artifact::profiles::CompressionProfile,
    pool: &ThreadPool,
    control: &crate::progress::GutchOperationControl,
    memory_budget: u64,
) -> Result<Vec<BuiltLooseChunkResult>> {
    control.checkpoint()?;
    let progress = BuildProgressTracker::new(
        control,
        crate::progress::GutchProgressPhase::Capture,
        "Building bundles",
    );
    progress.start(chunks_to_compress.len());
    let compression_profile_for_work = compression_profile.clone();
    let progress_for_work = progress.clone();

    run_parallel_chunk_work(
        chunks_to_compress,
        pool,
        control,
        memory_budget,
        |(_, payload)| {
            payload.raw_size() + crate::artifact::compression::ENCODE_WORKING_MEMORY_BYTES
        },
        move |(chunk_hash, payload), control| {
            build_loose_chunk(
                chunk_hash,
                payload,
                &compression_profile_for_work,
                artifact_staging_root,
                control,
            )
            .inspect(|_| {
                progress_for_work.completed();
            })
        },
    )
}

fn capture_bytes_trace_args(
    count_name: &str,
    count: usize,
    bytes: u64,
) -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([
        (count_name.to_string(), serde_json::json!(count)),
        ("size_bytes".to_string(), serde_json::json!(bytes)),
    ])
}

fn build_loose_chunk(
    chunk_hash: ChunkHash,
    payload: ChunkPayload,
    compression_profile: &crate::artifact::profiles::CompressionProfile,
    artifact_staging_root: &Path,
    control: &crate::progress::GutchOperationControl,
) -> Result<BuiltLooseChunkResult> {
    control.checkpoint()?;
    let load_start = ProfileInstant::detailed(control.profiler());
    let raw_size = payload.raw_size();
    let _artifact_span = control
        .profile_span_with_args("capture.loose.load_encode_and_write", || {
            BTreeMap::from([("size_bytes".into(), serde_json::json!(raw_size))])
        });
    let bytes = payload.load_bytes()?;
    let payload_load = load_start.elapsed();

    let mut staged = tempfile::NamedTempFile::new_in(artifact_staging_root)?;
    crate::artifact::loose::write_loose_chunk(
        &mut staged,
        chunk_hash,
        &bytes,
        compression_profile,
    )?;
    staged.as_file().sync_all()?;
    let compressed_size = staged.as_file().metadata()?.len();
    crate::artifact::format::size_u32(compressed_size)?;
    let staged_file = StagedArtifactFile::new(
        staged
            .into_temp_path()
            .keep()
            .map_err(|error| error.error)?,
    );
    Ok(BuiltLooseChunkResult {
        chunk: BuiltLooseChunk {
            chunk_hash,
            staged_file,
            raw_size,
            compressed_size,
        },
        payload_load,
        payload_load_bytes: raw_size,
    })
}

fn build_bounded_bundle_groups(
    planned: Vec<(ChunkHash, ChunkPayload)>,
    encoded_limit: u64,
    profile: &crate::artifact::profiles::CompressionProfile,
    staging: &Path,
    control: &crate::progress::GutchOperationControl,
) -> Result<Vec<BuiltBundleCompressedGroup>> {
    let built = build_bundle_compressed_group(planned.clone(), profile, staging, control)?;
    if built.bundle_size <= encoded_limit {
        return Ok(vec![built]);
    }
    drop(built);
    if planned.len() < 2 {
        return Err(crate::Error::bundle(
            "single chunk cannot fit encoded bundle limit",
        ));
    }
    let mut left = planned;
    let right = left.split_off(left.len() / 2);
    let mut result = build_bounded_bundle_groups(left, encoded_limit, profile, staging, control)?;
    result.extend(build_bounded_bundle_groups(
        right,
        encoded_limit,
        profile,
        staging,
        control,
    )?);
    Ok(result)
}

fn build_bundle_compressed_group(
    planned_bundle: Vec<(ChunkHash, ChunkPayload)>,
    compression_profile: &crate::artifact::profiles::CompressionProfile,
    artifact_staging_root: &Path,
    control: &crate::progress::GutchOperationControl,
) -> Result<BuiltBundleCompressedGroup> {
    control.checkpoint()?;
    let _job_span = control.profile_span_with_args("capture.bundle.build", || {
        BTreeMap::from([(
            "chunk_count".into(),
            serde_json::json!(planned_bundle.len()),
        )])
    });
    let mut builder = BundleBuilder::new(compression_profile.clone());
    let mut payload_load = std::time::Duration::ZERO;
    let mut payload_load_bytes: u64 = 0;

    let input_span = control.profile_span("capture.bundle.load_inputs");
    let mut reader = ChunkPayloadReader::default();
    for (hash, payload) in planned_bundle {
        control.checkpoint()?;
        let load_start = ProfileInstant::detailed(control.profiler());
        let bytes = reader.load(&payload)?;
        payload_load += load_start.elapsed();
        payload_load_bytes += bytes.len() as u64;
        builder.add_chunk(hash, bytes);
    }

    drop(reader);
    drop(input_span);
    let built = builder.write_to_staging(artifact_staging_root, control.profiler())?;
    Ok(BuiltBundleCompressedGroup {
        bundle_id: built.bundle_id,
        staged_file: StagedArtifactFile::new(built.staged_path),
        bundle_size: built.size,
        bundle_entries: built.entries,
        bundle_metrics: built.metrics,
        payload_load,
        payload_load_bytes,
    })
}

pub fn build_from_directory(dir: &Path, snapshot_tags: &SnapshotTagSet) -> Result<BuildResult> {
    Ok(build_with_config_timed(dir, &BuildConfig::for_snapshot(snapshot_tags))?.0)
}

pub(crate) fn build_with_config_timed(
    dir: &Path,
    config: &BuildConfig<'_>,
) -> Result<(BuildResult, BuildTimings)> {
    let runtime_plan = plan_runtime(&config.runtime_hints);
    let _pool_span = config
        .control
        .profile_span_with_args("capture.create_worker_pool", || {
            BTreeMap::from([(
                "worker_count".to_string(),
                serde_json::json!(runtime_plan.worker_count),
            )])
        });
    let build_pool = build_thread_pool(runtime_plan.worker_count, "Capture worker")?;
    drop(_pool_span);
    build_with_config_timed_on_pool(dir, config, &build_pool)
}

pub(crate) fn build_with_config_timed_on_pool(
    dir: &Path,
    config: &BuildConfig<'_>,
    build_pool: &ThreadPool,
) -> Result<(BuildResult, BuildTimings)> {
    let inventory = {
        let _inventory_span = config.control.profile_span("capture.scan_source.inventory");
        scan_source_inventory(
            dir,
            &config.control,
            plan_runtime(&config.runtime_hints).worker_count,
            build_pool,
        )?
    };
    build_with_config_timed_on_pool_with_inventory(dir, config, build_pool, &inventory)
}

pub(crate) fn build_with_config_timed_on_pool_with_inventory(
    _dir: &Path,
    config: &BuildConfig<'_>,
    build_pool: &ThreadPool,
    inventory: &SourceInventory,
) -> Result<(BuildResult, BuildTimings)> {
    crate::artifact::profiles::validate_packaging_sizes(
        &config.chunk_profile,
        &config.bundle_profile,
        config.storage_mode,
    )?;
    compress::validate_profile(&config.compression_profile)?;
    config.control.checkpoint()?;
    let _operation_span = config.control.profile_span("capture.build_snapshot");
    let total_start = ProfileInstant::start(config.control.profiler());
    let mut timings = BuildTimings {
        storage_mode: config.storage_mode,
        ..BuildTimings::default()
    };
    let runtime_plan = plan_runtime(&config.runtime_hints);
    timings.runtime_plan = Some(runtime_plan.clone());

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

    let scan_chunk_start = ProfileInstant::start(config.control.profiler());
    let crate::capture::scan::ScannedChunks {
        entries: mut snapshot_entries,
        unique_chunks,
        chunk_placements,
        total_logical_size,
    } = {
        let _scan_span = config.control.profile_span("capture.scan_source");
        collect_scanned_chunks(
            inventory,
            ScanDirectoryOptions {
                previous_file_manifests,
                chunking_profile: &config.chunk_profile,
                existing_chunk_locations: &existing_chunk_locations_by_hash,
                worker_count: runtime_plan.worker_count,
                worker_pool: build_pool,
                control: config.control.clone(),
            },
            &mut timings,
        )?
    };
    timings.scan_chunk_wall = scan_chunk_start.elapsed();

    let DeduplicatedChunks {
        mut total_compressed,
        chunks_to_compress,
    } = plan_deduplicated_chunks(
        config,
        &snapshot_entries,
        unique_chunks,
        &existing_chunk_locations_by_hash,
        &mut timings,
    )?;

    timings.compression_input_bytes = chunks_to_compress
        .iter()
        .map(|(_, payload)| payload.raw_size())
        .sum();

    let (bundles, loose_chunks) = build_capture_artifacts(
        config,
        build_pool,
        chunks_to_compress,
        &chunk_placements,
        runtime_plan.memory_budget_bytes,
        &mut timings,
        &mut total_compressed,
    )?;
    timings.total_compressed_bytes = total_compressed;
    timings.artifact_output_bytes = bundles.iter().map(|b| b.size).sum::<u64>()
        + loose_chunks
            .iter()
            .map(|chunk| chunk.compressed_size)
            .sum::<u64>();

    let manifest_finalize_start = ProfileInstant::start(config.control.profiler());
    let snapshot_manifest = {
        let _manifest_span = config
            .control
            .profile_span("capture.build_snapshot_manifest");
        {
            let _sort_span = config
                .control
                .profile_span_with_args("capture.build_snapshot_manifest.sort_entries", || {
                    capture_bytes_trace_args("entry_count", snapshot_entries.len(), 0)
                });
            snapshot_entries.sort_by(|a, b| a.path().cmp(b.path()));
        }
        let _construct_span = config
            .control
            .profile_span("capture.build_snapshot_manifest.construct");
        let mut snapshot_manifest = SnapshotManifest::new(
            crate::snapshot::identity::compute_snapshot_id_from_entries(&snapshot_entries),
            config.snapshot_tags.clone(),
        );
        snapshot_manifest.chunker = config.chunk_profile.chunker;
        snapshot_manifest.total_logical_size = total_logical_size;
        snapshot_manifest.total_compressed_size_estimate = total_compressed;
        snapshot_manifest.entries = snapshot_entries;
        snapshot_manifest
    };
    timings.manifest_finalize_wall = manifest_finalize_start.elapsed();
    timings.total = total_start.elapsed();

    Ok((
        BuildResult {
            snapshot_manifest,
            bundles,
            loose_chunks,
        },
        timings,
    ))
}

struct DeduplicatedChunks {
    total_compressed: u64,
    chunks_to_compress: Vec<ChunkPayloadEntry>,
}

fn plan_deduplicated_chunks(
    config: &BuildConfig<'_>,
    snapshot_entries: &[manifest::SnapshotEntry],
    unique_chunks: HashMap<ChunkHash, ChunkPayload>,
    existing_chunk_locations_by_hash: &HashMap<ChunkHash, ExistingChunkLocation>,
    timings: &mut BuildTimings,
) -> Result<DeduplicatedChunks> {
    let dedupe_plan_start = ProfileInstant::start(config.control.profiler());
    let mut total_compressed: u64 = 0;
    let mut compressed_estimate_seen: HashSet<ChunkHash> = HashSet::new();
    let mut chunks_to_compress: Vec<ChunkPayloadEntry> = Vec::new();
    {
        let _dedupe_span = config
            .control
            .profile_span("capture.plan_deduplicated_chunks");
        {
            let _existing_refs_span = config
                .control
                .profile_span("capture.plan_deduplicated_chunks.account_existing_chunks");
            for file_manifest in snapshot_entries
                .iter()
                .filter_map(manifest::SnapshotEntry::as_file)
            {
                config.control.checkpoint()?;
                for chunk_ref in &file_manifest.chunks {
                    config.control.checkpoint()?;
                    let chunk_hash = ChunkHash::parse_hex(&chunk_ref.chunk_hash)?;
                    if let Some(existing) = existing_chunk_locations_by_hash.get(&chunk_hash)
                        && compressed_estimate_seen.insert(chunk_hash)
                    {
                        total_compressed += existing.compressed_size;
                    }
                }
            }
        }

        {
            let _new_chunks_span = config
                .control
                .profile_span("capture.plan_deduplicated_chunks.select_new_chunks");
            let dedupe_lookup_started_at = ProfileInstant::start(config.control.profiler());
            for (hash, payload) in unique_chunks {
                let raw_size = payload.raw_size();
                config.control.checkpoint()?;

                timings.dedupe_lookup_bytes += raw_size;

                if let Some(existing) = existing_chunk_locations_by_hash.get(&hash) {
                    if compressed_estimate_seen.insert(hash) {
                        total_compressed += existing.compressed_size;
                    }
                } else {
                    chunks_to_compress.push((hash, payload));
                }
            }
            timings.dedupe_lookup += dedupe_lookup_started_at.elapsed();
        }
        config
            .control
            .profile_event_with_args("capture.plan_deduplicated_chunks.complete", || {
                BTreeMap::from([
                    (
                        "unique_chunk_count".to_string(),
                        serde_json::json!(
                            compressed_estimate_seen.len() + chunks_to_compress.len()
                        ),
                    ),
                    (
                        "new_chunk_count".to_string(),
                        serde_json::json!(chunks_to_compress.len()),
                    ),
                ])
            });
    }
    timings.dedupe_plan_wall = dedupe_plan_start.elapsed();
    Ok(DeduplicatedChunks {
        total_compressed,
        chunks_to_compress,
    })
}

fn build_capture_artifacts(
    config: &BuildConfig<'_>,
    build_pool: &ThreadPool,
    chunks_to_compress: Vec<ChunkPayloadEntry>,
    chunk_placements: &HashMap<ChunkHash, ChunkPlacementKey>,
    memory_budget_bytes: u64,
    timings: &mut BuildTimings,
    total_compressed: &mut u64,
) -> Result<(Vec<BuiltBundle>, Vec<BuiltLooseChunk>)> {
    let compression_profile = config.compression_profile.clone();
    let artifact_staging_root = config
        .artifact_staging_root
        .clone()
        .unwrap_or_else(std::env::temp_dir);
    let empty_existing_bundle_ids = HashSet::new();
    let existing_bundle_ids = config
        .existing_bundle_ids
        .unwrap_or(&empty_existing_bundle_ids);

    let artifact_encode_start = ProfileInstant::start(config.control.profiler());
    let (bundles, loose_chunks) = {
        let _artifact_encode_span = config.control.profile_span("capture.build_artifacts");
        let mut bundles = Vec::new();
        let mut loose_chunks = Vec::new();
        let bundle_size_policy = BundleSizingPolicy::new(
            config.bundle_profile.bundle_max_size_bytes(),
            config.bundle_profile.bundle_preferred_size_bytes(),
        );

        match config.storage_mode {
            ArtifactStorageMode::Bundled => {
                if compression_profile.compression_unit
                    != crate::artifact::profiles::CompressionUnit::Bundle
                {
                    return Err(crate::Error::manifest(
                        "gutch only supports bundle-compressed storage".to_string(),
                    ));
                }

                let planned_bundle_groups = {
                    let _plan_span = config.control.profile_span("capture.plan_artifact_groups");
                    plan_bundle_compressed_groups(
                        chunks_to_compress,
                        chunk_placements,
                        &compression_profile,
                        &config.bundle_profile,
                        bundle_size_policy,
                        build_pool,
                        &config.control,
                    )?
                };
                timings.bundle_compressed_sort += planned_bundle_groups.sort_duration;
                timings.bundle_compressed_sort_bytes += planned_bundle_groups.sort_bytes;
                timings.bundle_compressed_ratio_probe += planned_bundle_groups.probe_duration;
                timings.bundle_compressed_ratio_probe_bytes +=
                    planned_bundle_groups.probe_raw_bytes;
                let built_bundle_groups = {
                    let _compression_span =
                        config.control.profile_span("capture.compress_artifacts");
                    build_bundled_chunks(
                        planned_bundle_groups,
                        &compression_profile,
                        &artifact_staging_root,
                        build_pool,
                        &config.control,
                        memory_budget_bytes,
                    )?
                };

                for built_bundle in built_bundle_groups {
                    config.control.checkpoint()?;
                    timings.artifact_payload_load += built_bundle.payload_load;
                    timings.artifact_payload_load_bytes += built_bundle.payload_load_bytes;
                    if !existing_bundle_ids.contains(&built_bundle.bundle_id) {
                        record_bundle_compressed_bundle_metrics(
                            timings,
                            total_compressed,
                            built_bundle.bundle_size,
                            built_bundle.bundle_metrics,
                        );
                        bundles.push(BuiltBundle {
                            bundle_id: built_bundle.bundle_id,
                            staged_file: built_bundle.staged_file,
                            size: built_bundle.bundle_size,
                            entries: built_bundle.bundle_entries,
                        });
                    } else {
                        drop(built_bundle.staged_file);
                    }
                }
            }
            ArtifactStorageMode::LooseChunks => {
                let built_chunks = {
                    let _compression_span =
                        config.control.profile_span("capture.compress_artifacts");
                    build_loose_chunks(
                        chunks_to_compress,
                        &artifact_staging_root,
                        &compression_profile,
                        build_pool,
                        &config.control,
                        memory_budget_bytes,
                    )?
                };

                for built_chunk in built_chunks {
                    config.control.checkpoint()?;
                    timings.artifact_payload_load += built_chunk.payload_load;
                    timings.artifact_payload_load_bytes += built_chunk.payload_load_bytes;
                    timings.compression_output_bytes += built_chunk.chunk.compressed_size;
                    *total_compressed += built_chunk.chunk.compressed_size;
                    loose_chunks.push(built_chunk.chunk);
                }
            }
        }
        (bundles, loose_chunks)
    };

    timings.artifact_encode_wall = artifact_encode_start.elapsed();
    Ok((bundles, loose_chunks))
}

fn parse_existing_chunk_locations(
    existing_chunk_locations: &HashMap<String, ExistingChunkLocation>,
) -> Result<HashMap<ChunkHash, ExistingChunkLocation>> {
    existing_chunk_locations
        .iter()
        .map(|(chunk_hash, location)| Ok((ChunkHash::parse_hex(chunk_hash)?, location.clone())))
        .collect()
}
