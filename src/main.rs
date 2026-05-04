use std::collections::{HashMap, HashSet};
use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use gutch as g;
use serde::Deserialize;

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

fn accumulate_build_timings(total: &mut g::BuildTimings, add: &g::BuildTimings) {
    total.scan_read += add.scan_read;
    total.scan_read_bytes += add.scan_read_bytes;
    total.file_hash += add.file_hash;
    total.file_hash_bytes += add.file_hash_bytes;
    total.chunking += add.chunking;
    total.chunking_bytes += add.chunking_bytes;
    total.chunk_hash_judgment += add.chunk_hash_judgment;
    total.chunk_hash_judgment_bytes += add.chunk_hash_judgment_bytes;
    total.metadata += add.metadata;
    total.dedupe_lookup += add.dedupe_lookup;
    total.dedupe_lookup_bytes += add.dedupe_lookup_bytes;
    total.compression += add.compression;
    total.compression_input_bytes += add.compression_input_bytes;
    total.compression_output_bytes += add.compression_output_bytes;
    total.total_compressed_bytes += add.total_compressed_bytes;
    total.bundle_build += add.bundle_build;
    total.bundle_build_bytes += add.bundle_build_bytes;
    total.target_manifest_finalize += add.target_manifest_finalize;
    total.total += add.total;
}

#[derive(Parser)]
#[command(name = "gutch", version, about = "Launcher artifact engine")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Deserialize)]
struct RepoInitSpec {
    targets: Vec<RepoInitTargetSpec>,
}

#[derive(Debug, Deserialize)]
struct RepoInitTargetSpec {
    dir: PathBuf,
    #[serde(default)]
    tags: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RepoInitMode {
    Batch,
    Sequential,
}

fn resolve_target_input(spec: &RepoInitTargetSpec) -> g::Result<g::RepoInitTargetInput> {
    let version_id = spec
        .tags
        .get("version")
        .cloned()
        .ok_or_else(|| g::Error::manifest("repo init target is missing required tag 'version'"))?;
    let platform = spec
        .tags
        .get("platform")
        .cloned()
        .unwrap_or_else(|| "any".to_string());
    let architecture = spec
        .tags
        .get("arch")
        .cloned()
        .unwrap_or_else(|| "any".to_string());
    let tier_id = spec
        .tags
        .get("tier")
        .cloned()
        .unwrap_or_else(|| "base".to_string());

    let mut tags = spec.tags.clone();
    tags.entry("platform".to_string()).or_insert_with(|| platform.clone());
    tags.entry("arch".to_string()).or_insert_with(|| architecture.clone());
    tags.entry("tier".to_string()).or_insert_with(|| tier_id.clone());

    Ok(g::RepoInitTargetInput {
        dir: spec.dir.clone(),
        version_id,
        platform,
        architecture,
        tier_id,
        tags,
    })
}

#[derive(Subcommand)]
enum Commands {
    RepoInit {
        #[arg(short, long)]
        product: String,

        #[arg(long = "content-repo")]
        content_repo: String,

        #[arg(short, long)]
        spec: PathBuf,

        #[arg(long = "init-mode", value_enum, default_value_t = RepoInitMode::Batch)]
        init_mode: RepoInitMode,

        #[arg(short = 'C', long = "chunk-profile", value_enum, default_value_t = g::manifest::ArtifactSizingPreset::Balanced)]
        chunk_profile: g::manifest::ArtifactSizingPreset,

        #[arg(short = 'B', long = "bundle-profile", value_enum, default_value_t = g::manifest::BundleSizingPreset::Balanced)]
        bundle_profile: g::manifest::BundleSizingPreset,

        #[arg(long = "compression-unit", value_parser = ["chunk", "bundle"], default_value = "chunk")]
        compression_unit: String,

        #[arg(short, long)]
        output: PathBuf,
    },

