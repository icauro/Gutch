mod restore;
mod source;
mod types;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::Result;
use crate::bundle;
use crate::manifest::{CompressionProfile, target::TargetManifest};
use crate::repo::{GutchRepoPaths, required_bundle_ids, required_chunk_hashes};
use crate::runtime::RuntimeResourceHints;

use restore::{
    prepare_reconstruct, read_loose_repo_chunks, rebuild_files_from_chunks,
    reconstruct_from_bundle_paths_and_local_chunks,
};
use source::read_reusable_source_chunks;
pub use types::{ReconstructOptions, ReconstructTimings, VerifiedSourceInstall};

#[cfg(test)]
use crate::runtime::RuntimePlan;
#[cfg(test)]
use restore::{
    should_stage_bundle_payloads_in_memory, should_use_direct_bundle_to_file_restore,
    should_use_direct_bundle_to_file_restore_experimental,
};
#[cfg(test)]
use std::collections::HashMap as TestHashMap;
#[cfg(test)]
use std::path::PathBuf as TestPathBuf;
#[cfg(test)]
use types::BundleScanInfo;

pub fn reconstruct_from_target_manifest(
    target_manifest: &TargetManifest,
    bundles: &[Vec<u8>],
    output_dir: &Path,
) -> Result<()> {
    let (_, _) =
        reconstruct_from_target_manifest_with_timings(target_manifest, bundles, output_dir)?;
    Ok(())
}

pub fn reconstruct_from_target_manifest_with_timings(
    target_manifest: &TargetManifest,
    bundles: &[Vec<u8>],
    output_dir: &Path,
) -> Result<((), ReconstructTimings)> {
    let total_start = Instant::now();
    let mut timings = ReconstructTimings::default();
    crate::helpers::create_dir_all(output_dir)?;

    let needed_chunks = required_chunk_hashes(target_manifest);
    let mut chunk_map: HashMap<String, Vec<u8>> = HashMap::with_capacity(needed_chunks.len());

    for bundle_data in bundles {
        let t = Instant::now();
        let mut reader = bundle::read_bundle(bundle_data)?;
        let matching_indices: Vec<usize> = reader
            .entries()
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                if needed_chunks.contains(entry.chunk_hash.as_str())
                    && !chunk_map.contains_key(&entry.chunk_hash)
                {
                    Some(index)
                } else {
                    None
                }
            })
            .collect();
        if matching_indices.is_empty() {
            continue;
        }

        timings.bundle_read_compressed_bytes += bundle_data.len() as u64;
        for index in matching_indices {
            let hash = reader.entries()[index].chunk_hash.clone();
            let data = reader.read_chunk(index)?;
            timings.bundle_read_decompressed_bytes += data.len() as u64;
            chunk_map.insert(hash, data);
        }
        timings.bundle_read += t.elapsed();

        if chunk_map.len() == needed_chunks.len() {
            break;
        }
    }

    rebuild_files_from_chunks(target_manifest, &chunk_map, output_dir, &mut timings)?;
    timings.total = total_start.elapsed();
    Ok(((), timings))
}

pub fn reconstruct_from_bundle_paths_with_timings(
    target_manifest: &TargetManifest,
    bundle_paths: &[PathBuf],
    output_dir: &Path,
) -> Result<((), ReconstructTimings)> {
    reconstruct_from_bundle_paths_with_timings_and_hints(
        target_manifest,
        bundle_paths,
        output_dir,
        &RuntimeResourceHints::default(),
    )
}

pub fn reconstruct_from_bundle_paths_with_timings_and_hints(
    target_manifest: &TargetManifest,
    bundle_paths: &[PathBuf],
    output_dir: &Path,
    runtime_hints: &RuntimeResourceHints,
) -> Result<((), ReconstructTimings)> {
    reconstruct_from_bundle_paths_and_verified_source_with_timings_and_hints(
        target_manifest,
        bundle_paths,
        None,
        output_dir,
        runtime_hints,
    )
}

pub fn reconstruct_from_bundle_paths_and_verified_source_with_timings(
    target_manifest: &TargetManifest,
    bundle_paths: &[PathBuf],
    verified_source: Option<VerifiedSourceInstall<'_>>,
    output_dir: &Path,
) -> Result<((), ReconstructTimings)> {
    reconstruct_from_bundle_paths_and_verified_source_with_timings_and_hints(
        target_manifest,
        bundle_paths,
        verified_source,
        output_dir,
        &RuntimeResourceHints::default(),
    )
}

