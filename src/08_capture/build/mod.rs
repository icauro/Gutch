//! Build staged artifacts and retry once when the source inventory changes.

pub(crate) mod pipeline;
pub(crate) mod progress;
pub(crate) mod types;

use crate::Result;
use crate::capture::scan::{SourceInventory, scan_source_inventory};
use crate::manifest::constraints::RepositoryConstraints;
use std::path::Path;
use types::{BuildConfig, BuildResult, BuildTimings};

pub(crate) struct BuiltCapture {
    pub(crate) result: BuildResult,
    pub(crate) timings: BuildTimings,
    pub(crate) source_inventory: SourceInventory,
}

pub(crate) fn build_with_stable_source(
    source_dir: &Path,
    build_config: &BuildConfig<'_>,
    build_pool: &rayon::ThreadPool,
    source_inventory: SourceInventory,
    constraints: &RepositoryConstraints,
    worker_count: usize,
) -> Result<BuiltCapture> {
    let (mut result, mut timings) = self::pipeline::build_with_config_timed_on_pool_with_inventory(
        source_dir,
        build_config,
        build_pool,
        &source_inventory,
    )?;
    // The source may have changed while its bytes were read. Compare fresh
    // observations against the pre-read inventory, retry once with fresh
    // observations, and fail without publishing anything on a second mismatch.
    let mut source_inventory = source_inventory;
    let stable_after_build =
        scan_source_inventory(source_dir, &build_config.control, worker_count, build_pool)
            .and_then(|fresh| {
                crate::capture::scan::verify_source_inventory_stable(&source_inventory, &fresh)?;
                Ok(fresh)
            });
    source_inventory = match stable_after_build {
        Ok(fresh) => fresh,
        Err(_) => {
            source_inventory =
                scan_source_inventory(source_dir, &build_config.control, worker_count, build_pool)?;
            crate::capture::scan::validate_source_inventory(&source_inventory, constraints)?;
            (result, timings) = self::pipeline::build_with_config_timed_on_pool_with_inventory(
                source_dir,
                build_config,
                build_pool,
                &source_inventory,
            )?;
            let fresh =
                scan_source_inventory(source_dir, &build_config.control, worker_count, build_pool)?;
            crate::capture::scan::verify_source_inventory_stable(&source_inventory, &fresh)?;
            fresh
        }
    };
    Ok(BuiltCapture {
        result,
        timings,
        source_inventory,
    })
}
