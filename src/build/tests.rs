use super::*;

#[test]
fn bundle_sizing_policy_zero_bytes_uses_preferred() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.snapshot_size_for(0), 10);
}

#[test]
fn bundle_sizing_policy_below_preferred_uses_preferred() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.snapshot_size_for(5), 10);
}

#[test]
fn bundle_sizing_policy_prefers_preferred_for_large_inputs() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.snapshot_size_for(1_000), 10);
}

#[test]
fn bundle_sizing_policy_slightly_above_preferred_splits_evenly() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.snapshot_size_for(15), 10);
}

#[test]
fn bundle_sizing_policy_exactly_preferred() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.snapshot_size_for(10), 10);
}

#[test]
fn bundle_sizing_policy_caps_invalid_preferred_above_max() {
    let policy = BundleSizingPolicy::new(100, 200);
    assert_eq!(policy.snapshot_size_for(1_000), 100);
}

#[test]
fn bundle_sizing_policy_exact_preferred_divisor() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.snapshot_size_for(200), 10);
}

#[test]
fn bundle_sizing_policy_default_sizes() {
    let policy = BundleSizingPolicy::default();
    assert!(policy.max_size > 0);
    assert!(policy.preferred_size > 0);
    assert!(policy.max_size >= policy.preferred_size);
}

#[test]
fn bundle_raw_size_uses_compression_ratio_from_preferred_size() {
    let policy = BundleSizingPolicy::new(32 * 1024 * 1024, 16 * 1024 * 1024);
    let compressed_size = policy.snapshot_size_for(790 * 1024 * 1024);
    assert_eq!(compressed_size, 16 * 1024 * 1024);

    let raw_size = raw_size_size_for_bundle_compression(compressed_size, 0.5, policy);
    assert_eq!(raw_size, 32 * 1024 * 1024);
}
