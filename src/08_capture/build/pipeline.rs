//! Chunks, deduplicates and packs scanned files into staged bundles and loose chunks.

use crate::profiling::ProfileInstant;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use rayon::ThreadPool;
use rayon::prelude::*;

use super::progress::BuildProgressTracker;
use super::types::BuildConfig;
use super::types::BuildResult;
use super::types::BuildTimings;
use super::types::BuiltBundle;
use super::types::BuiltLooseChunk;
use super::types::ChunkPayload;
use super::types::ChunkPayloadReader;
use super::types::StagedArtifactFile;
use crate::Result;
use crate::artifact::bundle::BundleBuildMetrics;
use crate::artifact::bundle::BundleTableEntry;
use crate::artifact::bundle::encoded_metadata_size;
use crate::artifact::bundle::write_bundle_to_staging;
use crate::artifact::compression as compress;
use crate::artifact::compression::ENCODE_WORKING_MEMORY_BYTES;
use crate::artifact::format::MAX_RAW_BUNDLE_SIZE;
use crate::artifact::format::size_u32;
use crate::artifact::hash::ChunkHash;
use crate::artifact::loose::write_loose_chunk;
use crate::artifact::profiles::ArtifactStorageMode;
use crate::artifact::profiles::BundleSizingProfile;
use crate::artifact::profiles::CompressionKind;
use crate::artifact::profiles::CompressionProfile;
use crate::artifact::profiles::CompressionUnit;
use crate::artifact::profiles::validate_packaging_sizes;
use crate::capture::scan::ChunkPlacementKey;
use crate::capture::scan::ScanDirectoryOptions;
use crate::capture::scan::ScannedChunks;
use crate::capture::scan::SourceInventory;
use crate::capture::scan::collect_scanned_chunks;
use crate::capture::scan::scan_source_inventory;
use crate::manifest;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::OperationControl;
use crate::progress::ProgressPhase;
use crate::runtime::MemoryBudget;
use crate::runtime::build_thread_pool;
use crate::runtime::plan_runtime;
use crate::snapshot::identity::compute_snapshot_id_from_entries;
use std::env::temp_dir;
use std::time::Duration;

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

    fn minimum_raw_capacity(&self) -> u64 {
        self.preferred_size.min(MAX_RAW_BUNDLE_SIZE)
    }
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
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
    compression_profile: &CompressionProfile,
    probe_target_bytes: u64,
) -> Result<(f64, u64)> {
    if chunks.is_empty() {
        return Ok((1.0, 0));
    }

    let uncompressed = compression_profile.compression == CompressionKind::None;
    let mut probe_chunk_count = 0;
    let mut raw_probe_size: u64 = 0;
    for (_, payload) in chunks {
        raw_probe_size += payload.raw_size();
        probe_chunk_count += 1;
        if raw_probe_size >= probe_target_bytes {
            break;
        }
    }
    if raw_probe_size == 0 {
        return Ok((1.0, 0));
    }

    let encoded_payload_size = if uncompressed {
        compress::validate_profile(compression_profile)?;
        // Actual packing still reads and verifies every source chunk.
        raw_probe_size
    } else {
        compress::compressed_size_from_stream(compression_profile, raw_probe_size, |writer| {
            const SOURCE_HANDLE_LIMIT: usize = 8;
            let mut readers: Vec<(Arc<PathBuf>, ChunkPayloadReader)> = Vec::new();
            for (_, payload) in &chunks[..probe_chunk_count] {
                // Preserve hash-order sampling while reusing source handles.
                let reader_index = match readers
                    .iter()
                    .position(|(source, _)| source == &payload.source)
                {
                    Some(index) => index,
                    None => {
                        if readers.len() == SOURCE_HANDLE_LIMIT {
                            // Close the old handle before opening its replacement.
                            readers.remove(0);
                        }
                        let index = readers.len();
                        readers.push((payload.source.clone(), ChunkPayloadReader::default()));
                        index
                    }
                };
                // Release each verified chunk before loading the next one.
                let bytes = readers[reader_index].1.load(payload)?;
                writer.write_all(&bytes)?;
            }
            Ok(())
        })?
    };
    let encoded_size = encoded_payload_size + encoded_metadata_size(probe_chunk_count as u64);
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
        .max(bundle_policy.minimum_raw_capacity())
        .min(MAX_RAW_BUNDLE_SIZE)
}

struct PlannedBundleGroups {
    encoded_limit: u64,
    groups: Vec<PlannedChunkGroup>,
    sort_duration: Duration,
    sort_bytes: u64,
    probe_duration: Duration,
    probe_raw_bytes: u64,
}

