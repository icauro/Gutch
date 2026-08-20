mod restore;
mod source;
mod types;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::Result;
use crate::bundle;
use crate::hash::ChunkHash;
use crate::manifest::{CompressionProfile, target::TargetManifest};
use crate::progress::GutchOperationControl;
use crate::repo::{GutchRepoPaths, required_bundle_ids, required_chunk_hashes_typed};
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
    compression_profile: &CompressionProfile,
    output_dir: &Path,
    control: &GutchOperationControl,
) -> Result<()> {
    let (_, _) = reconstruct_from_target_manifest_with_timings(
        target_manifest,
        bundles,
        compression_profile,
        output_dir,
        control,
    )?;
    Ok(())
}

fn reconstruct_from_target_manifest_with_timings(
    target_manifest: &TargetManifest,
    bundles: &[Vec<u8>],
    compression_profile: &CompressionProfile,
    output_dir: &Path,
    control: &GutchOperationControl,
) -> Result<((), ReconstructTimings)> {
    let total_start = Instant::now();
    let mut timings = ReconstructTimings {
        verify_files: true,
        ..ReconstructTimings::default()
    };
    control.checkpoint()?;
    crate::helpers::create_dir_all(output_dir)?;

    let needed_chunks = required_chunk_hashes_typed(target_manifest)?;
    let mut chunk_map: HashMap<ChunkHash, Vec<u8>> = HashMap::with_capacity(needed_chunks.len());
    timings.prepare_wall = total_start.elapsed();
    let restore_start = Instant::now();

    for bundle_data in bundles {
        control.checkpoint()?;
        let t = Instant::now();
        let mut reader = bundle::read_bundle(bundle_data, compression_profile)?;
        let matching_indices: Vec<usize> = reader
            .entries()
            .iter()
            .enumerate()
            .filter_map(|(index, _)| {
                let chunk_hash = reader.chunk_hashes()[index];
                (needed_chunks.contains(&chunk_hash) && !chunk_map.contains_key(&chunk_hash))
                    .then_some(index)
            })
            .collect();
        if matching_indices.is_empty() {
            continue;
        }

        timings.bundle_read_compressed_bytes += bundle_data.len() as u64;
        for index in matching_indices {
            control.checkpoint()?;
            let hash = reader.chunk_hashes()[index];
            let data = reader.read_chunk(index)?;
            timings.bundle_read_decompressed_bytes += data.len() as u64;
            chunk_map.insert(hash, data);
        }
        timings.bundle_read += t.elapsed();

        if chunk_map.len() == needed_chunks.len() {
            break;
        }
    }

    rebuild_files_from_chunks(
        target_manifest,
        &chunk_map,
        output_dir,
        &mut timings,
        control,
    )?;
    timings.restore_wall = restore_start.elapsed();
    timings.total = total_start.elapsed();
    Ok(((), timings))
}

pub fn reconstruct_from_bundle_paths(
    target_manifest: &TargetManifest,
    bundle_paths: &[PathBuf],
    compression_profile: &CompressionProfile,
    verified_source: Option<VerifiedSourceInstall<'_>>,
    output_dir: &Path,
    runtime_hints: &RuntimeResourceHints,
    options: ReconstructOptions,
    control: &GutchOperationControl,
) -> Result<((), ReconstructTimings)> {
    let (needed_chunks, total_start, runtime_plan, timings) =
        prepare_reconstruct(target_manifest, output_dir, runtime_hints, control)?;

    let restore_start = Instant::now();
    let local_chunks = match verified_source {
        Some(source) => read_reusable_source_chunks(source, &needed_chunks, control)?,
        None => HashMap::new(),
    };
    let (result, mut timings) = reconstruct_from_bundle_paths_and_local_chunks(
        target_manifest,
        bundle_paths,
        compression_profile,
        &needed_chunks,
        local_chunks,
        output_dir,
        runtime_plan,
        total_start,
        timings,
        options,
        control,
    )?;
    timings.restore_wall = restore_start.elapsed();
    timings.total = total_start.elapsed();
    Ok((result, timings))
}

