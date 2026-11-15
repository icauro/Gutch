//! Gutch artifact engine.
//!
//! Cross-crate callers use [`api`]. All other modules are implementation
//! details of the engine or its command adapter.

#![warn(clippy::all)]
#![warn(rust_2024_compatibility)]

// Folder prefixes are dependency layers: a module uses only lower-numbered
// modules, so a lexical listing of `src/` shows the dependency order.
#[path = "11_api.rs"]
pub mod api;
#[path = "04_artifact/mod.rs"]
mod artifact;
#[path = "08_capture/mod.rs"]
mod capture;
#[path = "10_cli/mod.rs"]
mod cli;
#[path = "00_error/mod.rs"]
mod error;
#[path = "03_filesystem/mod.rs"]
mod filesystem;
#[path = "09_installation/mod.rs"]
mod installation;
#[path = "05_manifest/mod.rs"]
mod manifest;
#[path = "05_path_compare.rs"]
mod path_compare;
#[path = "01_profiling.rs"]
mod profiling;
#[path = "02_progress/mod.rs"]
mod progress;
#[path = "07_repo/mod.rs"]
mod repo;
#[path = "03_runtime/mod.rs"]
mod runtime;
#[path = "06_snapshot/mod.rs"]
mod snapshot;
#[path = "08_sync/mod.rs"]
mod sync;
#[path = "05_transfer/mod.rs"]
mod transfer;

#[cfg(test)]
mod test_util;

pub(crate) use error::Error;
pub(crate) use error::Result;

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "04_artifact/fuzz_tests.rs"]
mod engine_fuzz;
