//! Capture source discovery, stable scanning, reuse planning, and snapshot publication.

mod build;
pub(crate) mod configured_sources;
pub(crate) mod contracts;
pub(crate) mod estimate;
pub(crate) mod execute;
pub(crate) mod pipeline;
pub(crate) mod plan;
mod progress;
mod publish;
pub(crate) mod reuse;
pub(crate) mod scan;
pub(crate) mod sources;
pub(crate) mod types;
