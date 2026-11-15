//! Shared admission for payload and codec working memory.

use crate::Error;
use crate::Result;
use crate::progress::OperationControl;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::time::Duration;

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
#[cfg(test)]
#[path = "memory_budget_tests.rs"]
mod tests;
