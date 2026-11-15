//! `bundle inspect` command.

use crate::cli::output;
use crate::cli::reporting::ratio_pct;
use crate::repo::artifacts::inspect_bundle;
use std::path::PathBuf;

pub(crate) fn inspect(bundle: PathBuf) -> crate::Result<()> {
    let entries = inspect_bundle(&bundle)?;
    let raw_size: u64 = entries.iter().map(|entry| u64::from(entry.raw_size)).sum();
    let compressed_size: u64 = entries
        .iter()
        .map(|entry| u64::from(entry.estimated_compressed_size))
        .sum();
    let chunks = entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "hash": entry.chunk_hash.to_hex(),
                "raw_size": entry.raw_size,
                "estimated_compressed_size": entry.estimated_compressed_size,
            })
        })
        .collect::<Vec<_>>();
    output::record(&serde_json::json!({
        "bundle": bundle,
        "raw_size": raw_size,
        "compressed_size": compressed_size,
        "chunks": chunks,
    }))?;

    output::line(format_args!("bundle: {}", bundle.display()));
    output::line(format_args!("  chunks: {}", entries.len()));
    if raw_size > 0 {
        output::line(format_args!(
            "  raw={raw_size} compressed={compressed_size} ratio={:.1}%",
            ratio_pct(compressed_size, raw_size)
        ));
    }
    for entry in &entries {
        output::line(format_args!(
            "  - {} raw={}",
            &entry.chunk_hash.to_hex()[..16],
            entry.raw_size
        ));
    }
    Ok(())
}