pub fn reconstruct_from_bundle_paths_and_verified_source_with_timings_and_hints(
    target_manifest: &TargetManifest,
    bundle_paths: &[PathBuf],
    verified_source: Option<VerifiedSourceInstall<'_>>,
    output_dir: &Path,
    runtime_hints: &RuntimeResourceHints,
) -> Result<((), ReconstructTimings)> {
    reconstruct_from_bundle_paths_and_verified_source_with_timings_and_hints_with_options(
        target_manifest,
        bundle_paths,
        verified_source,
        output_dir,
        runtime_hints,
        ReconstructOptions::default(),
    )
}

pub fn reconstruct_from_bundle_paths_and_verified_source_with_timings_and_hints_with_options(
    target_manifest: &TargetManifest,
    bundle_paths: &[PathBuf],
    verified_source: Option<VerifiedSourceInstall<'_>>,
    output_dir: &Path,
    runtime_hints: &RuntimeResourceHints,
    options: ReconstructOptions,
) -> Result<((), ReconstructTimings)> {
    let (needed_chunks, total_start, runtime_plan, timings) =
        prepare_reconstruct(target_manifest, output_dir, runtime_hints)?;

    let local_chunks = match verified_source {
        Some(source) => read_reusable_source_chunks(source, &needed_chunks)?,
        None => HashMap::new(),
    };
    reconstruct_from_bundle_paths_and_local_chunks(
        target_manifest,
        bundle_paths,
        &needed_chunks,
        local_chunks,
        output_dir,
        runtime_plan,
        total_start,
        timings,
        options,
    )
}

pub fn reconstruct_from_repo_paths_with_timings_and_hints(
    target_manifest: &TargetManifest,
    repo_paths: &GutchRepoPaths,
    compression_profile: &CompressionProfile,
    output_dir: &Path,
    runtime_hints: &RuntimeResourceHints,
) -> Result<((), ReconstructTimings)> {
    reconstruct_from_repo_paths_with_timings_and_hints_with_options(
        target_manifest,
        repo_paths,
        compression_profile,
        output_dir,
        runtime_hints,
        ReconstructOptions::default(),
    )
}

