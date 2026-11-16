//! Worker counts, memory budgets and thread pools for one operation.

use crate::Error;
use crate::Result;
use rayon::ThreadPool;
use serde::Deserialize;
use serde::Serialize;
use std::num::NonZeroU64;
use std::num::NonZeroUsize;
use std::thread::available_parallelism;

const FALLBACK_MEMORY_BUDGET_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RuntimeResourceHints {
    pub jobs: Option<usize>,
    /// Shared payload/codec admission budget; None selects 25% of physical
    /// RAM, or of the container memory limit when that is lower.
    pub memory_budget_bytes: Option<NonZeroU64>,
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimePlan {
    pub(crate) worker_count: usize,
    pub(crate) memory_budget_bytes: u64,
}

pub(crate) fn build_thread_pool(
    worker_count: usize,
    thread_name: &'static str,
) -> Result<ThreadPool> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(worker_count.max(1))
        .thread_name(move |index| format!("{thread_name} #{}", index + 1))
        .build()
        .map_err(|err| {
            Error::Io(std::io::Error::other(format!(
                "failed to create {thread_name} pool: {err}"
            )))
        })
}

pub(crate) fn plan_runtime(hints: &RuntimeResourceHints) -> RuntimePlan {
    let logical_workers = available_parallelism().map_or(1, NonZeroUsize::get);
    let default_workers = sysinfo::System::physical_core_count()
        .unwrap_or(1)
        .max(1)
        .min(logical_workers);
    let worker_count = hints
        .jobs
        .unwrap_or(default_workers)
        .max(1)
        .min(logical_workers);

    RuntimePlan {
        worker_count,
        memory_budget_bytes: hints
            .memory_budget_bytes
            .map(NonZeroU64::get)
            .unwrap_or_else(automatic_memory_budget),
    }
}

fn automatic_memory_budget() -> u64 {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    let total = system.total_memory();
    #[cfg(target_os = "linux")]
    let cgroup_limit = system.cgroup_limits().map(|limits| limits.total_memory);
    #[cfg(not(target_os = "linux"))]
    let cgroup_limit = None;
    budget_from_limits(total, cgroup_limit)
}

fn budget_from_limits(total: u64, cgroup_limit: Option<u64>) -> u64 {
    let total = cgroup_limit.map_or(total, |limit| total.min(limit));
    if total == 0 {
        FALLBACK_MEMORY_BUDGET_BYTES
    } else {
        (total / 4).max(1)
    }
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
