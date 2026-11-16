//! Transfer contracts: artifact URLs, URL providers, concurrency policy and statistics.

use crate::progress::OperationControl;
use serde::{Deserialize, Serialize};

pub const DEFAULT_TRANSFER_CONCURRENCY: usize = 4;
pub const DEFAULT_ADAPTIVE_TRANSFER_MAXIMUM: usize = 16;
pub const MAX_TRANSFER_CONCURRENCY: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferConcurrencyPolicy {
    Fixed(usize),
    Adaptive { initial: usize, maximum: usize },
}

impl Default for TransferConcurrencyPolicy {
    fn default() -> Self {
        Self::Fixed(DEFAULT_TRANSFER_CONCURRENCY)
    }
}

impl TransferConcurrencyPolicy {
    pub const fn fixed(concurrency: usize) -> Self {
        Self::Fixed(concurrency)
    }

    pub const fn adaptive(initial: usize, maximum: usize) -> Self {
        Self::Adaptive { initial, maximum }
    }

    pub const fn initial(self) -> usize {
        match self {
            Self::Fixed(concurrency) => concurrency,
            Self::Adaptive { initial, .. } => initial,
        }
    }

    pub const fn maximum(self) -> usize {
        match self {
            Self::Fixed(concurrency) => concurrency,
            Self::Adaptive { maximum, .. } => maximum,
        }
    }

    /// Requires 1..=`MAX_TRANSFER_CONCURRENCY` workers with initial <= maximum.
    pub fn validate(self) -> crate::Result<()> {
        let (initial, maximum) = (self.initial(), self.maximum());
        if initial == 0 || initial > maximum || maximum > MAX_TRANSFER_CONCURRENCY {
            return Err(crate::Error::invalid_input(format!(
                "transfer concurrency must be between 1 and {MAX_TRANSFER_CONCURRENCY}, with initial no greater than maximum"
            )));
        }
        Ok(())
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Fixed(_) => "fixed",
            Self::Adaptive { .. } => "adaptive",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct TransferStats {
    pub initial_concurrency: usize,
    pub maximum_concurrency: usize,
    pub peak_concurrency: usize,
    pub adjustment_count: usize,
    pub retry_count: usize,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub elapsed_seconds: f64,
    pub mib_per_second: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactDownload {
    pub artifact_id: String,
    pub url: String,
}

pub trait ArtifactUrlProvider: Send + Sync {
    fn resolve_artifact_urls(
        &self,
        artifact_ids: &[String],
        control: &OperationControl,
    ) -> crate::Result<Vec<ArtifactDownload>>;
}

impl<F> ArtifactUrlProvider for F
where
    F: Fn(&[String], &OperationControl) -> crate::Result<Vec<ArtifactDownload>> + Send + Sync,
{
    fn resolve_artifact_urls(
        &self,
        artifact_ids: &[String],
        control: &OperationControl,
    ) -> crate::Result<Vec<ArtifactDownload>> {
        self(artifact_ids, control)
    }
}