pub fn reconstruct_from_repo_paths_with_timings_and_hints_with_options(
    target_manifest: &TargetManifest,
    repo_paths: &GutchRepoPaths,
    compression_profile: &CompressionProfile,
    output_dir: &Path,
    runtime_hints: &RuntimeResourceHints,
    options: ReconstructOptions,
) -> Result<((), ReconstructTimings)> {
    let (needed_chunks, total_start, runtime_plan, mut timings) =
        prepare_reconstruct(target_manifest, output_dir, runtime_hints)?;

    let chunk_index = crate::repo::load_chunk_index(repo_paths)?;
    let local_chunks = read_loose_repo_chunks(
        repo_paths,
        &chunk_index,
        compression_profile,
        &needed_chunks,
        options,
        runtime_plan.worker_count,
        &mut timings,
    )?;
    let required_bundle_ids = required_bundle_ids(target_manifest, &chunk_index);
    let bundle_paths = required_bundle_ids
        .iter()
        .map(|bundle_id| crate::repo::bundle_path(repo_paths, bundle_id))
        .collect::<Vec<_>>();

    reconstruct_from_bundle_paths_and_local_chunks(
        target_manifest,
        &bundle_paths,
        &needed_chunks,
        local_chunks,
        output_dir,
        runtime_plan,
        total_start,
        timings,
        options,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{ChunkStorageMode, EffectiveMemoryMode, MemoryMode};

    fn runtime_plan(
        effective_memory_mode: EffectiveMemoryMode,
        memory_budget_mib: u64,
    ) -> RuntimePlan {
        RuntimePlan {
            requested_memory_mode: MemoryMode::Auto,
            effective_memory_mode,
            available_memory_mib: Some(memory_budget_mib),
            memory_budget_mib,
            worker_count: 4,
            chunk_storage_mode: ChunkStorageMode::Memory,
        }
    }

    #[test]
    fn high_memory_reconstruct_stages_small_bundle_payloads_in_memory() {
        let bundle_path = TestPathBuf::from("bundle-a.bundle");
        let bundle_paths = vec![bundle_path.clone()];
        let bundle_scan_info = TestHashMap::from([(
            bundle_path,
            BundleScanInfo {
                compression_unit: crate::manifest::COMPRESSION_UNIT_BUNDLE,
                raw_payload_size: 64 * 1024 * 1024,
            },
        )]);

        assert!(should_stage_bundle_payloads_in_memory(
            &bundle_paths,
            &bundle_scan_info,
            &runtime_plan(EffectiveMemoryMode::High, 1024)
        ));
    }

    #[test]
    fn balanced_reconstruct_keeps_temp_staging() {
        let bundle_path = TestPathBuf::from("bundle-a.bundle");
        let bundle_paths = vec![bundle_path.clone()];
        let bundle_scan_info = TestHashMap::from([(
            bundle_path,
            BundleScanInfo {
                compression_unit: crate::manifest::COMPRESSION_UNIT_BUNDLE,
                raw_payload_size: 64 * 1024 * 1024,
            },
        )]);

        assert!(!should_stage_bundle_payloads_in_memory(
            &bundle_paths,
            &bundle_scan_info,
            &runtime_plan(EffectiveMemoryMode::Balanced, 1024)
        ));
    }

    #[test]
    fn high_memory_reconstruct_keeps_temp_staging_when_payload_exceeds_budget() {
        let bundle_path = TestPathBuf::from("bundle-a.bundle");
        let bundle_paths = vec![bundle_path.clone()];
        let bundle_scan_info = TestHashMap::from([(
            bundle_path,
            BundleScanInfo {
                compression_unit: crate::manifest::COMPRESSION_UNIT_BUNDLE,
                raw_payload_size: 700 * 1024 * 1024,
            },
        )]);

        assert!(!should_stage_bundle_payloads_in_memory(
            &bundle_paths,
            &bundle_scan_info,
            &runtime_plan(EffectiveMemoryMode::High, 1024)
        ));
    }

    #[test]
    fn high_memory_no_verify_restore_keeps_direct_bundle_to_file_disabled_by_default() {
        let bundle_scan_info = TestHashMap::from([(
            TestPathBuf::from("bundle-a.bundle"),
            BundleScanInfo {
                compression_unit: crate::manifest::COMPRESSION_UNIT_BUNDLE,
                raw_payload_size: 64 * 1024 * 1024,
            },
        )]);

        assert!(!should_use_direct_bundle_to_file_restore(
            &HashMap::new(),
            &bundle_scan_info,
            &runtime_plan(EffectiveMemoryMode::High, 1024),
            ReconstructOptions {
                verify_chunks: false,
                verify_files: false,
            },
        ));
    }

    #[test]
    fn experimental_high_memory_no_verify_restore_can_use_direct_bundle_to_file_path() {
        let bundle_scan_info = TestHashMap::from([(
            TestPathBuf::from("bundle-a.bundle"),
            BundleScanInfo {
                compression_unit: crate::manifest::COMPRESSION_UNIT_BUNDLE,
                raw_payload_size: 64 * 1024 * 1024,
            },
        )]);

        assert!(should_use_direct_bundle_to_file_restore_experimental(
            &HashMap::new(),
            &bundle_scan_info,
            &runtime_plan(EffectiveMemoryMode::High, 1024),
            ReconstructOptions {
                verify_chunks: false,
                verify_files: false,
            },
        ));
    }

    #[test]
    fn restore_with_file_verify_keeps_staged_reconstruct_path() {
        let bundle_scan_info = TestHashMap::from([(
            TestPathBuf::from("bundle-a.bundle"),
            BundleScanInfo {
                compression_unit: crate::manifest::COMPRESSION_UNIT_BUNDLE,
                raw_payload_size: 64 * 1024 * 1024,
            },
        )]);

        assert!(!should_use_direct_bundle_to_file_restore(
            &HashMap::new(),
            &bundle_scan_info,
            &runtime_plan(EffectiveMemoryMode::High, 1024),
            ReconstructOptions {
                verify_chunks: false,
                verify_files: true,
            },
        ));
    }
}