pub fn reconstruct_from_repo(
    target_manifest: &TargetManifest,
    repo_paths: &GutchRepoPaths,
    compression_profile: &CompressionProfile,
    output_dir: &Path,
    runtime_hints: &RuntimeResourceHints,
    options: ReconstructOptions,
    control: &GutchOperationControl,
) -> Result<((), ReconstructTimings)> {
    let (needed_chunks, total_start, runtime_plan, mut timings) =
        prepare_reconstruct(target_manifest, output_dir, runtime_hints, control)?;

    let restore_start = Instant::now();
    control.checkpoint()?;
    let chunk_index = crate::repo::load_chunk_index(repo_paths)?;
    let local_chunks = read_loose_repo_chunks(
        repo_paths,
        &chunk_index,
        compression_profile,
        &needed_chunks,
        options,
        runtime_plan.worker_count,
        &mut timings,
        control,
    )?;
    let required_bundle_ids = required_bundle_ids(target_manifest, &chunk_index);
    let bundle_paths = required_bundle_ids
        .iter()
        .map(|bundle_id| crate::repo::bundle_path(repo_paths, bundle_id))
        .collect::<Vec<_>>();

    let (result, mut timings) = reconstruct_from_bundle_paths_and_local_chunks(
        target_manifest,
        &bundle_paths,
        compression_profile,
        &needed_chunks,
        local_chunks,
        output_dir,
        runtime_plan,
        total_start,
        timings,
        options,
        control,
    )?;
    timings.restore_wall = restore_start.elapsed();
    timings.total = total_start.elapsed();
    Ok((result, timings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{ChunkStorageMode, EffectiveMemoryMode, MemoryMode};
    use std::sync::Arc;

    #[derive(Debug)]
    struct AlwaysCancelled;

    impl crate::GutchCancellationHook for AlwaysCancelled {
        fn is_cancelled(&self) -> bool {
            true
        }
    }

    #[test]
    fn pre_cancelled_reconstruct_does_not_create_output() {
        let output_dir = crate::test_util::temp_path("gutch_reconstruct_pre_cancelled");
        let _ = std::fs::remove_dir_all(&output_dir);
        let manifest = TargetManifest::new(
            "cancelled-target".to_string(),
            crate::manifest::TargetTagSet::default(),
        );
        let control = GutchOperationControl::default().with_cancellation(Arc::new(AlwaysCancelled));

        let result = reconstruct_from_target_manifest(
            &manifest,
            &[],
            &CompressionProfile::default(),
            &output_dir,
            &control,
        );

        assert!(matches!(result, Err(crate::Error::Cancelled)));
        assert!(!output_dir.exists());
    }

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
        let bundle_path =
            TestPathBuf::from("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let bundle_paths = vec![bundle_path.clone()];
        let bundle_scan_info = TestHashMap::from([(
            bundle_path,
            BundleScanInfo {
                compression_unit: crate::manifest::CompressionUnit::Bundle,
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
        let bundle_path =
            TestPathBuf::from("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let bundle_paths = vec![bundle_path.clone()];
        let bundle_scan_info = TestHashMap::from([(
            bundle_path,
            BundleScanInfo {
                compression_unit: crate::manifest::CompressionUnit::Bundle,
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
        let bundle_path =
            TestPathBuf::from("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let bundle_paths = vec![bundle_path.clone()];
        let bundle_scan_info = TestHashMap::from([(
            bundle_path,
            BundleScanInfo {
                compression_unit: crate::manifest::CompressionUnit::Bundle,
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
    fn high_memory_no_verify_restore_uses_direct_bundle_to_file_path() {
        let bundle_scan_info = TestHashMap::from([(
            TestPathBuf::from("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            BundleScanInfo {
                compression_unit: crate::manifest::CompressionUnit::Bundle,
                raw_payload_size: 64 * 1024 * 1024,
            },
        )]);

        assert!(should_use_direct_bundle_to_file_restore(
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
            TestPathBuf::from("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            BundleScanInfo {
                compression_unit: crate::manifest::CompressionUnit::Bundle,
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
            TestPathBuf::from("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            BundleScanInfo {
                compression_unit: crate::manifest::CompressionUnit::Bundle,
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
