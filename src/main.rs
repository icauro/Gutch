mod cli_identity;
mod cli_path_compare;
mod cli_reporting;

use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use cli_identity::{
    apply_resolved_target_identity, effective_target_schema, parse_cli_tags,
    resolve_target_identity, resolve_target_input, validate_target_schema,
};
use cli_path_compare::compare_paths;
use cli_reporting::{
    BuildTimingsSummary, OptionalOutputPaths, ReconstructTimingsSummary, VerifyTimingsSummary,
    accumulate_build_timings, build_timings_summary, collect_repo_init_target_added_bytes,
    ensure_parent_directory, format_bytes, optional_output_paths, print_build_timings,
    print_reconstruct_timings, print_repo_init_storage_summary, print_verify_timings, ratio_pct,
    reconstruct_timings_summary, verify_timings_summary, write_stats_json_output,
};
use gutch as g;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

struct InputSizingStats {
    target_logical_size_bytes: u64,
    median_file_size_bytes: u64,
}

struct PreparedInputPath {
    dir: PathBuf,
    _tempdir: Option<TempDir>,
}

#[derive(Debug, Serialize)]
struct TargetInsertStats {
    command: &'static str,
    repo_root: String,
    input_path: String,
    target_id: String,
    manifest_path: String,
    product_id: String,
    content_repo_id: String,
    version_id: String,
    platform: String,
    architecture: String,
    tier_id: String,
    file_count: usize,
    total_logical_size: u64,
    total_compressed_size_estimate: u64,
    required_chunk_count: usize,
    required_bundle_count: usize,
    generated_bundle_count: usize,
    generated_bundle_bytes: u64,
    reused_chunk_count: usize,
    new_chunk_count: usize,
    reused_chunk_percent: f64,
    reused_bundle_count: usize,
    reused_bundle_percent: f64,
    timings: BuildTimingsSummary,
    outputs: OptionalOutputPaths,
}

#[derive(Debug, Serialize)]
struct TargetExportStats {
    command: &'static str,
    repo_root: String,
    target_id: String,
    output_dir: String,
    file_count: usize,
    required_chunk_count: usize,
    required_bundle_count: usize,
    resolved_bundle_file_count: usize,
    total_logical_size: u64,
    total_compressed_size_estimate: u64,
    timings: ReconstructTimingsSummary,
    outputs: OptionalOutputPaths,
}

#[derive(Debug, Serialize)]
struct TargetInstallVerifyStats {
    command: &'static str,
    repo_root: String,
    target_id: String,
    install_dir: String,
    expected_file_count: usize,
    missing_file_count: usize,
    corrupt_file_count: usize,
    extra_file_count: usize,
    verification_passed: bool,
    errors: Vec<String>,
    timings: VerifyTimingsSummary,
    outputs: OptionalOutputPaths,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_repo_init_target_added_bytes_counts_only_new_bundles_once() {
        let target_a = g::manifest::TargetManifest {
            target_id: "1.0.0-windows-x86_64-base".to_string(),
            total_logical_size: 1000,
            required_bundle_set: vec!["bundle-a".to_string(), "bundle-b".to_string()],
            ..g::manifest::TargetManifest::new(
                "prod".into(),
                "base".into(),
                "1.0.0-windows-x86_64-base".into(),
                "1.0.0".into(),
                "windows".into(),
                "x86_64".into(),
                "base".into(),
                BTreeMap::new(),
            )
        };
        let target_b = g::manifest::TargetManifest {
            target_id: "1.1.0-windows-x86_64-base".to_string(),
            total_logical_size: 2000,
            required_bundle_set: vec!["bundle-b".to_string(), "bundle-c".to_string()],
            ..g::manifest::TargetManifest::new(
                "prod".into(),
                "base".into(),
                "1.1.0-windows-x86_64-base".into(),
                "1.1.0".into(),
                "windows".into(),
                "x86_64".into(),
                "base".into(),
                BTreeMap::new(),
            )
        };

        let target_manifest_sizes = HashMap::from([
            (target_a.target_id.clone(), 100u64),
            (target_b.target_id.clone(), 200u64),
        ]);
        let bundle_sizes = HashMap::from([
            ("bundle-a".to_string(), 1000u64),
            ("bundle-b".to_string(), 2000u64),
            ("bundle-c".to_string(), 3000u64),
        ]);

        let added_bytes = collect_repo_init_target_added_bytes(
            &[&target_a, &target_b],
            &target_manifest_sizes,
            &bundle_sizes,
        );

        assert_eq!(added_bytes[0].0.target_id, target_a.target_id);
        assert_eq!(added_bytes[0].1, 3100);
        assert_eq!(added_bytes[1].0.target_id, target_b.target_id);
        assert_eq!(added_bytes[1].1, 3200);
    }
}

