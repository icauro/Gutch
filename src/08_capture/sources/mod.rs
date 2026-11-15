//! Source selection and batch capture shared by hosts and the CLI.

mod batch;
pub(crate) mod config;
pub(crate) mod configured;
pub(crate) mod discovery;
mod resolve;

pub use batch::{capture_snapshots, plan_snapshot_captures};
pub use resolve::{ResolvedCaptureSource, resolve_capture_sources};
