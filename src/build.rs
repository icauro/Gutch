use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use rayon::prelude::*;

use crate::Result;
use crate::build_scan::collect_scanned_chunks;
use crate::build_types::{
    BuildConfig, BuildResult, BuildTimings, BuiltBundle, ExistingChunkLocation, RepoInitResult,
    RepoInitTargetInput,
};
use crate::bundle::{self, BundleBuilder};
use crate::compress::{compress_chunk, compress_with_level};
use crate::manifest::{
    self, COMPRESSION_UNIT_BUNDLE, FileManifest, TargetTagSet, target::TargetManifest,
};

const MAX_BUNDLE_SIZE: u64 = 64 * 1024 * 1024;
const MIN_PREFERRED_BUNDLE_SIZE: u64 = 8 * 1024 * 1024;

struct ScannedTarget {
    target_id: String,
    target_tags: TargetTagSet,
    files: Vec<FileManifest>,
    unique_chunks: HashMap<String, Vec<u8>>,
    total_logical_size: u64,
}

#[derive(Clone)]
struct CompressedChunk {
    hash: String,
    raw_size: u64,
    compressed: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
struct BundleSizingPolicy {
    max_size: u64,
    min_preferred_size: u64,
}

impl BundleSizingPolicy {
    fn new(max_size: u64, min_preferred_size: u64) -> Self {
        Self {
            max_size,
            min_preferred_size,
        }
    }

