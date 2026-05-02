use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use gutch as g;

fn secs(d: std::time::Duration) -> f64 {
    d.as_secs_f64()
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn mib_per_s(bytes: u64, d: std::time::Duration) -> f64 {
    let secs = d.as_secs_f64();
    if secs <= f64::EPSILON {
        0.0
    } else {
        mib(bytes) / secs
    }
}

fn ratio_pct(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        (numerator as f64 * 100.0) / denominator as f64
    }
}

struct InputSizingStats {
    target_logical_size_bytes: u64,
    median_file_size_bytes: u64,
}

fn collect_input_sizing_stats(dir: &PathBuf) -> g::Result<InputSizingStats> {
    let mut sizes = Vec::new();
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry?;
        if entry.path().is_file() {
            sizes.push(entry.metadata()?.len());
        }
    }

    sizes.sort_unstable();
    let target_logical_size_bytes = sizes.iter().copied().sum();
    let median_file_size_bytes = if sizes.is_empty() {
        0
    } else {
        let mid = sizes.len() / 2;
        if sizes.len() % 2 == 1 {
            sizes[mid]
        } else {
            (sizes[mid - 1] + sizes[mid]) / 2
        }
    };

    Ok(InputSizingStats {
        target_logical_size_bytes,
        median_file_size_bytes,
    })
}

fn print_build_timings(t: &g::BuildTimings) {
    println!("timings:");
    println!(
        "  read files:        {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.scan_read),
        mib(t.scan_read_bytes),
        mib_per_s(t.scan_read_bytes, t.scan_read)
    );
    println!(
        "  file hashing:      {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.file_hash),
        mib(t.file_hash_bytes),
        mib_per_s(t.file_hash_bytes, t.file_hash)
    );
    println!(
        "  chunking:          {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.chunking),
        mib(t.chunking_bytes),
        mib_per_s(t.chunking_bytes, t.chunking)
    );
    println!(
        "  hash judgment:     {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.chunk_hash_judgment),
        mib(t.chunk_hash_judgment_bytes),
        mib_per_s(t.chunk_hash_judgment_bytes, t.chunk_hash_judgment)
    );
    println!("  metadata:          {:.2} s", secs(t.metadata));
    println!(
        "  dedupe lookup:     {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.dedupe_lookup),
        mib(t.dedupe_lookup_bytes),
        mib_per_s(t.dedupe_lookup_bytes, t.dedupe_lookup)
    );
    println!(
        "  compression:       {:.2} s  in {:.2} MiB  out {:.2} MiB  {:.2} MiB/s",
        secs(t.compression),
        mib(t.compression_input_bytes),
        mib(t.compression_output_bytes),
        mib_per_s(t.compression_input_bytes, t.compression)
    );
    println!(
        "  total dedupe ratio: {:.2}%  (out / in)",
        ratio_pct(t.total_compressed_bytes, t.dedupe_lookup_bytes)
    );
    println!(
        "  bundle build:      {:.2} s  out {:.2} MiB  {:.2} MiB/s",
        secs(t.bundle_build),
        mib(t.bundle_build_bytes),
        mib_per_s(t.bundle_build_bytes, t.bundle_build)
    );
    println!(
        "  target manifest finalize: {:.2} s",
        secs(t.target_manifest_finalize)
    );
    println!("  total:             {:.2} s", secs(t.total));
}

fn print_reconstruct_timings(t: &g::ReconstructTimings) {
    println!("timings:");
    println!(
        "  bundle read:       {:.2} s  in {:.2} MiB  out {:.2} MiB  {:.2} MiB/s",
        secs(t.bundle_read),
        mib(t.bundle_read_compressed_bytes),
        mib(t.bundle_read_decompressed_bytes),
        mib_per_s(t.bundle_read_decompressed_bytes, t.bundle_read)
    );
    println!(
        "  file rebuild:      {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.file_rebuild),
        mib(t.file_rebuild_bytes),
        mib_per_s(t.file_rebuild_bytes, t.file_rebuild)
    );
    println!(
        "  file verify:       {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.file_verify),
        mib(t.file_verify_bytes),
        mib_per_s(t.file_verify_bytes, t.file_verify)
    );
    println!(
        "  file write:        {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.file_write),
        mib(t.file_write_bytes),
        mib_per_s(t.file_write_bytes, t.file_write)
    );
    println!("  total:             {:.2} s", secs(t.total));
}

fn print_verify_timings(t: &g::VerifyTimings) {
    println!("timings:");
    println!("  stat:              {:.2} s", secs(t.stat));
    println!(
        "  file read:         {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.file_read),
        mib(t.file_read_bytes),
        mib_per_s(t.file_read_bytes, t.file_read)
    );
    println!(
        "  file hash:         {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.file_hash),
        mib(t.file_hash_bytes),
        mib_per_s(t.file_hash_bytes, t.file_hash)
    );
    println!(
        "  chunk hash:        {:.2} s  {:.2} MiB  {:.2} MiB/s",
        secs(t.chunk_hash),
        mib(t.chunk_hash_bytes),
        mib_per_s(t.chunk_hash_bytes, t.chunk_hash)
    );
    println!("  total:             {:.2} s", secs(t.total));
}

