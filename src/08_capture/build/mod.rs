//! Build staged artifacts and retry once when the source inventory changes.

pub(crate) mod pipeline;
pub(crate) mod progress;
pub(crate) mod types;

use crate::Error;
use crate::Result;
use crate::capture::scan::validate_source_inventory;
use crate::capture::scan::verify_source_inventory_stable;
use crate::capture::scan::{SourceInventory, scan_source_inventory};
use crate::manifest::constraints::RepositoryConstraints;
use pipeline::build_from_inventory;
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
    mut source_inventory: SourceInventory,
    constraints: &RepositoryConstraints,
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
mod tests {
    use super::*;
    use crate::manifest::SnapshotTagSet;

    #[test]
    fn a_source_change_seen_during_the_build_is_retried_from_a_fresh_scan() {
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("file"), b"before").unwrap();
        let tags = SnapshotTagSet::default();
        let config = BuildConfig::for_snapshot(&tags);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let stale = scan_source_inventory(source.path(), &config.control, 1, &pool).unwrap();
        // The build reads a different size than the inventory recorded.
        std::fs::write(source.path().join("file"), b"after the edit").unwrap();

        let built = build_with_stable_source(
            source.path(),
            &config,
            &pool,
            stale,
            &RepositoryConstraints::default(),
            1,
        )
        .unwrap();
        assert_eq!(
            built.result.snapshot_manifest.total_logical_size,
            b"after the edit".len() as u64
        );
    }
}
