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
