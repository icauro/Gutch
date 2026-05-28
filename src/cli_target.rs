use std::collections::{HashMap, HashSet};

use crate::TargetCommands;
use crate::cli_common::{
    TargetExportStats, TargetInsertStats, collect_input_sizing_stats,
    count_required_bundles_for_build, count_required_bundles_from_chunk_index,
    count_required_chunks, load_target_manifest_from_repo, prepare_input_path, write_report,
};
use crate::cli_reporting::{
    build_timings_summary, optional_output_paths, print_build_timings, print_reconstruct_timings,
    ratio_pct, reconstruct_timings_summary,
};
use g::cli_identity::{apply_resolved_target_identity, parse_cli_tags, resolve_target_identity};
use g::repo::required_chunk_hashes;
use gutch as g;

pub(crate) fn run(command: TargetCommands) -> g::Result<()> {
    match command {
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
        } => {
            let prepared_input = prepare_input_path(&dir)?;
            let (repo_paths, mut repo_manifest) = g::load_content_repo_manifest(&repo)?;
            let rules = repo_manifest.target_schema.clone().unwrap_or_default();
            let mut explicit_tags = parse_cli_tags(&tags)?;
            if let Some(version) = version {
                explicit_tags.upsert_exclusive("version", version);
            }
            if let Some(platform) = platform {
                explicit_tags.upsert_exclusive("os", platform);
            }
            if let Some(architecture) = architecture {
                explicit_tags.upsert_exclusive("arch", architecture);
            }
            if let Some(tier) = tier {
                explicit_tags.upsert_exclusive("tier", tier);
            }
            let context = format!("target insert input '{}'", dir.display());
            let resolved_identity =
                resolve_target_identity(&dir, &explicit_tags, &rules, &context)?;
            let input_stats = collect_input_sizing_stats(&prepared_input.dir)?;
            let content_repo = repo_manifest.content_repo_id.clone();
            let target_id = g::generate_target_id();

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
                if entry.path().extension().is_some_and(|e| e == "bundle") {
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
                    let manifest: g::manifest::target::TargetManifest =
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
                &content_repo,
                &target_id,
                &resolved_identity.target_tags,
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

            let required_chunk_count = count_required_chunks(&result.target_manifest);
            let required_bundle_count = count_required_bundles_for_build(
                &result.target_manifest,
                &existing_chunk_locations,
                &result.bundles,
            );
            let generated_bundle_count = result.bundles.len();
            let generated_bundle_bytes: u64 = result
                .bundles
                .iter()
                .map(|bundle| bundle.data.len() as u64)
                .sum();
            let reused_chunk_count = required_chunk_hashes(&result.target_manifest)
                .into_iter()
                .filter(|chunk_id| existing_chunk_ids.contains(chunk_id))
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
                product_id: content_repo.clone(),
                content_repo_id: content_repo,
                version_id: resolved_identity
                    .target_tags
                    .get_exclusive("version")
                    .unwrap_or_default()
                    .to_string(),
                platform: resolved_identity
                    .target_tags
                    .get_exclusive("os")
                    .unwrap_or_default()
                    .to_string(),
                architecture: resolved_identity
                    .target_tags
                    .get_exclusive("arch")
                    .unwrap_or_default()
                    .to_string(),
                tier_id: resolved_identity
                    .target_tags
                    .get_exclusive("tier")
                    .unwrap_or_default()
                    .to_string(),
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

            write_report(&summary_out, &stats_json_out, &stats)?;

            repo_manifest.chunk_sizing_profile = chunk_sizing_profile;
            repo_manifest.bundle_sizing_profile = bundle_sizing_profile;
            repo_manifest.compression_profile = compression_profile;
            g::write_content_repo_manifest(&repo_paths, &repo_manifest)?;
        }

        TargetCommands::Export {
            repo,
            target,
            output,
            summary_out,
            stats_json_out,
        } => {
            let (repo_paths, _repo_manifest, _manifest_path, tm) =
                load_target_manifest_from_repo(&repo, &target)?;
            let bundle_paths =
                crate::cli_common::resolve_required_bundle_paths(&repo_paths.bundles_root)?;
            let chunk_index = g::load_chunk_index(&repo_paths)?;

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
                required_chunk_count: count_required_chunks(&tm),
                required_bundle_count: count_required_bundles_from_chunk_index(&tm, &chunk_index),
                resolved_bundle_file_count: bundle_paths.len(),
                total_logical_size: tm.total_logical_size,
                total_compressed_size_estimate: tm.total_compressed_size_estimate,
                timings: reconstruct_timings_summary(&timings),
                outputs: optional_output_paths(&summary_out, &stats_json_out),
            };
            write_report(&summary_out, &stats_json_out, &stats)?;
        }

        TargetCommands::Estimate { dir, version } => {
            let prepared_input = prepare_input_path(&dir)?;
            let result = {
                let mut build_tags = g::manifest::TargetTagSet::default();
                build_tags.upsert_exclusive("version", version.clone());
                build_tags.upsert_exclusive("os", "windows");
                build_tags.upsert_exclusive("arch", "x86_64");
                build_tags.upsert_exclusive("tier", "base");
                g::build_from_directory(&prepared_input.dir, "base", "1", &build_tags)?
            };

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
