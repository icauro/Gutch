mod pipeline;
mod source;
mod types;

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use crate::Result;
use crate::bundle;
use crate::hash::ChunkHash;
use crate::manifest::{CompressionProfile, snapshot::SnapshotManifest};
use crate::progress::GutchOperationControl;
use crate::repo::{GutchRepoPaths, required_bundle_ids, required_chunk_hashes_typed};
use crate::runtime::RuntimeResourceHints;

use pipeline::{
    BundleReconstructRequest, ReadLooseRepoChunksRequest, prepare_reconstruct,
    read_loose_repo_chunks, rebuild_files_from_chunks,
    reconstruct_from_bundle_paths_and_local_chunks,
};
use source::read_reusable_source_chunks;
pub use types::{ReconstructOptions, ReconstructRequest, ReconstructTimings, VerifiedSourceTree};

pub(crate) fn prepare_snapshot_entries(
    snapshot_manifest: &SnapshotManifest,
    output_dir: &Path,
) -> Result<()> {
    crate::manifest::validate_snapshot_entries(&snapshot_manifest.entries)?;
    crate::helpers::create_dir_all(output_dir)?;
    pipeline::create_snapshot_directories(snapshot_manifest, output_dir)
}

pub(crate) fn finalize_snapshot_entries(
    snapshot_manifest: &SnapshotManifest,
    output_dir: &Path,
) -> Result<()> {
    pipeline::finalize_snapshot_entries(snapshot_manifest, output_dir)
}

#[cfg(test)]
use crate::runtime::RuntimePlan;
#[cfg(test)]
use pipeline::{
    should_stage_bundle_payloads_in_memory, should_use_direct_bundle_to_file_reconstruct,
    should_use_direct_bundle_to_file_reconstruct_experimental,
};
#[cfg(test)]
use std::collections::HashMap as TestHashMap;
#[cfg(test)]
use std::path::PathBuf as TestPathBuf;
#[cfg(test)]
use types::BundleScanInfo;

pub fn reconstruct_from_snapshot_manifest(
    snapshot_manifest: &SnapshotManifest,
    bundles: &[Vec<u8>],
    compression_profile: &CompressionProfile,
    output_dir: &Path,
    control: &GutchOperationControl,
) -> Result<()> {
    let (_, _) = reconstruct_from_snapshot_manifest_with_timings(
        snapshot_manifest,
        bundles,
        compression_profile,
        output_dir,
        control,
    )?;
    Ok(())
}

fn reconstruct_from_snapshot_manifest_with_timings(
    snapshot_manifest: &SnapshotManifest,
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
    crate::manifest::validate_snapshot_entries(&snapshot_manifest.entries)?;
    crate::helpers::create_dir_all(output_dir)?;
    pipeline::create_snapshot_directories(snapshot_manifest, output_dir)?;

    let needed_chunks = required_chunk_hashes_typed(snapshot_manifest)?;
    let mut chunk_map: HashMap<ChunkHash, Vec<u8>> = HashMap::with_capacity(needed_chunks.len());
    timings.prepare_wall = total_start.elapsed();
    let reconstruct_start = Instant::now();

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
        snapshot_manifest,
        &chunk_map,
        output_dir,
        &mut timings,
        control,
    )?;
    timings.reconstruct_wall = reconstruct_start.elapsed();
    timings.total = total_start.elapsed();
    Ok(((), timings))
}

pub fn reconstruct_from_bundle_paths(
    request: ReconstructRequest<'_>,
) -> Result<((), ReconstructTimings)> {
    let (needed_chunks, total_start, runtime_plan, timings) = prepare_reconstruct(
        request.snapshot_manifest,
        request.output_dir,
        request.runtime_hints,
        request.control,
    )?;

    let reconstruct_start = Instant::now();
    let local_chunks = match request.verified_source {
        Some(source) => read_reusable_source_chunks(source, &needed_chunks, request.control)?,
        None => HashMap::new(),
    };
    let (result, mut timings) =
        reconstruct_from_bundle_paths_and_local_chunks(BundleReconstructRequest {
            snapshot_manifest: request.snapshot_manifest,
            bundle_paths: request.bundle_paths,
            compression_profile: request.compression_profile,
            needed_chunks: &needed_chunks,
            local_chunks,
            output_dir: request.output_dir,
            runtime_plan,
            total_start,
            timings,
            options: request.options,
            control: request.control,
        })?;
    timings.reconstruct_wall = reconstruct_start.elapsed();
    timings.total = total_start.elapsed();
    Ok((result, timings))
}

