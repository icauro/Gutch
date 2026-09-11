//! Gutch artifact engine.
//!
//! Cross-crate callers use [`api`]. All other modules are implementation
//! details of the engine or its command adapter.

#![warn(clippy::all)]
#![warn(rust_2024_compatibility)]

pub mod api;
mod artifact;
mod capture;
mod cli;
mod config;
mod error;
mod file_integrity;
mod filesystem;
mod installation;
mod manifest;
mod path_compare;
mod profile;
mod progress;
mod repo;
mod runtime;
mod snapshot;
#[cfg(test)]
mod test_util;
mod transfer;

pub(crate) use error::Error;
pub(crate) use error::Result;

#[cfg(test)]
mod tests;
