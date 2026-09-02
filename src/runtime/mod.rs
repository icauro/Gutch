use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuntimeResourceHints {
    pub jobs: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimePlan {
    pub worker_count: usize,
}

pub(crate) fn plan_build_runtime(hints: &RuntimeResourceHints) -> RuntimePlan {
    plan_runtime(hints)
}

pub(crate) fn plan_verify_runtime(hints: &RuntimeResourceHints) -> RuntimePlan {
    plan_runtime(hints)
}

pub(crate) fn plan_reconstruct_runtime(hints: &RuntimeResourceHints) -> RuntimePlan {
    plan_runtime(hints)
}

fn plan_runtime(hints: &RuntimeResourceHints) -> RuntimePlan {
    let available_workers = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
        .max(1);
    let worker_count = hints
        .jobs
        .unwrap_or(available_workers)
        .max(1)
        .min(available_workers);

    RuntimePlan { worker_count }
}

pub(crate) fn run_in_thread_pool<T: Send, F: FnOnce() -> T + Send>(
    worker_count: usize,
    work: F,
) -> Result<T> {
    if worker_count <= 1 {
        return Ok(work());
    }

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(worker_count)
        .build()
        .map_err(|err| Error::manifest(format!("failed to create thread pool: {}", err)))?;
    Ok(pool.install(work))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_jobs_use_available_parallelism() {
        let available_workers = std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(1)
            .max(1);
        let plan = plan_build_runtime(&RuntimeResourceHints::default());
        assert_eq!(plan.worker_count, available_workers);
    }

    #[test]
    fn explicit_jobs_override_the_default_and_respect_cpu_limit() {
        let available_workers = std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(1)
            .max(1);
        let plan = plan_reconstruct_runtime(&RuntimeResourceHints {
            jobs: Some(available_workers.saturating_add(8)),
        });
        assert_eq!(plan.worker_count, available_workers);

        let plan = plan_verify_runtime(&RuntimeResourceHints { jobs: Some(1) });
        assert_eq!(plan.worker_count, 1);
    }
}
