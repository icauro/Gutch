use super::*;

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
