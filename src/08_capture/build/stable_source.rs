//! Builds from a source inventory and retries once if the source changes meanwhile.

use super::pipeline::build_from_inventory;
use super::types::{BuildConfig, BuildResult, BuildTimings};
use crate::Error;
use crate::Result;
use crate::capture::scan::validate_source_inventory;
use crate::capture::scan::verify_source_inventory_stable;
use crate::capture::scan::{SourceInventory, scan_source_inventory};
use crate::manifest::constraints::RepoConstraints;
use std::path::Path;

pub(crate) struct BuiltCapture {
    pub(crate) result: BuildResult,
    pub(crate) timings: BuildTimings,
    pub(crate) source_inventory: SourceInventory,
}

pub(crate) fn build_with_stable_source(
    source_dir: &Path,
    build_config: &BuildConfig<'_>,
    build_pool: &rayon::ThreadPool,
    mut source_inventory: SourceInventory,
    constraints: &RepoConstraints,
    worker_count: usize,
) -> Result<BuiltCapture> {
    // The source may change while its bytes are read. A change seen during the
    // build or by the confirming rescan gets one retry from a fresh inventory;
    // a second change fails without publishing anything.
    let mut retried = false;
    loop {
        match build_and_confirm_stable(
            source_dir,
            build_config,
            build_pool,
            &source_inventory,
            worker_count,
        ) {
            Err(Error::SourceChanged { .. }) if !retried => {
                retried = true;
                source_inventory = scan_source_inventory(
                    source_dir,
                    &build_config.control,
                    worker_count,
                    build_pool,
                )?;
                validate_source_inventory(&source_inventory, constraints)?;
            }
            result => return result,
        }
    }
}

fn build_and_confirm_stable(
    source_dir: &Path,
    build_config: &BuildConfig<'_>,
    build_pool: &rayon::ThreadPool,
    source_inventory: &SourceInventory,
    worker_count: usize,
) -> Result<BuiltCapture> {
    let (result, timings) = build_from_inventory(source_inventory, build_config, build_pool)?;
    let fresh = scan_source_inventory(source_dir, &build_config.control, worker_count, build_pool)?;
    verify_source_inventory_stable(source_inventory, &fresh)?;
    Ok(BuiltCapture {
        result,
        timings,
        source_inventory: fresh,
    })
}

#[cfg(test)]
#[path = "stable_source_tests.rs"]
mod tests;
