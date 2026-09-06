use std::path::PathBuf;

use gutch as g;

pub(crate) fn inspect(bundle: PathBuf, repo: PathBuf) -> g::Result<()> {
    let entries = g::api::inspect_bundle(&bundle, &repo)?;

    println!("bundle: {}", bundle.display());
    println!("repo: {}", repo.display());
    println!("  chunks: {}", entries.len());
    for entry in &entries {
        println!(
            "  - {} raw={} compressed={} ratio={:.1}%",
            &entry.chunk_hash[..16.min(entry.chunk_hash.len())],
            entry.raw_size,
            entry.compressed_size,
            100.0 * entry.compressed_size as f64 / entry.raw_size as f64
        );
    }
    Ok(())
}
