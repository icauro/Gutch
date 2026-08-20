use std::path::PathBuf;

use gutch as g;

pub(crate) fn inspect(bundle: PathBuf, repo: PathBuf) -> g::Result<()> {
    let data = crate::helpers::read(&bundle)?;
    let (_, repo_manifest) = g::load_gutch_repo_manifest(&repo)?;
    let reader = g::bundle::read_bundle(&data, &repo_manifest.compression_profile())?;

    println!("bundle: {}", bundle.display());
    println!("repo: {}", repo.display());
    println!("  chunks: {}", reader.chunk_count());
    for entry in reader.entries() {
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
