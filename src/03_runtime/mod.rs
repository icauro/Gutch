//! Worker plans and memory admission for capture and installation operations.

use rayon::ThreadPool;
use serde::Deserialize;
use serde::Serialize;

use crate::Error;
use crate::Result;
use crate::progress::OperationControl;
use std::num::NonZeroU64;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::thread::available_parallelism;
use std::time::Duration;

const FALLBACK_MEMORY_BUDGET_BYTES: u64 = 512 * 1024 * 1024;
/// How often a blocked admission checks for cancellation.
const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Shared per-operation admission for payload and codec working memory.
pub(crate) struct MemoryBudget {
    limit: u64,
    available: Mutex<u64>,
    changed: Condvar,
}
pub(crate) struct MemoryPermit {
    budget: Arc<MemoryBudget>,
    bytes: u64,
}
impl MemoryBudget {
    pub(crate) fn new(limit: u64) -> Self {
        Self {
            limit,
            available: Mutex::new(limit),
            changed: Condvar::new(),
        }
    }
    fn validate_request(&self, bytes: u64) -> Result<()> {
        if bytes > self.limit {
            return Err(Error::invalid_input(format!(
                "job requires {bytes} bytes but memory budget is {} bytes; increase the memory budget",
                self.limit
            )));
        }
        Ok(())
    }
    /// The counter stays consistent across panics: every update is a single
    /// arithmetic step, so a poisoned lock still holds a valid count.
    fn available(&self) -> MutexGuard<'_, u64> {
        self.available
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub(crate) fn try_acquire(self: &Arc<Self>, bytes: u64) -> Result<Option<MemoryPermit>> {
        self.validate_request(bytes)?;
        let mut available = self.available();
        if *available < bytes {
            return Ok(None);
        }
        *available -= bytes;
        Ok(Some(MemoryPermit {
            budget: self.clone(),
            bytes,
        }))
    }
    pub(crate) fn acquire(
        self: &Arc<Self>,
        bytes: u64,
        control: &OperationControl,
    ) -> Result<MemoryPermit> {
        let permit = match self.try_acquire(bytes)? {
            Some(permit) => permit,
            None => self.wait_for(bytes, control)?,
        };
        // A cancelled operation gives the reservation back when the permit drops.
        control.checkpoint()?;
        Ok(permit)
    }
    /// Blocks until `bytes` fit, polling for cancellation. The profiling span
    /// and checkpoints run outside the budget lock.
    fn wait_for(self: &Arc<Self>, bytes: u64, control: &OperationControl) -> Result<MemoryPermit> {
        // Keep one measured wait across cancellation polling, not one slice per timeout.
        let _waiting = control.profile_span("runtime.memory.wait");
        loop {
            control.checkpoint()?;
            let available = self.available();
            let (mut available, _) = self
                .changed
                .wait_timeout_while(available, CANCELLATION_POLL_INTERVAL, |available| {
                    *available < bytes
                })
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if *available >= bytes {
                *available -= bytes;
                return Ok(MemoryPermit {
                    budget: self.clone(),
                    bytes,
                });
            }
        }
    }
}
impl Drop for MemoryPermit {
    fn drop(&mut self) {
        *self.budget.available() += self.bytes;
        self.budget.changed.notify_all();
    }
}

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
    let default_workers = num_cpus::get_physical().max(1).min(logical_workers);
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
mod tests {
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
    fn nonblocking_admission_releases_its_reservation() {
        let budget = std::sync::Arc::new(MemoryBudget::new(10));
        let permit = budget.try_acquire(7).unwrap().unwrap();
        assert!(budget.try_acquire(4).unwrap().is_none());
        assert!(budget.try_acquire(11).is_err());
        drop(permit);
        assert!(budget.try_acquire(10).unwrap().is_some());
    }

    #[test]
    fn exhausted_memory_admission_remains_cancellable() {
        #[derive(Debug)]
        struct Cancellation(std::sync::atomic::AtomicBool);
        impl crate::progress::CancellationHook for Cancellation {
            fn is_cancelled(&self) -> bool {
                self.0.load(std::sync::atomic::Ordering::Acquire)
            }
        }
        let budget = std::sync::Arc::new(MemoryBudget::new(FALLBACK_MEMORY_BUDGET_BYTES));
        let profiler = std::sync::Arc::new(crate::profiling::Profiler::new(
            crate::profiling::ProfileMode::Detailed,
        ));
        let control = crate::progress::OperationControl::default().with_profiler(profiler.clone());
        let permit = budget
            .acquire(FALLBACK_MEMORY_BUDGET_BYTES, &control)
            .unwrap();
        let cancellation =
            std::sync::Arc::new(Cancellation(std::sync::atomic::AtomicBool::new(false)));
        let waiting_control = control.clone().with_cancellation(cancellation.clone());
        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| budget.acquire(16 * 1024 * 1024, &waiting_control));
            cancellation
                .0
                .store(true, std::sync::atomic::Ordering::Release);
            assert!(
                waiter
                    .join()
                    .unwrap()
                    .is_err_and(|error| error.is_cancelled())
            );
        });
        let summary = profiler.summary();
        let waits: Vec<_> = summary
            .spans
            .iter()
            .filter(|span| span.name == "runtime.memory.wait")
            .collect();
        assert_eq!(waits.len(), 1);
        assert_eq!(waits[0].count, 1);
        drop(permit);
        assert!(
            budget
                .acquire(FALLBACK_MEMORY_BUDGET_BYTES, &control)
                .is_ok()
        );
        assert!(
            budget
                .acquire(FALLBACK_MEMORY_BUDGET_BYTES + 1, &control)
                .is_err()
        );
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
}
