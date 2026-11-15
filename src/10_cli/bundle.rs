//! `bundle inspect` command.

use crate::cli::output;
use crate::repo::artifacts::inspect_bundle;
use std::path::PathBuf;

pub(crate) fn inspect(bundle: PathBuf, repo: PathBuf) -> crate::Result<()> {
    let entries = inspect_bundle(&bundle, &repo)?;

    output::record(
        &serde_json::json!({"bundle": bundle, "chunks": entries.iter().map(|entry| serde_json::json!({"hash": entry.chunk_hash.to_hex(), "raw_size": entry.raw_size, "compressed_size": entry.compressed_size})).collect::<Vec<_>>()}),
    )?;
    output::line(format_args!("bundle: {}", bundle.display()));
    output::line(format_args!("repo: {}", repo.display()));
    output::line(format_args!("  chunks: {}", entries.len()));
    for entry in &entries {
        output::line(format_args!(
            "  - {} raw={} compressed={} ratio={:.1}%",
            &entry.chunk_hash.to_hex()[..16],
            entry.raw_size,
            entry.compressed_size,
            100.0 * entry.compressed_size as f64 / entry.raw_size as f64
        ));
    }
    Ok(())
}
