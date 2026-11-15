use super::*;

#[test]
fn automatic_budget_respects_ram_and_container_limits() {
    assert_eq!(budget_from_limits(16 * 1024, None), 4 * 1024);
    assert_eq!(budget_from_limits(16 * 1024, Some(2 * 1024)), 512);
    assert_eq!(budget_from_limits(16 * 1024, Some(32 * 1024)), 4 * 1024);
    assert_eq!(budget_from_limits(0, None), FALLBACK_MEMORY_BUDGET_BYTES);
    let hints = RuntimeResourceHints {
        memory_budget_bytes: std::num::NonZeroU64::new(123),
        ..Default::default()
    };
    assert_eq!(plan_runtime(&hints).memory_budget_bytes, 123);
}

#[test]
fn default_jobs_use_physical_parallelism() {
    let logical_workers = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
        .max(1);
    let physical_workers = num_cpus::get_physical().max(1).min(logical_workers);
    let plan = plan_runtime(&RuntimeResourceHints::default());
    assert_eq!(plan.worker_count, physical_workers);
}

#[test]
fn explicit_jobs_override_the_default_and_respect_cpu_limit() {
    let logical_workers = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
        .max(1);
    let plan = plan_runtime(&RuntimeResourceHints {
        jobs: Some(logical_workers.saturating_add(8)),
        ..Default::default()
    });
    assert_eq!(plan.worker_count, logical_workers);

    let plan = plan_runtime(&RuntimeResourceHints {
        jobs: Some(1),
        ..Default::default()
    });
    assert_eq!(plan.worker_count, 1);
}
