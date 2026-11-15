//! Capture source discovery, stable scanning, reuse planning, and snapshot publication.
//!
//! `sources` selects what to capture, `scan` and `reuse` read the source and
//! existing repository state, `build` packs staged artifacts, and `execute`
//! publishes the snapshot.

pub(crate) mod build;
pub(crate) mod contracts;
pub(crate) mod estimate;
pub(crate) mod execute;
pub(crate) mod plan;
mod publish;
pub(crate) mod reuse;
pub(crate) mod scan;
pub(crate) mod sources;
