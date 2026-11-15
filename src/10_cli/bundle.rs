use std::path::PathBuf;

pub(crate) fn inspect(bundle: PathBuf, repo: PathBuf) -> crate::Result<()> {
    let entries = crate::repo::artifacts::inspect_bundle(&bundle, &repo)?;

    crate::cli::output::record(
        &serde_json::json!({"bundle": bundle, "chunks": entries.iter().map(|entry| serde_json::json!({"hash": entry.chunk_hash.to_hex(), "raw_size": entry.raw_size, "compressed_size": entry.compressed_size})).collect::<Vec<_>>()}),
    )?;
    crate::cli::output::line(format_args!("bundle: {}", bundle.display()));
    crate::cli::output::line(format_args!("repo: {}", repo.display()));
    crate::cli::output::line(format_args!("  chunks: {}", entries.len()));
    for entry in &entries {
        crate::cli::output::line(format_args!(
            "  - {} raw={} compressed={} ratio={:.1}%",
            &entry.chunk_hash.to_hex()[..16],
            entry.raw_size,
            entry.compressed_size,
            100.0 * entry.compressed_size as f64 / entry.raw_size as f64
        ));
    }
    Ok(())
}