pub fn reconstruct_from_repo(
    snapshot_manifest: &SnapshotManifest,
    repo_paths: &GutchRepoPaths,
    compression_profile: &CompressionProfile,
    output_dir: &Path,
    runtime_hints: &RuntimeResourceHints,
    options: ReconstructOptions,
    control: &GutchOperationControl,
) -> Result<((), ReconstructTimings)> {
    let (needed_chunks, total_start, runtime_plan, mut timings) =
        prepare_reconstruct(snapshot_manifest, output_dir, runtime_hints, control)?;

    let reconstruct_start = Instant::now();
    control.checkpoint()?;
    let chunk_index = crate::repo::load_chunk_index(repo_paths)?;
    let local_chunks = read_loose_repo_chunks(ReadLooseRepoChunksRequest {
        repo_paths,
        chunk_index: &chunk_index,
        compression_profile,
        needed_chunks: &needed_chunks,
        options,
        worker_count: runtime_plan.worker_count,
        timings: &mut timings,
        control,
    })?;
    let required_bundle_ids = required_bundle_ids(snapshot_manifest, &chunk_index);
    let bundle_paths = required_bundle_ids
        .iter()
        .map(|bundle_id| crate::repo::bundle_path(repo_paths, bundle_id))
        .collect::<Result<Vec<_>>>()?;

    let (result, mut timings) =
        reconstruct_from_bundle_paths_and_local_chunks(BundleReconstructRequest {
            snapshot_manifest,
            bundle_paths: &bundle_paths,
            compression_profile,
            needed_chunks: &needed_chunks,
            local_chunks,
            output_dir,
            runtime_plan,
            total_start,
            timings,
            options,
            control,
        })?;
    timings.reconstruct_wall = reconstruct_start.elapsed();
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
        let manifest = SnapshotManifest::new(
            "cancelled-snapshot".to_string(),
            crate::manifest::SnapshotTagSet::default(),
        );
        let control = GutchOperationControl::default().with_cancellation(Arc::new(AlwaysCancelled));

        let result = reconstruct_from_snapshot_manifest(
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
    fn high_memory_no_verify_reconstruct_uses_direct_bundle_to_file_path() {
        let bundle_scan_info = TestHashMap::from([(
            TestPathBuf::from("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            BundleScanInfo {
                compression_unit: crate::manifest::CompressionUnit::Bundle,
                raw_payload_size: 64 * 1024 * 1024,
            },
        )]);

        assert!(should_use_direct_bundle_to_file_reconstruct(
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
    fn experimental_high_memory_no_verify_reconstruct_can_use_direct_bundle_to_file_path() {
        let bundle_scan_info = TestHashMap::from([(
            TestPathBuf::from("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            BundleScanInfo {
                compression_unit: crate::manifest::CompressionUnit::Bundle,
                raw_payload_size: 64 * 1024 * 1024,
            },
        )]);

        assert!(should_use_direct_bundle_to_file_reconstruct_experimental(
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
    fn reconstruct_with_file_verify_keeps_staged_reconstruct_path() {
        let bundle_scan_info = TestHashMap::from([(
            TestPathBuf::from("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            BundleScanInfo {
                compression_unit: crate::manifest::CompressionUnit::Bundle,
                raw_payload_size: 64 * 1024 * 1024,
            },
        )]);

        assert!(!should_use_direct_bundle_to_file_reconstruct(
            &HashMap::new(),
            &bundle_scan_info,
            &runtime_plan(EffectiveMemoryMode::High, 1024),
            ReconstructOptions {
                verify_chunks: false,
                verify_files: true,
            },
        ));
    }

    #[test]
    fn reconstructs_empty_directory_and_safe_internal_symlink() {
        let output = crate::test_util::temp_path("gutch_entry_reconstruct");
        let _ = std::fs::remove_dir_all(&output);
        let mut manifest =
            SnapshotManifest::new("entries".into(), crate::manifest::SnapshotTagSet::default());
        manifest.entries = vec![
            crate::manifest::SnapshotEntry::Directory(crate::manifest::DirectoryManifest {
                path: "empty".into(),
                read_only: false,
            }),
            crate::manifest::SnapshotEntry::Symlink(crate::manifest::SymlinkManifest {
                path: "current".into(),
                target_path: "empty".into(),
            }),
        ];
        let result = reconstruct_from_snapshot_manifest(
            &manifest,
            &[],
            &CompressionProfile::default(),
            &output,
            &GutchOperationControl::default(),
        );
        #[cfg(unix)]
        assert!(result.is_ok(), "{result:?}");
        #[cfg(windows)]
        if let Err(error) = &result {
            assert!(
                error.to_string().contains("Developer Mode")
                    || error.to_string().contains("symbolic-link privilege")
            );
        }
        if result.is_ok() {
            assert!(output.join("empty").is_dir());
            assert!(
                std::fs::symlink_metadata(output.join("current"))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        }
        let _ = std::fs::remove_dir_all(output);
    }
}