#[derive(Parser)]
#[command(name = "gutch", version, about = "Launcher artifact engine")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Build {
        #[arg(short, long)]
        dir: PathBuf,

        #[arg(short, long)]
        product: String,

        #[arg(short, long)]
        version: String,

        #[arg(short = 'P', long, default_value = "windows")]
        platform: String,

        #[arg(short = 'A', long, default_value = "x86_64")]
        architecture: String,

        #[arg(short, long, default_value = "base")]
        tier: String,

        #[arg(short = 'C', long = "chunk-profile", value_enum, default_value_t = g::manifest::ArtifactSizingPreset::Balanced)]
        chunk_profile: g::manifest::ArtifactSizingPreset,

        #[arg(short = 'B', long = "bundle-profile", value_enum, default_value_t = g::manifest::BundleSizingPreset::Balanced)]
        bundle_profile: g::manifest::BundleSizingPreset,

        #[arg(long = "compression-unit", value_parser = ["chunk", "bundle"], default_value = "chunk")]
        compression_unit: String,

        #[arg(short, long)]
        output: PathBuf,
    },

    Reconstruct {
        #[arg(short = 'm', long)]
        manifest: PathBuf,

        #[arg(short = 'b', long)]
        bundles: PathBuf,

        #[arg(short, long)]
        output: PathBuf,
    },

    Verify {
        #[arg(short = 'm', long)]
        manifest: PathBuf,

        #[arg(short, long)]
        dir: PathBuf,
    },

    BundleInfo {
        #[arg(short, long)]
        bundle: PathBuf,
    },

    Estimate {
        #[arg(short, long)]
        dir: PathBuf,

        #[arg(short, long)]
        product: String,

        #[arg(short, long)]
        version: String,
    },
}

fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli) {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> g::Result<()> {
    match cli.command {
        Commands::Build {
            dir,
            product,
            version,
            platform,
            architecture,
            tier,
            chunk_profile,
            bundle_profile,
            compression_unit,
            output,
        } => {
            let input_stats = collect_input_sizing_stats(&dir)?;
            let product_root = output.join("v1").join("products").join(&product);
            let target_root = product_root
                .join("targets")
                .join(&version)
                .join(&platform)
                .join(&architecture)
                .join(&tier);
            let bundles_root = product_root.join("bundles");

            std::fs::create_dir_all(&target_root)?;
            std::fs::create_dir_all(&bundles_root)?;

            let mut existing_bundle_ids = HashSet::new();
            let mut existing_chunk_locations = HashMap::new();
            let mut previous_file_manifests = HashMap::new();
            for entry in walkdir::WalkDir::new(&bundles_root).sort_by_file_name() {
                let entry = entry?;
                if !entry.path().is_file() {
                    continue;
                }
                if entry.path().extension().map_or(false, |e| e == "bundle") {
                    if let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str()) {
                        let bundle_id = stem.to_string();
                        existing_bundle_ids.insert(bundle_id.clone());

                        let file = std::fs::File::open(entry.path())?;
                        let reader = g::bundle::BundleReader::open(file)?;
                        for chunk in reader.entries() {
                            existing_chunk_locations
                                .entry(chunk.chunk_hash.clone())
                                .or_insert_with(|| g::ExistingChunkLocation {
                                    bundle_id: bundle_id.clone(),
                                    raw_size: chunk.raw_size,
                                    compressed_size: chunk.compressed_size,
                                });
                        }
                    }
                }
            }
            let targets_root = product_root.join("targets");
            if targets_root.exists() {
                for entry in walkdir::WalkDir::new(&targets_root).sort_by_file_name() {
                    let entry = entry?;
                    if !entry.path().is_file() || entry.file_name() != "manifest.json" {
                        continue;
                    }

                    let manifest_json = std::fs::read_to_string(entry.path())?;
                    let manifest: g::manifest::TargetManifest =
                        serde_json::from_str(&manifest_json)?;
                    for file in manifest.files {
                        previous_file_manifests
                            .entry(file.path.clone())
                            .or_insert(file);
                    }
                }
            }

            let chunk_sizing_profile = g::manifest::ChunkSizingProfile::for_selection(
                chunk_profile,
                input_stats.median_file_size_bytes,
            );
            let bundle_sizing_profile = g::manifest::BundleSizingProfile::for_selection(
                bundle_profile,
                input_stats.target_logical_size_bytes,
            );
            let compression_profile = g::manifest::CompressionProfile {
                compression: g::manifest::COMPRESSION_ZSTD.to_string(),
                compression_unit,
                zstd_level: 3,
            };

            let (result, timings) = g::build_from_directory_with_timings_against_existing_store_and_manifests_and_profiles(
                &dir,
                &product,
                &version,
                &platform,
                &architecture,
                &tier,
                &existing_chunk_locations,
                &existing_bundle_ids,
                &previous_file_manifests,
                &chunk_sizing_profile,
                &bundle_sizing_profile,
                &compression_profile,
            )?;

            let manifest_path = target_root.join("manifest.json");
            let manifest_json = serde_json::to_string_pretty(&result.target_manifest)?;
            std::fs::write(&manifest_path, &manifest_json)?;
            println!("wrote target manifest: {}", manifest_path.display());

            for bundle in &result.bundles {
                let prefix = g::bundle::bundle_id_prefix(&bundle.bundle_id);
                let bundle_dir = bundles_root.join(prefix);
                std::fs::create_dir_all(&bundle_dir)?;
                let bundle_path = bundle_dir.join(format!("{}.bundle", bundle.bundle_id));
                std::fs::write(&bundle_path, &bundle.data)?;
                println!(
                    "wrote bundle: {} ({} bytes)",
                    bundle_path.display(),
                    bundle.data.len()
                );
            }

            println!(
                "done. {} files in manifest, total logical: {} bytes",
                result.target_manifest.files.len(),
                result.target_manifest.total_logical_size
            );

            if let Ok(est) = g::estimate_build(&result.target_manifest, &Default::default(), 0) {
                println!(
                    "estimate: {} chunks missing, {:.1} MiB logical",
                    est.missing_chunk_count,
                    est.missing_logical_bytes as f64 / (1024.0 * 1024.0)
                );
            }

            print_build_timings(&timings);
        }

        Commands::Reconstruct {
            manifest,
            bundles,
            output,
        } => {
            let manifest_json = std::fs::read_to_string(&manifest)?;
            let tm: g::manifest::TargetManifest = serde_json::from_str(&manifest_json)?;

            let bundle_paths: Vec<PathBuf> = if bundles.is_dir() {
                let mut paths = Vec::new();
                let required_bundle_ids: HashSet<&str> =
                    tm.required_bundle_set.iter().map(String::as_str).collect();
                for entry in walkdir::WalkDir::new(&bundles).sort_by_file_name() {
                    let entry = entry?;
                    if !entry.path().is_file() {
                        continue;
                    }
                    if entry.path().extension().map_or(false, |e| e == "bundle") {
                        if !required_bundle_ids.is_empty() {
                            let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str())
                            else {
                                continue;
                            };
                            if !required_bundle_ids.contains(stem) {
                                continue;
                            }
                        }
                        paths.push(entry.path().to_path_buf());
                    }
                }
                paths
            } else if bundles.is_file() {
                vec![bundles]
            } else {
                Vec::new()
            };

            let (_, timings) =
                g::reconstruct_from_bundle_paths_with_timings(&tm, &bundle_paths, &output)?;
            println!("reconstructed to: {}", output.display());
            print_reconstruct_timings(&timings);
        }

        Commands::Verify { manifest, dir } => {
            let manifest_json = std::fs::read_to_string(&manifest)?;
            let tm: g::manifest::TargetManifest = serde_json::from_str(&manifest_json)?;

            let (errors, timings) = g::verify_directory_with_timings(&tm, &dir)?;

            if errors.is_empty() {
                println!("verification passed for {}", dir.display());
                print_verify_timings(&timings);
            } else {
                eprintln!("{} verification errors:", errors.len());
                for err in &errors {
                    eprintln!("  - {}", err);
                }
                std::process::exit(1);
            }
        }

        Commands::BundleInfo { bundle } => {
            let data = std::fs::read(&bundle)?;
            let reader = g::bundle::read_bundle(&data)?;

            println!("bundle: {}", bundle.display());
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
        }

        Commands::Estimate {
            dir,
            product,
            version,
        } => {
            let result =
                g::build_from_directory(&dir, &product, &version, "windows", "x86_64", "base")?;

            let est = g::estimate_build(
                &result.target_manifest,
                &Default::default(),
                1024 * 1024 * 1024,
            )?;

            println!("fresh install estimate:");
            println!("  files: {}", result.target_manifest.files.len());
            println!(
                "  logical size:  {:.2} MiB",
                result.target_manifest.total_logical_size as f64 / (1024.0 * 1024.0)
            );
            println!("  missing chunks: {}", est.missing_chunk_count);
            println!(
                "  missing logical: {:.2} MiB",
                est.missing_logical_bytes as f64 / (1024.0 * 1024.0)
            );
            println!(
                "  missing compressed: {:.2} MiB",
                est.missing_compressed_bytes as f64 / (1024.0 * 1024.0)
            );
            println!("  estimated bundles: {}", est.required_bundle_count);
            println!("  fragmentation score: {:.1}", est.fragmentation_score);
        }
    }

    Ok(())
}