fn write_optional_summary(summary_out: &Option<PathBuf>, lines: &[String]) -> g::Result<()> {
    let Some(summary_out) = summary_out else {
        return Ok(());
    };

    ensure_parent_directory(summary_out)?;
    std::fs::write(summary_out, lines.join("\n"))?;
    Ok(())
}

fn write_optional_stats_json<T: Serialize>(
    stats_json_out: &Option<PathBuf>,
    stats: &T,
) -> g::Result<()> {
    write_stats_json_output(stats_json_out, stats)
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

fn prepare_input_path(path: &PathBuf) -> g::Result<PreparedInputPath> {
    if path.is_dir() {
        return Ok(PreparedInputPath {
            dir: path.clone(),
            _tempdir: None,
        });
    }

    if path.is_file() {
        let tempdir = tempfile::Builder::new().prefix("gutch_input_").tempdir()?;
        let filename = path.file_name().ok_or_else(|| {
            g::Error::manifest(format!("input file has no name: {}", path.display()))
        })?;
        let staged_path = tempdir.path().join(filename);
        std::fs::copy(path, &staged_path)?;
        return Ok(PreparedInputPath {
            dir: tempdir.path().to_path_buf(),
            _tempdir: Some(tempdir),
        });
    }

    Err(g::Error::manifest(format!(
        "input path does not exist or is unsupported: {}",
        path.display()
    )))
}

fn directory_has_any_entries(path: &std::path::Path) -> g::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    Ok(std::fs::read_dir(path)?.next().is_some())
}

