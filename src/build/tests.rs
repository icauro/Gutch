use super::*;
use std::collections::HashMap;

fn existing_locations_for_manifest(
    manifest: &crate::manifest::target::TargetManifest,
) -> HashMap<String, ExistingChunkLocation> {
    manifest
        .files
        .iter()
        .flat_map(|file| file.chunks.iter())
        .map(|chunk| {
            (
                chunk.chunk_hash.clone(),
                ExistingChunkLocation {
                    bundle_id: "existing-bundle".to_string(),
                    raw_size: chunk.raw_size,
                    compressed_size: chunk.raw_size,
                    storage_mode: crate::manifest::ArtifactStorageMode::Bundled,
                },
            )
        })
        .collect()
}

fn build_test_target(dir: &std::path::Path) -> BuildResult {
    build_from_directory(
        dir,
        "test-repo",
        "test-target",
        &crate::manifest::TargetTagSet::default(),
    )
    .unwrap()
}

#[test]
fn bundle_sizing_policy_zero_bytes_uses_preferred() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.target_size_for(0), 10);
}

#[test]
fn bundle_sizing_policy_below_preferred_uses_preferred() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.target_size_for(5), 10);
}

#[test]
fn bundle_sizing_policy_prefers_preferred_for_large_inputs() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.target_size_for(1_000), 10);
}

#[test]
fn bundle_sizing_policy_slightly_above_preferred_splits_evenly() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.target_size_for(15), 10);
}

#[test]
fn bundle_sizing_policy_exactly_preferred() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.target_size_for(10), 10);
}

#[test]
fn bundle_sizing_policy_caps_invalid_preferred_above_max() {
    let policy = BundleSizingPolicy::new(100, 200);
    assert_eq!(policy.target_size_for(1_000), 100);
}

#[test]
fn bundle_sizing_policy_exact_preferred_divisor() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.target_size_for(200), 10);
}

#[test]
fn bundle_sizing_policy_default_sizes() {
    let policy = BundleSizingPolicy::default();
    assert!(policy.max_size > 0);
    assert!(policy.preferred_size > 0);
    assert!(policy.max_size >= policy.preferred_size);
}

#[test]
fn bundle_raw_target_size_uses_compression_ratio_from_preferred_target() {
    let policy = BundleSizingPolicy::new(32 * 1024 * 1024, 16 * 1024 * 1024);
    let compressed_target = policy.target_size_for(790 * 1024 * 1024);
    assert_eq!(compressed_target, 16 * 1024 * 1024);

    let raw_target = raw_target_size_for_bundle_compression(compressed_target, 0.5, policy);
    assert_eq!(raw_target, 32 * 1024 * 1024);
}

#[test]
fn bundle_compression_workers_do_not_exceed_bundle_count() {
    assert_eq!(bundle_compression_worker_count(0, 16), 1);
    assert_eq!(bundle_compression_worker_count(2, 16), 2);
    assert_eq!(bundle_compression_worker_count(8, 4), 4);
    assert_eq!(bundle_compression_worker_count(8, 0), 1);
}

#[test]
fn experimental_prehash_reuses_matching_manifest_without_chunking() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let content = b"unchanged target content".repeat(1024);
    std::fs::write(source.join("game.bin"), &content).unwrap();

    let initial = build_test_target(&source);
    let expected_file = initial.target_manifest.files[0].clone();
    let mut nonmatching_file = expected_file.clone();
    nonmatching_file.full_file_hash =
        crate::hash::hash_hex(&blake3::hash(&b"different target content".repeat(1024)));
    let previous_manifests = HashMap::from([(
        "game.bin".to_string(),
        vec![nonmatching_file, expected_file.clone()],
    )]);
    let existing_locations = existing_locations_for_manifest(&initial.target_manifest);
    let single_pass_config = BuildConfig::with_ids(
        "test-repo",
        "test-target",
        &crate::manifest::TargetTagSet::default(),
    )
    .with_previous_manifests(&previous_manifests)
    .with_existing_chunks(&existing_locations);
    let (_, single_pass_timings) = build_with_config(&source, &single_pass_config).unwrap();
    let config = BuildConfig::with_ids(
        "test-repo",
        "test-target",
        &crate::manifest::TargetTagSet::default(),
    )
    .with_previous_manifests(&previous_manifests)
    .with_existing_chunks(&existing_locations)
    .with_experimental_prehash_reuse(true);

    let (result, timings) = build_with_config(&source, &config).unwrap();

    assert_eq!(single_pass_timings.prehash_bytes, 0);
    assert_eq!(single_pass_timings.stream_chunk_bytes, content.len() as u64);
    assert_eq!(single_pass_timings.file_hash_bytes, content.len() as u64);
    assert_eq!(
        result.target_manifest.files[0].full_file_hash,
        expected_file.full_file_hash
    );
    assert_eq!(timings.prehash_candidate_file_count, 1);
    assert_eq!(timings.prehash_reused_file_count, 1);
    assert_eq!(timings.prehash_mismatch_file_count, 0);
    assert_eq!(timings.prehash_bytes, content.len() as u64);
    assert_eq!(timings.prehash_reused_bytes, content.len() as u64);
    assert_eq!(timings.stream_chunk_bytes, 0);
    assert_eq!(timings.file_hash_bytes, 0);
    assert_eq!(timings.compression_input_bytes, 0);
}

#[test]
fn experimental_prehash_mismatch_falls_back_to_chunking() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let initial_content = b"a".repeat(64 * 1024);
    let changed_content = b"b".repeat(initial_content.len());
    std::fs::write(source.join("game.bin"), &initial_content).unwrap();

    let initial = build_test_target(&source);
    let previous_manifests = HashMap::from([(
        "game.bin".to_string(),
        vec![initial.target_manifest.files[0].clone()],
    )]);
    let existing_locations = existing_locations_for_manifest(&initial.target_manifest);
    std::fs::write(source.join("game.bin"), &changed_content).unwrap();
    let config = BuildConfig::with_ids(
        "test-repo",
        "test-target",
        &crate::manifest::TargetTagSet::default(),
    )
    .with_previous_manifests(&previous_manifests)
    .with_existing_chunks(&existing_locations)
    .with_experimental_prehash_reuse(true);

    let (result, timings) = build_with_config(&source, &config).unwrap();

    assert_ne!(
        result.target_manifest.files[0].full_file_hash,
        initial.target_manifest.files[0].full_file_hash
    );
    assert_eq!(timings.prehash_candidate_file_count, 1);
    assert_eq!(timings.prehash_reused_file_count, 0);
    assert_eq!(timings.prehash_mismatch_file_count, 1);
    assert_eq!(timings.prehash_bytes, changed_content.len() as u64);
    assert_eq!(timings.prehash_reused_bytes, 0);
    assert_eq!(timings.stream_chunk_bytes, changed_content.len() as u64);
    assert_eq!(timings.file_hash_bytes, changed_content.len() as u64);
}