    fn target_size_for(&self, compressed_bytes: u64) -> u64 {
        if compressed_bytes == 0 {
            return self.max_size;
        }

        if compressed_bytes <= self.max_size {
            return compressed_bytes
                .max(self.min_preferred_size)
                .min(self.max_size);
        }

        let bundle_count = compressed_bytes.div_ceil(self.max_size);
        compressed_bytes
            .div_ceil(bundle_count)
            .max(self.min_preferred_size)
            .min(self.max_size)
    }
}

#[cfg(test)]
#[path = "build_tests.rs"]
mod tests;

impl Default for BundleSizingPolicy {
    fn default() -> Self {
        Self::new(MAX_BUNDLE_SIZE, MIN_PREFERRED_BUNDLE_SIZE)
    }
}

fn estimate_bundle_compression_ratio_from_probe(
    chunks: &[(String, Vec<u8>)],
    compression_profile: &manifest::CompressionProfile,
    probe_raw_target_size: u64,
) -> Result<f64> {
    if chunks.is_empty() {
        return Ok(1.0);
    }

    let mut raw_probe_size: u64 = 0;
    let mut probe_payload = Vec::new();
    for (_, data) in chunks {
        probe_payload.extend_from_slice(data);
        raw_probe_size += data.len() as u64;
        if raw_probe_size >= probe_raw_target_size {
            break;
        }
    }

    if raw_probe_size == 0 {
        return Ok(1.0);
    }

    let compressed_probe = compress_with_level(&probe_payload, compression_profile.zstd_level)?;
    Ok((compressed_probe.len() as f64 / raw_probe_size as f64).max(0.01))
}

fn raw_target_size_for_bundle_compression(
    compressed_target_size: u64,
    compression_ratio: f64,
    bundle_policy: BundleSizingPolicy,
) -> u64 {
    let normalized_ratio = compression_ratio.max(0.01);
    ((compressed_target_size as f64 / normalized_ratio).ceil() as u64)
        .max(bundle_policy.min_preferred_size)
        .min(u64::MAX)
}

fn update_bundle_compression_ratio(
    previous_ratio: f64,
    raw_size: u64,
    compressed_size: u64,
) -> f64 {
    if raw_size == 0 {
        return previous_ratio;
    }

    let actual_ratio = (compressed_size as f64 / raw_size as f64).max(0.01);
    previous_ratio * 0.7 + actual_ratio * 0.3
}

pub fn build_from_directory(
    dir: &Path,
    content_repo_id: &str,
    target_id: &str,
    target_tags: &TargetTagSet,
) -> Result<BuildResult> {
    let (result, _) = build_from_directory_with_timings(dir, content_repo_id, target_id, target_tags)?;
    Ok(result)
}

pub fn build_from_directory_with_timings(
    dir: &Path,
    content_repo_id: &str,
    target_id: &str,
    target_tags: &TargetTagSet,
) -> Result<(BuildResult, BuildTimings)> {
    let config = BuildConfig::with_ids(content_repo_id, target_id, target_tags);
    build_with_config(dir, &config)
}

pub fn build_from_directory_with_timings_against_existing_bundles(
    dir: &Path,
    content_repo_id: &str,
    target_id: &str,
    target_tags: &TargetTagSet,
    existing_bundle_ids: &HashSet<String>,
) -> Result<(BuildResult, BuildTimings)> {
    let mut config = BuildConfig::with_ids(content_repo_id, target_id, target_tags);
    config.existing_bundle_ids = existing_bundle_ids.clone();
    build_with_config(dir, &config)
}

pub fn build_from_directory_with_timings_against_existing_store(
    dir: &Path,
    content_repo_id: &str,
    target_id: &str,
    target_tags: &TargetTagSet,
    existing_chunk_locations: &HashMap<String, ExistingChunkLocation>,
    existing_bundle_ids: &HashSet<String>,
) -> Result<(BuildResult, BuildTimings)> {
    let mut config = BuildConfig::with_ids(content_repo_id, target_id, target_tags);
    config.existing_chunk_locations = existing_chunk_locations.clone();
    config.existing_bundle_ids = existing_bundle_ids.clone();
    build_with_config(dir, &config)
}

pub fn build_from_directory_with_timings_against_existing_store_and_manifests(
    dir: &Path,
    content_repo_id: &str,
    target_id: &str,
    target_tags: &TargetTagSet,
    existing_chunk_locations: &HashMap<String, ExistingChunkLocation>,
    existing_bundle_ids: &HashSet<String>,
    previous_file_manifests: &HashMap<String, FileManifest>,
) -> Result<(BuildResult, BuildTimings)> {
    let mut config = BuildConfig::with_ids(content_repo_id, target_id, target_tags);
    config.existing_chunk_locations = existing_chunk_locations.clone();
    config.existing_bundle_ids = existing_bundle_ids.clone();
    config.previous_file_manifests = previous_file_manifests.clone();
    build_with_config(dir, &config)
}

pub fn build_from_directory_with_timings_against_existing_store_and_manifests_and_profile(
    dir: &Path,
    content_repo_id: &str,
    target_id: &str,
    target_tags: &TargetTagSet,
    existing_chunk_locations: &HashMap<String, ExistingChunkLocation>,
    existing_bundle_ids: &HashSet<String>,
    previous_file_manifests: &HashMap<String, FileManifest>,
    chunk_profile: &manifest::ChunkSizingProfile,
    bundle_profile: &manifest::BundleSizingProfile,
) -> Result<(BuildResult, BuildTimings)> {
    let mut config = BuildConfig::with_ids(content_repo_id, target_id, target_tags);
    config.existing_chunk_locations = existing_chunk_locations.clone();
    config.existing_bundle_ids = existing_bundle_ids.clone();
    config.previous_file_manifests = previous_file_manifests.clone();
    config.chunk_profile = chunk_profile.clone();
    config.bundle_profile = bundle_profile.clone();
    build_with_config(dir, &config)
}

pub fn build_from_directory_with_timings_against_existing_store_and_manifests_and_profiles(
    dir: &Path,
    content_repo_id: &str,
    target_id: &str,
    target_tags: &TargetTagSet,
    existing_chunk_locations: &HashMap<String, ExistingChunkLocation>,
    existing_bundle_ids: &HashSet<String>,
    previous_file_manifests: &HashMap<String, FileManifest>,
    chunk_profile: &manifest::ChunkSizingProfile,
    bundle_profile: &manifest::BundleSizingProfile,
    compression_profile: &manifest::CompressionProfile,
) -> Result<(BuildResult, BuildTimings)> {
    let mut config = BuildConfig::with_ids(content_repo_id, target_id, target_tags);
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

    let (mut file_manifests, unique_chunks, total_logical_size) = collect_scanned_chunks(
        dir,
        &config.previous_file_manifests,
        &config.chunk_profile,
        &config.existing_chunk_locations,
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
    for (hash, data) in unique_chunks {
        let raw_size = data.len() as u64;
        timings.dedupe_lookup_bytes += raw_size;

        if let Some(existing) = config.existing_chunk_locations.get(&hash) {
            if compressed_estimate_seen.insert(hash.clone()) {
                total_compressed += existing.compressed_size;
            }
        } else {
            chunks_to_compress.push((hash, data));
        }
    }
    timings.dedupe_lookup += t.elapsed();

    for (_, data) in &chunks_to_compress {
        timings.compression_input_bytes += data.len() as u64;
    }

    let t = Instant::now();
    let mut bundles = Vec::new();
    let mut builder = BundleBuilder::new(compression_profile.clone());
    let bundle_target_size_policy = BundleSizingPolicy::new(
        config.bundle_profile.bundle_max_size_bytes(),
        config.bundle_profile.bundle_preferred_size_bytes(),
    );

    if compression_profile.compression_unit == COMPRESSION_UNIT_BUNDLE {
        let mut sorted_chunks = chunks_to_compress;
        sorted_chunks.sort_by(|a, b| a.0.cmp(&b.0));
        let new_raw_bytes: u64 = sorted_chunks
            .iter()
            .map(|(_, data)| data.len() as u64)
            .sum();
        let mut bundle_compression_ratio = estimate_bundle_compression_ratio_from_probe(
            &sorted_chunks,
            &compression_profile,
            config.bundle_profile.bundle_preferred_size_bytes(),
        )?;
        let estimated_total_compressed_bytes =
            ((new_raw_bytes as f64) * bundle_compression_ratio).ceil() as u64;
        let bundle_target_compressed_size =
            bundle_target_size_policy.target_size_for(estimated_total_compressed_bytes);
        let mut current_bundle_raw_size: u64 = 0;
        let mut current_bundle_raw_target_size = raw_target_size_for_bundle_compression(
            bundle_target_compressed_size,
            bundle_compression_ratio,
            bundle_target_size_policy,
        );

        for (hash, data) in sorted_chunks {
            let raw_size = data.len() as u64;
            if !builder.is_empty()
                && current_bundle_raw_size + raw_size > current_bundle_raw_target_size
            {
                let bundle_id = builder.compute_bundle_id();
                required_bundle_set.push(bundle_id.clone());
                if !config.existing_bundle_ids.contains(&bundle_id) {
                    let bundle_data = builder.build()?;
                    timings.compression_output_bytes += bundle_data.len() as u64;
                    total_compressed += bundle_data.len() as u64;
                    bundle_compression_ratio = update_bundle_compression_ratio(
                        bundle_compression_ratio,
                        current_bundle_raw_size,
                        bundle_data.len() as u64,
                    );
                    bundles.push(BuiltBundle {
                        bundle_id,
                        data: bundle_data,
                    });
                }
                builder = BundleBuilder::new(compression_profile.clone());
                current_bundle_raw_size = 0;
                current_bundle_raw_target_size = raw_target_size_for_bundle_compression(
                    bundle_target_compressed_size,
                    bundle_compression_ratio,
                    bundle_target_size_policy,
                );
            }

            builder.add_chunk(hash, data);
            current_bundle_raw_size += raw_size;
        }
    } else {
        let t = Instant::now();
        let mut current_bundle_size: u64 = 0;
        let compressed_chunks: Vec<CompressedChunk> = chunks_to_compress
            .into_par_iter()
            .map(|(hash, data)| -> Result<CompressedChunk> {
                let raw_size = data.len() as u64;
                let compressed = compress_chunk(&data, &compression_profile)?;
                Ok(CompressedChunk {
                    hash,
                    raw_size,
                    compressed,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        timings.compression += t.elapsed();

        for chunk in &compressed_chunks {
            timings.compression_output_bytes += chunk.compressed.len() as u64;
            if compressed_estimate_seen.insert(chunk.hash.clone()) {
                total_compressed += chunk.compressed.len() as u64;
            }
        }

        let mut sorted_chunks = compressed_chunks;
        sorted_chunks.sort_by(|a, b| a.hash.cmp(&b.hash));
        let new_compressed_bytes: u64 = sorted_chunks
            .iter()
            .map(|c| c.compressed.len() as u64)
            .sum();
        let bundle_target_size = bundle_target_size_policy.target_size_for(new_compressed_bytes);

        for chunk in sorted_chunks {
            let compressed_size = chunk.compressed.len() as u64;
            if !builder.is_empty() && current_bundle_size + compressed_size > bundle_target_size {
                let bundle_id = builder.compute_bundle_id();
                required_bundle_set.push(bundle_id.clone());
                if !config.existing_bundle_ids.contains(&bundle_id) {
                    let bundle_data = builder.build()?;
                    bundles.push(BuiltBundle {
                        bundle_id,
                        data: bundle_data,
                    });
                }
                builder = BundleBuilder::new(compression_profile.clone());
                current_bundle_size = 0;
            }

            builder.add_precompressed_chunk(chunk.hash, chunk.raw_size, chunk.compressed);
            current_bundle_size += compressed_size;
        }
    }

    if !builder.is_empty() {
        let bundle_id = builder.compute_bundle_id();
        required_bundle_set.push(bundle_id.clone());
        if !config.existing_bundle_ids.contains(&bundle_id) {
            let bundle_data = builder.build()?;
            if compression_profile.compression_unit == COMPRESSION_UNIT_BUNDLE {
                timings.compression_output_bytes += bundle_data.len() as u64;
                total_compressed += bundle_data.len() as u64;
            }
            bundles.push(BuiltBundle {
                bundle_id,
                data: bundle_data,
            });
        }
    }

    if compression_profile.compression_unit == COMPRESSION_UNIT_BUNDLE {
        timings.compression = t.elapsed();
    }
    timings.total_compressed_bytes = total_compressed;
    timings.bundle_build += t.elapsed();
    timings.bundle_build_bytes = bundles.iter().map(|b| b.data.len() as u64).sum();

    let t = Instant::now();
    file_manifests.sort_by(|a, b| a.path.cmp(&b.path));
    let mut vm = TargetManifest::new(
        config.content_repo_id.clone(),
        config.target_id.clone(),
        config.target_tags.clone(),
    );
    vm.total_logical_size = total_logical_size;
    vm.total_compressed_size_estimate = total_compressed;
    vm.files = file_manifests;
    timings.target_manifest_finalize += t.elapsed();
    timings.total = total_start.elapsed();

    Ok((
        BuildResult {
            target_manifest: vm,
            bundles,
        },
        timings,
    ))
}

pub fn build_repo_init_batch_with_timings(
    content_repo_id: &str,
    targets: &[RepoInitTargetInput],
    chunk_profile: &manifest::ChunkSizingProfile,
    bundle_profile: &manifest::BundleSizingProfile,
    compression_profile: &manifest::CompressionProfile,
) -> Result<(RepoInitResult, BuildTimings)> {
    let total_start = Instant::now();
    let mut timings = BuildTimings::default();
    let previous_file_manifests: HashMap<String, FileManifest> = HashMap::new();
    let existing_chunk_locations: HashMap<String, ExistingChunkLocation> = HashMap::new();
    let mut scanned_targets = Vec::with_capacity(targets.len());

    for target in targets {
        let (files, unique_chunks, total_logical_size) = collect_scanned_chunks(
            &target.dir,
            &previous_file_manifests,
            chunk_profile,
            &existing_chunk_locations,
            &mut timings,
        )?;

        scanned_targets.push(ScannedTarget {
            target_id: target.target_id.clone(),
            target_tags: target.target_tags.clone(),
            files,
            unique_chunks,
            total_logical_size,
        });
    }

    let mut repo_unique_chunks: HashMap<String, Vec<u8>> = HashMap::new();
    for target in &scanned_targets {
        for (hash, data) in &target.unique_chunks {
            repo_unique_chunks
                .entry(hash.clone())
                .or_insert_with(|| data.clone());
        }
    }

    let mut chunks_to_compress: Vec<(String, Vec<u8>)> = repo_unique_chunks.into_iter().collect();
    for (_, data) in &chunks_to_compress {
        timings.dedupe_lookup_bytes += data.len() as u64;
        timings.compression_input_bytes += data.len() as u64;
    }

    let t = Instant::now();
    let bundle_target_size_policy = BundleSizingPolicy::new(
        bundle_profile.bundle_max_size_bytes(),
        bundle_profile.bundle_preferred_size_bytes(),
    );
    let mut bundles = Vec::new();
    let mut builder = BundleBuilder::new(compression_profile.clone());

    if compression_profile.compression_unit == COMPRESSION_UNIT_BUNDLE {
        chunks_to_compress.sort_by(|a, b| a.0.cmp(&b.0));
        let new_raw_bytes: u64 = chunks_to_compress
            .iter()
            .map(|(_, data)| data.len() as u64)
            .sum();
        let mut bundle_compression_ratio = estimate_bundle_compression_ratio_from_probe(
            &chunks_to_compress,
            compression_profile,
            bundle_profile.bundle_preferred_size_bytes(),
        )?;
        let estimated_total_compressed_bytes =
            ((new_raw_bytes as f64) * bundle_compression_ratio).ceil() as u64;
        let bundle_target_compressed_size =
            bundle_target_size_policy.target_size_for(estimated_total_compressed_bytes);
        let mut current_bundle_raw_size: u64 = 0;
        let mut current_bundle_raw_target_size = raw_target_size_for_bundle_compression(
            bundle_target_compressed_size,
            bundle_compression_ratio,
            bundle_target_size_policy,
        );

        for (hash, data) in chunks_to_compress {
            let raw_size = data.len() as u64;
            if !builder.is_empty()
                && current_bundle_raw_size + raw_size > current_bundle_raw_target_size
            {
                let bundle_id = builder.compute_bundle_id();
                let bundle_data = builder.build()?;
                timings.compression_output_bytes += bundle_data.len() as u64;
                timings.total_compressed_bytes += bundle_data.len() as u64;
                bundle_compression_ratio = update_bundle_compression_ratio(
                    bundle_compression_ratio,
                    current_bundle_raw_size,
                    bundle_data.len() as u64,
                );
                bundles.push(BuiltBundle {
                    bundle_id,
                    data: bundle_data,
                });
                builder = BundleBuilder::new(compression_profile.clone());
                current_bundle_raw_size = 0;
                current_bundle_raw_target_size = raw_target_size_for_bundle_compression(
                    bundle_target_compressed_size,
                    bundle_compression_ratio,
                    bundle_target_size_policy,
                );
            }

            builder.add_chunk(hash, data);
            current_bundle_raw_size += raw_size;
        }
    } else {
        let compressed_chunks: Vec<CompressedChunk> = chunks_to_compress
            .into_par_iter()
            .map(|(hash, data)| -> Result<CompressedChunk> {
                let raw_size = data.len() as u64;
                let compressed = compress_chunk(&data, compression_profile)?;
                Ok(CompressedChunk {
                    hash,
                    raw_size,
                    compressed,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        for chunk in &compressed_chunks {
            timings.compression_output_bytes += chunk.compressed.len() as u64;
        }

        let mut sorted_chunks = compressed_chunks;
        sorted_chunks.sort_by(|a, b| a.hash.cmp(&b.hash));
        let new_compressed_bytes: u64 = sorted_chunks
            .iter()
            .map(|c| c.compressed.len() as u64)
            .sum();
        let bundle_target_size = bundle_target_size_policy.target_size_for(new_compressed_bytes);
        let mut current_bundle_size: u64 = 0;

        for chunk in sorted_chunks {
            let compressed_size = chunk.compressed.len() as u64;
            if !builder.is_empty() && current_bundle_size + compressed_size > bundle_target_size {
                let bundle_id = builder.compute_bundle_id();
                let bundle_data = builder.build()?;
                timings.total_compressed_bytes += bundle_data.len() as u64;
                bundles.push(BuiltBundle {
                    bundle_id,
                    data: bundle_data,
                });
                builder = BundleBuilder::new(compression_profile.clone());
                current_bundle_size = 0;
            }

            builder.add_precompressed_chunk(chunk.hash, chunk.raw_size, chunk.compressed);
            current_bundle_size += compressed_size;
        }
    }

    if !builder.is_empty() {
        let bundle_id = builder.compute_bundle_id();
        let bundle_data = builder.build()?;
        if compression_profile.compression_unit == COMPRESSION_UNIT_BUNDLE {
            timings.compression_output_bytes += bundle_data.len() as u64;
        }
        timings.total_compressed_bytes += bundle_data.len() as u64;
        bundles.push(BuiltBundle {
            bundle_id,
            data: bundle_data,
        });
    }

    timings.compression += t.elapsed();
    timings.bundle_build += t.elapsed();
    timings.bundle_build_bytes = bundles.iter().map(|b| b.data.len() as u64).sum();

    let mut chunk_locations = HashMap::new();
    for bundle in &bundles {
        let reader = bundle::read_bundle(&bundle.data)?;
        for entry in reader.entries() {
            chunk_locations.insert(
                entry.chunk_hash.clone(),
                ExistingChunkLocation {
                    bundle_id: bundle.bundle_id.clone(),
                    raw_size: entry.raw_size,
                    compressed_size: entry.compressed_size,
                },
            );
        }
    }

    let t = Instant::now();
    let mut target_manifests = Vec::with_capacity(scanned_targets.len());
    for mut target in scanned_targets {
        target.files.sort_by(|a, b| a.path.cmp(&b.path));
        let mut manifest = TargetManifest::new(
            content_repo_id.to_string(),
            target.target_id,
            target.target_tags,
        );
        manifest.total_logical_size = target.total_logical_size;
        manifest.files = target.files;

        let required_chunk_hashes: Vec<String> = manifest
            .files
            .iter()
            .flat_map(|file| file.chunks.iter().map(|chunk| chunk.chunk_hash.clone()))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        let mut total_compressed_size_estimate = 0u64;
        for chunk_hash in &required_chunk_hashes {
            if let Some(location) = chunk_locations.get(chunk_hash) {
                total_compressed_size_estimate += location.compressed_size;
            }
        }
        manifest.total_compressed_size_estimate = total_compressed_size_estimate;
        target_manifests.push(manifest);
    }
    timings.target_manifest_finalize += t.elapsed();

    timings.total = total_start.elapsed();
    Ok((
        RepoInitResult {
            target_manifests,
            bundles,
        },
        timings,
    ))
}