    Build {
        #[arg(short, long)]
        dir: PathBuf,

        #[arg(short, long)]
        product: String,

        #[arg(long = "content-repo", default_value = "base")]
        content_repo: String,

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
        Commands::RepoInit {
            product,
            content_repo,
            spec,
            init_mode,
            chunk_profile,
            bundle_profile,
            compression_unit,
            output,
        } => {
            let spec_json = std::fs::read_to_string(&spec)?;
            let spec: RepoInitSpec = serde_json::from_str(&spec_json)?;
            if spec.targets.is_empty() {
                return Err(g::Error::manifest("repo init spec must include at least one target"));
            }

            let target_inputs: Vec<g::RepoInitTargetInput> = spec
                .targets
                .iter()
                .map(resolve_target_input)
                .collect::<g::Result<Vec<_>>>()?;

            let repo_root = output
                .join("v1")
                .join("products")
                .join(&product)
                .join("repos")
                .join(&content_repo);
            let targets_root = repo_root.join("targets");
            let bundles_root = repo_root.join("bundles");

            if targets_root.exists() || bundles_root.exists() {
                return Err(g::Error::manifest(format!(
                    "content repo already exists at {}",
                    repo_root.display()
                )));
            }

            std::fs::create_dir_all(&targets_root)?;
            std::fs::create_dir_all(&bundles_root)?;
            std::fs::create_dir_all(repo_root.join("metadata"))?;

            let mut all_file_sizes = Vec::new();
            let mut all_target_logical_size = 0u64;
            for target in &target_inputs {
                let stats = collect_input_sizing_stats(&target.dir)?;
                all_target_logical_size += stats.target_logical_size_bytes;
                if stats.median_file_size_bytes > 0 {
                    all_file_sizes.push(stats.median_file_size_bytes);
                }
            }
            all_file_sizes.sort_unstable();
            let batch_median_file_size = if all_file_sizes.is_empty() {
                0
            } else {
                let mid = all_file_sizes.len() / 2;
                if all_file_sizes.len() % 2 == 1 {
                    all_file_sizes[mid]
                } else {
                    (all_file_sizes[mid - 1] + all_file_sizes[mid]) / 2
                }
            };

            let chunk_sizing_profile = g::manifest::ChunkSizingProfile::for_selection(
                chunk_profile,
                batch_median_file_size,
            );
            let bundle_sizing_profile = g::manifest::BundleSizingProfile::for_selection(
                bundle_profile,
                all_target_logical_size,
            );
            let compression_profile = g::manifest::CompressionProfile {
                compression: g::manifest::COMPRESSION_ZSTD.to_string(),
                compression_unit,
                zstd_level: 3,
            };

            match init_mode {
                RepoInitMode::Batch => {
                    let (result, timings) = g::build_repo_init_batch_with_timings(
                        &product,
                        &content_repo,
                        &target_inputs,
                        &chunk_sizing_profile,
                        &bundle_sizing_profile,
                        &compression_profile,
                    )?;

                    for manifest in &result.target_manifests {
                        let target_root = targets_root.join(&manifest.target_id);
                        std::fs::create_dir_all(&target_root)?;
                        let manifest_path = target_root.join("manifest.json");
                        let manifest_json = serde_json::to_string_pretty(manifest)?;
                        std::fs::write(&manifest_path, &manifest_json)?;
                        println!("wrote target manifest: {}", manifest_path.display());
                    }

                    for bundle in &result.bundles {
                        let bundle_path = bundles_root.join(format!("{}.bundle", bundle.bundle_id));
                        std::fs::write(&bundle_path, &bundle.data)?;
                        println!("wrote bundle: {} ({} bytes)", bundle_path.display(), bundle.data.len());
                    }

                    println!(
                        "initialized content repo {} with {} targets using batch mode",
                        content_repo,
                        result.target_manifests.len()
                    );
                    print_build_timings(&timings);
                }
                RepoInitMode::Sequential => {
                    let mut existing_bundle_ids = HashSet::new();
                    let mut existing_chunk_locations = HashMap::new();
                    let mut previous_file_manifests = HashMap::new();
                    let mut combined_timings = g::BuildTimings::default();

                    for target in &target_inputs {
                        let (result, timings) =
                            g::build_from_directory_with_timings_against_existing_store_and_manifests_and_profiles(
                                &target.dir,
                                &product,
                                &content_repo,
                                &target.version_id,
                                &target.platform,
                                &target.architecture,
                                &target.tier_id,
                                &existing_chunk_locations,
                                &existing_bundle_ids,
                                &previous_file_manifests,
                                &chunk_sizing_profile,
                                &bundle_sizing_profile,
                                &compression_profile,
                            )?;

                        let target_root = targets_root.join(&result.target_manifest.target_id);
                        std::fs::create_dir_all(&target_root)?;
                        let manifest_path = target_root.join("manifest.json");
                        let manifest_json = serde_json::to_string_pretty(&result.target_manifest)?;
                        std::fs::write(&manifest_path, &manifest_json)?;
                        println!("wrote target manifest: {}", manifest_path.display());

                        for file in &result.target_manifest.files {
                            previous_file_manifests
                                .entry(file.path.clone())
                                .or_insert_with(|| file.clone());
                        }

                        for bundle in &result.bundles {
                            let bundle_path = bundles_root.join(format!("{}.bundle", bundle.bundle_id));
                            std::fs::write(&bundle_path, &bundle.data)?;
                            println!("wrote bundle: {} ({} bytes)", bundle_path.display(), bundle.data.len());
                            existing_bundle_ids.insert(bundle.bundle_id.clone());

                            let reader = g::bundle::read_bundle(&bundle.data)?;
                            for entry in reader.entries() {
                                existing_chunk_locations
                                    .entry(entry.chunk_hash.clone())
                                    .or_insert_with(|| g::ExistingChunkLocation {
                                        bundle_id: bundle.bundle_id.clone(),
                                        raw_size: entry.raw_size,
                                        compressed_size: entry.compressed_size,
                                    });
                            }
                        }

                        accumulate_build_timings(&mut combined_timings, &timings);
                    }

                    println!(
                        "initialized content repo {} with {} targets using sequential mode",
                        content_repo,
                        target_inputs.len()
                    );
                    print_build_timings(&combined_timings);
                }
            }
        }

        Commands::Build {
            dir,
            product,
            content_repo,
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
            let target_id = g::compute_target_id(&version, &platform, &architecture, &tier);
            let repo_root = output
                .join("v1")
                .join("products")
                .join(&product)
                .join("repos")
                .join(&content_repo);
            let target_root = repo_root.join("targets").join(&target_id);
            let bundles_root = repo_root.join("bundles");

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
            let targets_root = repo_root.join("targets");
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
                &content_repo,
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
                let bundle_path = bundles_root.join(format!("{}.bundle", bundle.bundle_id));
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
                g::build_from_directory(&dir, &product, "base", &version, "windows", "x86_64", "base")?;

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