fn load_target_manifest_from_repo(
    repo_root: &std::path::Path,
    target_id: &str,
) -> g::Result<(
    g::ContentRepoPaths,
    g::manifest::ContentRepoManifest,
    PathBuf,
    g::manifest::TargetManifest,
)> {
    let (repo_paths, repo_manifest) = g::load_content_repo_manifest(repo_root)?;
    let manifest_path = g::target_manifest_path(&repo_paths, target_id);
    if !manifest_path.exists() {
        return Err(g::Error::manifest(format!(
            "target manifest not found at {}",
            manifest_path.display()
        )));
    }

    let manifest_json = std::fs::read_to_string(&manifest_path)?;
    let target_manifest: g::manifest::TargetManifest = serde_json::from_str(&manifest_json)?;
    Ok((repo_paths, repo_manifest, manifest_path, target_manifest))
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
    #[serde(default)]
    target_schema: Option<g::manifest::ContentRepoTargetSchema>,
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

#[derive(Subcommand)]
enum Commands {
    Repo {
        #[command(subcommand)]
        command: RepoCommands,
    },

    Target {
        #[command(subcommand)]
        command: TargetCommands,
    },

    #[command(name = "target_install")]
    TargetInstall {
        #[command(subcommand)]
        command: TargetInstallCommands,
    },

    Bundle {
        #[command(subcommand)]
        command: BundleCommands,
    },

    Path {
        #[command(subcommand)]
        command: PathCommands,
    },
}

#[derive(Subcommand)]
enum RepoCommands {
    Init {
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

        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,
    },
}

#[derive(Subcommand)]
enum TargetCommands {
    Insert {
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,

        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,

        #[arg(short, long)]
        version: Option<String>,

        #[arg(short = 'P', long, visible_alias = "os")]
        platform: Option<String>,

        #[arg(short = 'A', long, visible_alias = "arch")]
        architecture: Option<String>,

        #[arg(short, long)]
        tier: Option<String>,

        #[arg(long = "tag")]
        tags: Vec<String>,

        #[arg(short = 'C', long = "chunk-profile", value_enum, default_value_t = g::manifest::ArtifactSizingPreset::Balanced)]
        chunk_profile: g::manifest::ArtifactSizingPreset,

        #[arg(short = 'B', long = "bundle-profile", value_enum, default_value_t = g::manifest::BundleSizingPreset::Balanced)]
        bundle_profile: g::manifest::BundleSizingPreset,

        #[arg(long = "compression-unit", value_parser = ["chunk", "bundle"], default_value = "chunk")]
        compression_unit: String,

        #[arg(long = "summary-out")]
        summary_out: Option<PathBuf>,

        #[arg(long = "stats-json-out")]
        stats_json_out: Option<PathBuf>,
    },

    Export {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,

        #[arg(long = "target")]
        target: String,

        #[arg(short, long)]
        output: PathBuf,

        #[arg(long = "summary-out")]
        summary_out: Option<PathBuf>,

        #[arg(long = "stats-json-out")]
        stats_json_out: Option<PathBuf>,
    },

    Estimate {
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,

        #[arg(short, long)]
        product: String,

        #[arg(short, long)]
        version: String,
    },
}

#[derive(Subcommand)]
enum TargetInstallCommands {
    Verify {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,

        #[arg(long = "target")]
        target: String,

        #[arg(short, long)]
        dir: PathBuf,

        #[arg(long = "summary-out")]
        summary_out: Option<PathBuf>,

        #[arg(long = "stats-json-out")]
        stats_json_out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum BundleCommands {
    Inspect {
        #[arg(short, long)]
        bundle: PathBuf,
    },
}

#[derive(Subcommand)]
enum PathCommands {
    Compare {
        #[arg(long = "expected")]
        expected: PathBuf,

        #[arg(long = "actual")]
        actual: PathBuf,

        #[arg(long = "summary-out")]
        summary_out: Option<PathBuf>,

        #[arg(long = "stats-json-out")]
        stats_json_out: Option<PathBuf>,
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
        Commands::Repo {
            command:
                RepoCommands::Init {
                    product,
                    content_repo,
                    spec,
                    init_mode,
                    chunk_profile,
                    bundle_profile,
                    compression_unit,
                    repo,
                },
        } => {
            let spec_json = std::fs::read_to_string(&spec)?;
            let spec: RepoInitSpec = serde_json::from_str(&spec_json)?;
            if spec.targets.is_empty() {
                return Err(g::Error::manifest(
                    "repo init spec must include at least one target",
                ));
            }

            let target_schema = effective_target_schema(spec.target_schema.clone());
            validate_target_schema(&target_schema)?;

            let target_inputs: Vec<g::RepoInitTargetInput> = spec
                .targets
                .iter()
                .map(|target| resolve_target_input(target, &target_schema))
                .collect::<g::Result<Vec<_>>>()?;

            let repo_paths = g::ContentRepoPaths::from_repo_root(&repo);

            if directory_has_any_entries(&repo_paths.targets_root)?
                || directory_has_any_entries(&repo_paths.bundles_root)?
            {
                return Err(g::Error::manifest(format!(
                    "content repo already exists at {}",
                    repo_paths.repo_root.display()
                )));
            }

            let mut repo_manifest =
                g::ensure_content_repo_layout(&repo_paths, &product, &content_repo)?;
            repo_manifest.target_schema = Some(target_schema.clone());
            std::fs::write(
                &repo_paths.repo_manifest_path,
                serde_json::to_string_pretty(&repo_manifest)?,
            )?;

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
                    let (mut result, timings) = g::build_repo_init_batch_with_timings(
                        &product,
                        &content_repo,
                        &target_inputs,
                        &chunk_sizing_profile,
                        &bundle_sizing_profile,
                        &compression_profile,
                    )?;
                    let mut target_manifest_sizes = HashMap::new();
                    let mut bundle_sizes = HashMap::new();

                    for (manifest, target_input) in
                        result.target_manifests.iter_mut().zip(target_inputs.iter())
                    {
                        let resolved_identity = resolve_target_identity(
                            &target_input.dir,
                            &target_input.tags,
                            &target_schema,
                            "repo init target",
                        )?;
                        apply_resolved_target_identity(manifest, &resolved_identity);
                        let target_root = repo_paths.targets_root.join(&manifest.target_id);
                        std::fs::create_dir_all(&target_root)?;
                        let manifest_path = target_root.join("manifest.json");
                        let manifest_json = serde_json::to_string_pretty(&*manifest)?;
                        std::fs::write(&manifest_path, &manifest_json)?;
                        target_manifest_sizes.insert(
                            manifest.target_id.clone(),
                            std::fs::metadata(&manifest_path)?.len(),
                        );
                        println!("wrote target manifest: {}", manifest_path.display());
                    }

                    for bundle in &result.bundles {
                        let bundle_path = repo_paths
                            .bundles_root
                            .join(format!("{}.bundle", bundle.bundle_id));
                        std::fs::write(&bundle_path, &bundle.data)?;
                        bundle_sizes.insert(bundle.bundle_id.clone(), bundle.data.len() as u64);
                        println!(
                            "wrote bundle: {} ({} bytes)",
                            bundle_path.display(),
                            bundle.data.len()
                        );
                    }

                    println!(
                        "initialized content repo {} with {} targets using batch mode",
                        content_repo,
                        result.target_manifests.len()
                    );
                    let ordered_target_manifests: Vec<&g::manifest::TargetManifest> =
                        result.target_manifests.iter().collect();
                    print_repo_init_storage_summary(
                        &ordered_target_manifests,
                        &repo_paths.repo_root,
                        &target_manifest_sizes,
                        &bundle_sizes,
                    )?;
                    print_build_timings(&timings);
                }
                RepoInitMode::Sequential => {
                    let mut existing_bundle_ids = HashSet::new();
                    let mut existing_chunk_locations = HashMap::new();
                    let mut previous_file_manifests = HashMap::new();
                    let mut combined_timings = g::BuildTimings::default();
                    let mut ordered_target_manifests = Vec::new();
                    let mut target_manifest_sizes = HashMap::new();
                    let mut bundle_sizes = HashMap::new();

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

                        let resolved_identity = resolve_target_identity(
                            &target.dir,
                            &target.tags,
                            &target_schema,
                            "repo init target",
                        )?;
                        let mut target_manifest = result.target_manifest.clone();
                        apply_resolved_target_identity(&mut target_manifest, &resolved_identity);
                        let target_root = repo_paths.targets_root.join(&target_manifest.target_id);
                        std::fs::create_dir_all(&target_root)?;
                        let manifest_path = target_root.join("manifest.json");
                        let manifest_json = serde_json::to_string_pretty(&target_manifest)?;
                        std::fs::write(&manifest_path, &manifest_json)?;
                        target_manifest_sizes.insert(
                            target_manifest.target_id.clone(),
                            std::fs::metadata(&manifest_path)?.len(),
                        );
                        println!("wrote target manifest: {}", manifest_path.display());
                        ordered_target_manifests.push(target_manifest.clone());

                        for file in &target_manifest.files {
                            previous_file_manifests
                                .entry(file.path.clone())
                                .or_insert_with(|| file.clone());
                        }

                        for bundle in &result.bundles {
                            let bundle_path = repo_paths
                                .bundles_root
                                .join(format!("{}.bundle", bundle.bundle_id));
                            std::fs::write(&bundle_path, &bundle.data)?;
                            bundle_sizes.insert(bundle.bundle_id.clone(), bundle.data.len() as u64);
                            println!(
                                "wrote bundle: {} ({} bytes)",
                                bundle_path.display(),
                                bundle.data.len()
                            );
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
                    let ordered_target_manifest_refs: Vec<&g::manifest::TargetManifest> =
                        ordered_target_manifests.iter().collect();
                    print_repo_init_storage_summary(
                        &ordered_target_manifest_refs,
                        &repo_paths.repo_root,
                        &target_manifest_sizes,
                        &bundle_sizes,
                    )?;
                    print_build_timings(&combined_timings);
                }
            }
        }

        Commands::Target {
            command:
                TargetCommands::Insert {
                    dir,
                    repo,
                    version,
                    platform,
                    architecture,
                    tier,
                    tags,
                    chunk_profile,
                    bundle_profile,
                    compression_unit,
                    summary_out,
                    stats_json_out,
                },
        } => {
            let prepared_input = prepare_input_path(&dir)?;
            let (repo_paths, repo_manifest) = g::load_content_repo_manifest(&repo)?;
            let target_schema = effective_target_schema(repo_manifest.target_schema.clone());
            let mut explicit_tags = parse_cli_tags(&tags)?;
            if let Some(version) = version {
                explicit_tags.insert("version".to_string(), version);
            }
            if let Some(platform) = platform {
                explicit_tags.insert("platform".to_string(), platform);
            }
            if let Some(architecture) = architecture {
                explicit_tags.insert("arch".to_string(), architecture);
            }
            if let Some(tier) = tier {
                explicit_tags.insert("tier".to_string(), tier);
            }
            let context = format!("target insert input '{}'", dir.display());
            let resolved_identity =
                resolve_target_identity(&dir, &explicit_tags, &target_schema, &context)?;
            let input_stats = collect_input_sizing_stats(&prepared_input.dir)?;
            let product = repo_manifest.product_id;
            let content_repo = repo_manifest.content_repo_id;
            let target_id = resolved_identity.target_id.clone();

            let target_root = repo_paths.targets_root.join(&target_id);
            let bundles_root = repo_paths.bundles_root.clone();

            std::fs::create_dir_all(&target_root)?;

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
            let existing_chunk_ids: HashSet<String> =
                existing_chunk_locations.keys().cloned().collect();
            if repo_paths.targets_root.exists() {
                for entry in walkdir::WalkDir::new(&repo_paths.targets_root).sort_by_file_name() {
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
                &prepared_input.dir,
                &product,
                &content_repo,
                &resolved_identity.version_id,
                &resolved_identity.platform,
                &resolved_identity.architecture,
                &resolved_identity.tier_id,
                &existing_chunk_locations,
                &existing_bundle_ids,
                &previous_file_manifests,
                &chunk_sizing_profile,
                &bundle_sizing_profile,
                &compression_profile,
            )?;
            let mut result = result;
            apply_resolved_target_identity(&mut result.target_manifest, &resolved_identity);

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

            let required_chunk_count = result.target_manifest.required_chunk_set.len();
            let required_bundle_count = result.target_manifest.required_bundle_set.len();
            let generated_bundle_count = result.bundles.len();
            let generated_bundle_bytes: u64 = result
                .bundles
                .iter()
                .map(|bundle| bundle.data.len() as u64)
                .sum();
            let reused_chunk_count = result
                .target_manifest
                .required_chunk_set
                .iter()
                .filter(|chunk_id| existing_chunk_ids.contains(chunk_id.as_str()))
                .count();
            let new_chunk_count = required_chunk_count.saturating_sub(reused_chunk_count);
            let reused_bundle_count = required_bundle_count.saturating_sub(generated_bundle_count);
            let reused_chunk_percent =
                ratio_pct(reused_chunk_count as u64, required_chunk_count as u64);
            let reused_bundle_percent =
                ratio_pct(reused_bundle_count as u64, required_bundle_count as u64);
            let stats = TargetInsertStats {
                command: "target insert",
                repo_root: repo_paths.repo_root.display().to_string(),
                input_path: dir.display().to_string(),
                target_id: result.target_manifest.target_id.clone(),
                manifest_path: manifest_path.display().to_string(),
                product_id: product,
                content_repo_id: content_repo,
                version_id: resolved_identity.version_id,
                platform: resolved_identity.platform,
                architecture: resolved_identity.architecture,
                tier_id: resolved_identity.tier_id,
                file_count: result.target_manifest.files.len(),
                total_logical_size: result.target_manifest.total_logical_size,
                total_compressed_size_estimate: result
                    .target_manifest
                    .total_compressed_size_estimate,
                required_chunk_count,
                required_bundle_count,
                generated_bundle_count,
                generated_bundle_bytes,
                reused_chunk_count,
                new_chunk_count,
                reused_chunk_percent,
                reused_bundle_count,
                reused_bundle_percent,
                timings: build_timings_summary(&timings),
                outputs: optional_output_paths(&summary_out, &stats_json_out),
            };

            let summary_lines = vec![
                format!("command: {}", stats.command),
                format!("repo_root: {}", stats.repo_root),
                format!("input_path: {}", stats.input_path),
                format!("target_id: {}", stats.target_id),
                format!("manifest_path: {}", stats.manifest_path),
                format!(
                    "identity: product={}, content_repo={}, version={}, platform={}, architecture={}, tier={}",
                    stats.product_id,
                    stats.content_repo_id,
                    stats.version_id,
                    stats.platform,
                    stats.architecture,
                    stats.tier_id
                ),
                format!(
                    "files: count={}, logical_size={}, compressed_estimate={}",
                    stats.file_count,
                    format_bytes(stats.total_logical_size),
                    format_bytes(stats.total_compressed_size_estimate)
                ),
                format!(
                    "chunks: required={}, new={}, reused={}, reuse={:.2}%",
                    stats.required_chunk_count,
                    stats.new_chunk_count,
                    stats.reused_chunk_count,
                    stats.reused_chunk_percent
                ),
                format!(
                    "bundles: required={}, generated={}, reused={}, generated_size={}, reuse={:.2}%",
                    stats.required_bundle_count,
                    stats.generated_bundle_count,
                    stats.reused_bundle_count,
                    format_bytes(stats.generated_bundle_bytes),
                    stats.reused_bundle_percent
                ),
                format!("timings_total_seconds: {:.2}", stats.timings.total_seconds),
            ];
            write_optional_summary(&summary_out, &summary_lines)?;
            write_optional_stats_json(&stats_json_out, &stats)?;
        }

        Commands::Target {
            command:
                TargetCommands::Export {
                    repo,
                    target,
                    output,
                    summary_out,
                    stats_json_out,
                },
        } => {
            let (repo_paths, _repo_manifest, _manifest_path, tm) =
                load_target_manifest_from_repo(&repo, &target)?;
            let bundles = repo_paths.bundles_root;

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

            let stats = TargetExportStats {
                command: "target export",
                repo_root: repo_paths.repo_root.display().to_string(),
                target_id: tm.target_id.clone(),
                output_dir: output.display().to_string(),
                file_count: tm.files.len(),
                required_chunk_count: tm.required_chunk_set.len(),
                required_bundle_count: tm.required_bundle_set.len(),
                resolved_bundle_file_count: bundle_paths.len(),
                total_logical_size: tm.total_logical_size,
                total_compressed_size_estimate: tm.total_compressed_size_estimate,
                timings: reconstruct_timings_summary(&timings),
                outputs: optional_output_paths(&summary_out, &stats_json_out),
            };
            let summary_lines = vec![
                format!("command: {}", stats.command),
                format!("repo_root: {}", stats.repo_root),
                format!("target_id: {}", stats.target_id),
                format!("output_dir: {}", stats.output_dir),
                format!(
                    "files: count={}, logical_size={}, compressed_estimate={}",
                    stats.file_count,
                    format_bytes(stats.total_logical_size),
                    format_bytes(stats.total_compressed_size_estimate)
                ),
                format!(
                    "bundles: required={}, resolved={}",
                    stats.required_bundle_count, stats.resolved_bundle_file_count
                ),
                format!("chunks: required={}", stats.required_chunk_count),
                format!("timings_total_seconds: {:.2}", stats.timings.total_seconds),
            ];
            write_optional_summary(&summary_out, &summary_lines)?;
            write_optional_stats_json(&stats_json_out, &stats)?;
        }

        Commands::TargetInstall {
            command:
                TargetInstallCommands::Verify {
                    repo,
                    target,
                    dir,
                    summary_out,
                    stats_json_out,
                },
        } => {
            let (repo_paths, _repo_manifest, _manifest_path, tm) =
                load_target_manifest_from_repo(&repo, &target)?;

            let (errors, timings) = g::verify_directory_with_timings(&tm, &dir)?;
            let mut missing_file_count = 0usize;
            let mut corrupt_file_count = 0usize;
            let mut extra_file_count = 0usize;
            for error in &errors {
                if error.starts_with("missing: ") {
                    missing_file_count += 1;
                } else if error.starts_with("extra: ") {
                    extra_file_count += 1;
                } else {
                    corrupt_file_count += 1;
                }
            }
            let stats = TargetInstallVerifyStats {
                command: "target_install verify",
                repo_root: repo_paths.repo_root.display().to_string(),
                target_id: tm.target_id.clone(),
                install_dir: dir.display().to_string(),
                expected_file_count: tm.files.len(),
                missing_file_count,
                corrupt_file_count,
                extra_file_count,
                verification_passed: errors.is_empty(),
                errors: errors.clone(),
                timings: verify_timings_summary(&timings),
                outputs: optional_output_paths(&summary_out, &stats_json_out),
            };
            let summary_lines = vec![
                format!("command: {}", stats.command),
                format!("repo_root: {}", stats.repo_root),
                format!("target_id: {}", stats.target_id),
                format!("install_dir: {}", stats.install_dir),
                format!(
                    "status: {}",
                    if stats.verification_passed {
                        "passed"
                    } else {
                        "failed"
                    }
                ),
                format!("expected_file_count: {}", stats.expected_file_count),
                format!(
                    "issues: missing={}, corrupt={}, extra={}",
                    stats.missing_file_count, stats.corrupt_file_count, stats.extra_file_count
                ),
                format!("timings_total_seconds: {:.2}", stats.timings.total_seconds),
            ];
            write_optional_summary(&summary_out, &summary_lines)?;
            write_optional_stats_json(&stats_json_out, &stats)?;

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

        Commands::Bundle {
            command: BundleCommands::Inspect { bundle },
        } => {
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

        Commands::Path {
            command:
                PathCommands::Compare {
                    expected,
                    actual,
                    summary_out,
                    stats_json_out,
                },
        } => {
            let stats = compare_paths(
                &expected,
                &actual,
                optional_output_paths(&summary_out, &stats_json_out),
            )?;
            let mut summary_lines = vec![
                format!("command: {}", stats.command),
                format!("expected_path: {}", stats.expected_path),
                format!("actual_path: {}", stats.actual_path),
                format!(
                    "status: {}",
                    if stats.compare_passed {
                        "match"
                    } else {
                        "failed"
                    }
                ),
                format!(
                    "files: expected={}, actual={}, matched={}, mismatched={}, missing={}, extra={}",
                    stats.expected_file_count,
                    stats.actual_file_count,
                    stats.matched_file_count,
                    stats.mismatched_file_count,
                    stats.missing_file_count,
                    stats.extra_file_count
                ),
            ];
            if !stats.mismatched_files.is_empty() {
                summary_lines.push("[mismatched]".to_string());
                summary_lines.extend(stats.mismatched_files.iter().cloned());
            }
            if !stats.missing_files.is_empty() {
                summary_lines.push("[missing]".to_string());
                summary_lines.extend(stats.missing_files.iter().cloned());
            }
            if !stats.extra_files.is_empty() {
                summary_lines.push("[extra]".to_string());
                summary_lines.extend(stats.extra_files.iter().cloned());
            }
            write_optional_summary(&summary_out, &summary_lines)?;
            write_optional_stats_json(&stats_json_out, &stats)?;

            if stats.compare_passed {
                println!(
                    "binary compare passed: {} matched, {} expected files",
                    stats.matched_file_count, stats.expected_file_count
                );
            } else {
                eprintln!(
                    "binary compare failed: mismatched={}, missing={}, extra={}",
                    stats.mismatched_file_count, stats.missing_file_count, stats.extra_file_count
                );
                for relative_path in &stats.mismatched_files {
                    eprintln!("  MISMATCH: {}", relative_path);
                }
                for relative_path in &stats.missing_files {
                    eprintln!("  MISSING: {}", relative_path);
                }
                for relative_path in &stats.extra_files {
                    eprintln!("  EXTRA: {}", relative_path);
                }
                std::process::exit(1);
            }
        }

        Commands::Target {
            command:
                TargetCommands::Estimate {
                    dir,
                    product,
                    version,
                },
        } => {
            let prepared_input = prepare_input_path(&dir)?;
            let result = g::build_from_directory(
                &prepared_input.dir,
                &product,
                "base",
                &version,
                "windows",
                "x86_64",
                "base",
            )?;

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
