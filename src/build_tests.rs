use super::*;

#[test]
fn bundle_sizing_policy_zero_bytes_uses_max() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.target_size_for(0), 100);
}

#[test]
fn bundle_sizing_policy_below_max_uses_actual() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.target_size_for(50), 50);
}

#[test]
fn bundle_sizing_policy_below_min_preferred_uses_min() {
    let policy = BundleSizingPolicy::new(100, 50);
    assert_eq!(policy.target_size_for(20), 50);
}

#[test]
fn bundle_sizing_policy_slightly_above_max_splits_evenly() {
    let policy = BundleSizingPolicy::new(100, 10);
    let result = policy.target_size_for(150);
    assert!(result >= 10);
    assert!(result <= 100);
}

#[test]
fn bundle_sizing_policy_exactly_max() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.target_size_for(100), 100);
}

#[test]
fn bundle_sizing_policy_large_input_splits() {
    let policy = BundleSizingPolicy::new(100, 10);
    let result = policy.target_size_for(1000);
    assert!(result >= 10);
    assert!(result <= 100);
}

#[test]
fn bundle_sizing_policy_exact_divisor() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.target_size_for(200), 100);
}

#[test]
fn bundle_sizing_policy_default_sizes() {
    let policy = BundleSizingPolicy::default();
    assert!(policy.max_size > 0);
    assert!(policy.min_preferred_size > 0);
    assert!(policy.max_size >= policy.min_preferred_size);
}