fn plan_bundle_compressed_groups(
    mut chunks: Vec<ChunkPayloadEntry>,
    chunk_placements: &HashMap<ChunkHash, ChunkPlacementKey>,
    compression_profile: &CompressionProfile,
    bundle_profile: &BundleSizingProfile,
    bundle_policy: BundleSizingPolicy,
    pool: &ThreadPool,
    control: &OperationControl,
) -> Result<PlannedBundleGroups> {
    if chunks.is_empty() {
        return Ok(PlannedBundleGroups {
            encoded_limit: bundle_policy.max_size,
            groups: Vec::new(),
            sort_duration: Duration::ZERO,
            sort_bytes: 0,
            probe_duration: Duration::ZERO,
            probe_raw_bytes: 0,
        });
    }

    let sort_bytes: u64 = chunks.iter().map(|(_, payload)| payload.raw_size()).sum();
    // Capacity never drops below the preferred raw size (up to the raw bundle cap).
    // Below that bound the probe cannot change grouping; encoded limits still apply.
    let one_raw_group = sort_bytes <= bundle_policy.minimum_raw_capacity();
    let hash_sort_duration = if one_raw_group {
        Duration::ZERO
    } else {
        let _sort_span = control
            .profile_span_with_args("capture.plan_artifact_groups.sort_chunks_for_probe", || {
                capture_bytes_trace_args("chunk_count", chunks.len(), sort_bytes)
            });
        let sort_start = ProfileInstant::detailed(control.profiler());
        pool.install(|| chunks.par_sort_unstable_by(|left, right| left.0.cmp(&right.0)));
        sort_start.elapsed()
    };
    let (bundle_compression_ratio, probe_raw_bytes, probe_duration) = if one_raw_group {
        (1.0, 0, Duration::ZERO)
    } else {
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
        return Err(crate::Error::capture(format!(
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
    bundle_metrics: BundleBuildMetrics,
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
    bundle_entries: Vec<BundleTableEntry>,
    bundle_metrics: BundleBuildMetrics,
    payload_load: Duration,
    payload_load_bytes: u64,
}

struct BuiltLooseChunkResult {
    chunk: BuiltLooseChunk,
    payload_load: Duration,
    payload_load_bytes: u64,
}

fn run_parallel_chunk_work<T, U, F>(
    work_items: Vec<T>,
    pool: &ThreadPool,
    control: &OperationControl,
    memory_budget: u64,
    working_bytes: impl Fn(&T) -> u64,
    work: F,
) -> Result<Vec<U>>
where
    T: Send,
    U: Send,
    F: Fn(T, &OperationControl) -> Result<U> + Send + Sync,
{
    control.checkpoint()?;
    control.profile_event_with_args("runtime.memory.budget", || {
        BTreeMap::from([("budget_bytes".into(), serde_json::json!(memory_budget))])
    });
    let memory = Arc::new(MemoryBudget::new(memory_budget));
    let mut results = (0..work_items.len()).map(|_| None).collect::<Vec<_>>();
    // After any failure, nothing new is admitted or started; the result is that
    // failure, so skipped slots stay empty.
    let failed = AtomicBool::new(false);
    // Each scoped worker exclusively borrows its result slot until the scope joins.
    // in_place_scope keeps admission on the caller, not on a Rayon worker.
    pool.in_place_scope(|scope| -> Result<()> {
        for (item, slot) in work_items.into_iter().zip(&mut results) {
            if failed.load(Ordering::Acquire) {
                break;
            }
            let permit = memory.acquire(working_bytes(&item), control)?;
            let work = &work;
            let failed = &failed;
            scope.spawn(move |_| {
                if failed.load(Ordering::Acquire) {
                    return;
                }
                let result = control.checkpoint().and_then(|_| work(item, control));
                if result.is_err() {
                    failed.store(true, Ordering::Release);
                }
                *slot = Some(result);
                drop(permit);
            });
        }
        Ok(())
    })?;
    let mut values = Vec::with_capacity(results.len());
    let mut first_error = None;
    for slot in results {
        match slot {
            Some(Ok(value)) => values.push(value),
            Some(Err(error)) => {
                first_error.get_or_insert(error);
            }
            None => {}
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(values),
    }
}

fn build_bundled_chunks(
    planned_bundle_groups: PlannedBundleGroups,
    compression_profile: &CompressionProfile,
    artifact_staging_root: &Path,
    pool: &ThreadPool,
    control: &OperationControl,
    memory_budget: u64,
) -> Result<Vec<BuiltBundleCompressedGroup>> {
    control.checkpoint()?;
    let progress = BuildProgressTracker::new(control, ProgressPhase::Capture, "Building bundles");
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
            // Loading and encoding overlap: only the largest chunk is live,
            // alongside the codec's own working-memory reservation.
            bundle
                .iter()
                .map(|(_, payload)| payload.raw_size())
                .max()
                .unwrap_or(0)
                + ENCODE_WORKING_MEMORY_BYTES
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
    compression_profile: &CompressionProfile,
    pool: &ThreadPool,
    control: &OperationControl,
    memory_budget: u64,
) -> Result<Vec<BuiltLooseChunkResult>> {
    control.checkpoint()?;
    let progress = BuildProgressTracker::new(control, ProgressPhase::Capture, "Building bundles");
    progress.start(chunks_to_compress.len());
    let compression_profile_for_work = compression_profile.clone();
    let progress_for_work = progress.clone();

    run_parallel_chunk_work(
        chunks_to_compress,
        pool,
        control,
        memory_budget,
        |(_, payload)| payload.raw_size() + ENCODE_WORKING_MEMORY_BYTES,
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
    compression_profile: &CompressionProfile,
    artifact_staging_root: &Path,
    control: &OperationControl,
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
    write_loose_chunk(&mut staged, chunk_hash, &bytes, compression_profile)?;
    staged.as_file().sync_all()?;
    let compressed_size = staged.as_file().metadata()?.len();
    size_u32(compressed_size)?;
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
    profile: &CompressionProfile,
    staging: &Path,
    control: &OperationControl,
) -> Result<Vec<BuiltBundleCompressedGroup>> {
    let built = build_bundle_compressed_group(planned.clone(), profile, staging, control)?;
    if built.bundle_size <= encoded_limit {
        return Ok(vec![built]);
    }
    drop(built);
    if planned.len() < 2 {
        return Err(crate::Error::artifact(
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
    compression_profile: &CompressionProfile,
    artifact_staging_root: &Path,
    control: &OperationControl,
) -> Result<BuiltBundleCompressedGroup> {
    control.checkpoint()?;
    let _job_span = control.profile_span_with_args("capture.bundle.build", || {
        BTreeMap::from([(
            "chunk_count".into(),
            serde_json::json!(planned_bundle.len()),
        )])
    });
    let descriptors = planned_bundle
        .iter()
        .map(|(hash, payload)| (*hash, payload.raw_size()))
        .collect::<Vec<_>>();
    let mut payload_load = Duration::ZERO;
    let mut payload_load_bytes: u64 = 0;
    let mut reader = ChunkPayloadReader::default();
    let built = write_bundle_to_staging(
        &descriptors,
        compression_profile,
        artifact_staging_root,
        control.profiler(),
        |index| {
            control.checkpoint()?;
            let _input_span = control.profile_span("capture.bundle.load_inputs");
            let load_start = ProfileInstant::detailed(control.profiler());
            let bytes = reader.load(&planned_bundle[index].1)?;
            payload_load += load_start.elapsed();
            payload_load_bytes += bytes.len() as u64;
            Ok(bytes)
        },
    )?;
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

/// Scans `dir` on a new worker pool and builds its snapshot artifacts.
pub(crate) fn build_directory(
    dir: &Path,
    config: &BuildConfig<'_>,
) -> Result<(BuildResult, BuildTimings)> {
    let worker_count = plan_runtime(&config.runtime_hints).worker_count;
    let build_pool = {
        let _pool_span = config
            .control
            .profile_span_with_args("capture.create_worker_pool", || {
                BTreeMap::from([("worker_count".to_string(), serde_json::json!(worker_count))])
            });
        build_thread_pool(worker_count, "Capture worker")?
    };
    let inventory = {
        let _inventory_span = config.control.profile_span("capture.scan_source.inventory");
        scan_source_inventory(dir, &config.control, worker_count, &build_pool)?
    };
    build_from_inventory(&inventory, config, &build_pool)
}

/// Builds snapshot artifacts from an already scanned source inventory.
pub(crate) fn build_from_inventory(
    inventory: &SourceInventory,
    config: &BuildConfig<'_>,
    build_pool: &ThreadPool,
) -> Result<(BuildResult, BuildTimings)> {
    validate_packaging_sizes(
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
    let empty_available_chunk_sizes = HashMap::new();
    let available_chunk_sizes = config
        .available_chunk_sizes
        .unwrap_or(&empty_available_chunk_sizes);

    let scan_chunk_start = ProfileInstant::start(config.control.profiler());
    let ScannedChunks {
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
                available_chunk_sizes,
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
        available_chunk_sizes,
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
            compute_snapshot_id_from_entries(&snapshot_entries),
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
    available_chunk_sizes: &HashMap<ChunkHash, u64>,
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
                    let chunk_hash = chunk_ref.chunk_hash;
                    if let Some(&compressed_size) = available_chunk_sizes.get(&chunk_hash)
                        && compressed_estimate_seen.insert(chunk_hash)
                    {
                        total_compressed += compressed_size;
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

                if let Some(&compressed_size) = available_chunk_sizes.get(&hash) {
                    if compressed_estimate_seen.insert(hash) {
                        total_compressed += compressed_size;
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
        .unwrap_or_else(temp_dir);
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
                if compression_profile.compression_unit != CompressionUnit::Bundle {
                    return Err(crate::Error::capture(
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
