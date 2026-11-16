//! Source selection and batch capture shared by hosts and the CLI.

mod batch;
pub(crate) mod config;
mod resolve;

pub use batch::{
    capture_snapshot, capture_snapshots, plan_capture_snapshot, plan_capture_snapshots,
};
pub use resolve::{ResolvedCaptureSource, resolve_capture_sources};
