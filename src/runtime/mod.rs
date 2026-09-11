use rayon::ThreadPool;
use serde::Deserialize;
use serde::Serialize;

use crate::Error;
use crate::Result;

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
            available: std::sync::Mutex::new(512 * 1024 * 1024),
            changed: std::sync::Condvar::new(),
        }
    }
    pub(crate) fn acquire(
        &self,
        bytes: u64,
        control: &crate::progress::GutchOperationControl,
    ) -> Result<MemoryPermit<'_>> {
        if bytes > 512 * 1024 * 1024 {
            return Err(Error::manifest(
                "artifact working memory exceeds operation budget",
            ));
        }
        let mut available = self
            .available
            .lock()
            .map_err(|_| Error::manifest("memory budget poisoned"))?;
        while *available < bytes {
            control.checkpoint()?;
            available = self
                .changed
                .wait_timeout(available, std::time::Duration::from_millis(50))
                .map_err(|_| Error::manifest("memory budget poisoned"))?
                .0;
        }
        control.checkpoint()?;
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
        .thread_name(move |index| format!("{thread_name}-{index}"))
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
