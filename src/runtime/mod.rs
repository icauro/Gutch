use rayon::ThreadPool;
use serde::Deserialize;
use serde::Serialize;

use crate::Error;
use crate::Result;

const OPERATION_MEMORY_BUDGET_BYTES: u64 = 512 * 1024 * 1024;

/// Shared per-operation admission for payload and codec working memory.
pub(crate) struct MemoryBudget {
    available: std::sync::Mutex<u64>,
    changed: std::sync::Condvar,
}
pub(crate) struct MemoryPermit<'a> {
    budget: &'a MemoryBudget,
    bytes: u64,
}
impl MemoryBudget {
    pub(crate) fn new() -> Self {
        Self {
            available: std::sync::Mutex::new(OPERATION_MEMORY_BUDGET_BYTES),
            changed: std::sync::Condvar::new(),
        }
    }
    pub(crate) fn acquire(
        &self,
        bytes: u64,
        control: &crate::progress::GutchOperationControl,
    ) -> Result<MemoryPermit<'_>> {
        if bytes > OPERATION_MEMORY_BUDGET_BYTES {
            return Err(Error::manifest(
                "artifact working memory exceeds operation budget",
            ));
        }
        let mut available = self
            .available
            .lock()
            .map_err(|_| Error::manifest("memory budget poisoned"))?;
        // Keep one measured wait across cancellation polling, not one slice per timeout.
        let waiting = (*available < bytes)
            .then(|| control.profile_span("runtime.memory.wait"));
        while *available < bytes {
            control.checkpoint()?;
            available = self
                .changed
                .wait_timeout(available, std::time::Duration::from_millis(50))
                .map_err(|_| Error::manifest("memory budget poisoned"))?
                .0;
        }
        control.checkpoint()?;
        drop(waiting);
        *available -= bytes;
        Ok(MemoryPermit {
            budget: self,
            bytes,
        })
    }
}
impl Drop for MemoryPermit<'_> {
    fn drop(&mut self) {
        if let Ok(mut available) = self.budget.available.lock() {
            *available += self.bytes;
            self.budget.changed.notify_all();
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuntimeResourceHints {
    pub jobs: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimePlan {
    pub worker_count: usize,
}

pub(crate) fn build_thread_pool(
    worker_count: usize,
    thread_name: &'static str,
) -> Result<ThreadPool> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(worker_count.max(1))
        .thread_name(move |index| format!("{thread_name} #{}", index + 1))
        .build()
        .map_err(|err| Error::manifest(format!("failed to create thread pool: {err}")))
}

pub(crate) fn plan_runtime(hints: &RuntimeResourceHints) -> RuntimePlan {
    let logical_workers = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
        .max(1);
    let default_workers = num_cpus::get_physical().max(1).min(logical_workers);
    let worker_count = hints
        .jobs
        .unwrap_or(default_workers)
        .max(1)
        .min(logical_workers);

    RuntimePlan { worker_count }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhausted_memory_admission_remains_cancellable() {
        #[derive(Debug)]
        struct Cancellation(std::sync::atomic::AtomicBool);
        impl crate::progress::GutchCancellationHook for Cancellation {
            fn is_cancelled(&self) -> bool {
                self.0.load(std::sync::atomic::Ordering::Acquire)
            }
        }
        let budget = MemoryBudget::new();
        let profiler = std::sync::Arc::new(crate::profile::GutchProfiler::new(
            crate::profile::GutchProfileMode::Detailed,
        ));
        let control = crate::progress::GutchOperationControl::default()
            .with_profiler(profiler.clone());
        let permit = budget
            .acquire(OPERATION_MEMORY_BUDGET_BYTES, &control)
            .unwrap();
        let cancellation =
            std::sync::Arc::new(Cancellation(std::sync::atomic::AtomicBool::new(false)));
        let waiting_control = control.clone().with_cancellation(cancellation.clone());
        std::thread::scope(|scope| {
            let waiter =
                scope.spawn(|| budget.acquire(16 * 1024 * 1024, &waiting_control).is_err());
            cancellation
                .0
                .store(true, std::sync::atomic::Ordering::Release);
            assert!(waiter.join().unwrap());
        });
        let summary = profiler.summary();
        let waits: Vec<_> = summary.spans.iter()
            .filter(|span| span.name == "runtime.memory.wait").collect();
        assert_eq!(waits.len(), 1);
        assert_eq!(waits[0].count, 1);
        drop(permit);
        assert!(
            budget
                .acquire(OPERATION_MEMORY_BUDGET_BYTES, &control)
                .is_ok()
        );
        assert!(
            budget
                .acquire(OPERATION_MEMORY_BUDGET_BYTES + 1, &control)
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
        });
        assert_eq!(plan.worker_count, logical_workers);

        let plan = plan_runtime(&RuntimeResourceHints { jobs: Some(1) });
        assert_eq!(plan.worker_count, 1);
    }
}
