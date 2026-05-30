use crate::TargetCommands;
use crate::cli::common::{
    TargetExportStats, TargetInsertStats, collect_input_sizing_stats,
    count_required_bundles_for_build, count_required_bundles_from_chunk_index,
    count_required_chunks, load_target_manifest_from_repo, prepare_input_path, write_bundles,
    write_report, write_target_manifest,
};
use crate::cli::progress::TargetInsertProgress;
use crate::cli::reporting::{
    build_timings_summary, optional_output_paths, print_build_timings, print_reconstruct_timings,
    ratio_pct, reconstruct_timings_summary,
};
use crate::cli::verbose_output_enabled;
use g::cli_identity::{apply_resolved_target_identity, parse_cli_tags, resolve_target_identity};
use g::repo::{rebuild_chunk_index_from_bundles, required_chunk_hashes, scan_existing_store_state};
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
            memory_mode,
            max_memory_mib,
            jobs,
            summary_out,
            stats_json_out,
        } => {
            let prepared_input = prepare_input_path(&dir)?;
            let (repo_paths, mut repo_manifest) = g::load_gutch_repo_manifest(&repo)?;
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
            let gutch_repo = repo_manifest.gutch_repo_id.clone();
            let bundles_root = repo_paths.bundles_root.clone();

            let existing_store_state = scan_existing_store_state(&repo_paths)?;
            let existing_chunk_ids = existing_store_state
                .chunk_locations
                .keys()
                .cloned()
                .collect::<std::collections::HashSet<_>>();

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
                compression_unit: g::manifest::COMPRESSION_UNIT_BUNDLE.to_string(),
                zstd_level: 1,
            };

            let mut build_config =
                g::BuildConfig::with_ids(&gutch_repo, "", &resolved_identity.target_tags);
            build_config.existing_chunk_locations = existing_store_state.chunk_locations.clone();
            build_config.existing_bundle_ids = existing_store_state.bundle_ids.clone();
            build_config.previous_file_manifests =
                existing_store_state.previous_file_manifests.clone();
            build_config.chunk_profile = chunk_sizing_profile.clone();
            build_config.bundle_profile = bundle_sizing_profile.clone();
            build_config.compression_profile = compression_profile.clone();
            build_config.runtime_hints = g::RuntimeResourceHints {
                memory_mode,
                max_memory_mib,
                jobs,
            };
            let build_progress = TargetInsertProgress::new();
            build_config.progress = Some(build_progress.as_sink());

            let (result, timings) = g::build_with_config(&prepared_input.dir, &build_config)?;
            let mut result = result;
            let target_id = g::compute_target_id_from_files(&result.target_manifest.files);
            result.target_manifest.target_id = target_id.clone();
            apply_resolved_target_identity(&mut result.target_manifest, &resolved_identity);
            build_progress.set_message("write repo outputs");

            let existing_manifest_path = g::target_manifest_path(&repo_paths, &target_id);
            if existing_manifest_path.exists() {
                let existing_manifest = g::load_target_manifest(&repo_paths, &target_id)?;
                if existing_manifest.target_tags != result.target_manifest.target_tags {
                    return Err(g::Error::manifest(format!(
                        "target content id '{}' already exists with different target tags",
                        target_id
                    )));
                }
            }

            let (manifest_path, _) = write_target_manifest(&repo_paths, &result.target_manifest)?;
            write_bundles(&bundles_root, &result.bundles)?;
            let chunk_index = rebuild_chunk_index_from_bundles(&repo_paths)?;
            g::write_chunk_index(&repo_paths, &chunk_index)?;

            if verbose_output_enabled() {
                println!(
                    "done. {} files in manifest, total logical: {} bytes",
                    result.target_manifest.files.len(),
                    result.target_manifest.total_logical_size
                );

                if let Ok(est) = g::estimate_build(&result.target_manifest, &Default::default(), 0)
                {
                    println!(
                        "estimate: {} chunks missing, {:.1} MiB logical",
                        est.missing_chunk_count,
                        est.missing_logical_bytes as f64 / (1024.0 * 1024.0)
                    );
                }
            }

            print_build_timings(&timings);

            let required_chunk_count = count_required_chunks(&result.target_manifest);
            let required_bundle_count = count_required_bundles_for_build(
                &result.target_manifest,
                &existing_store_state.chunk_locations,
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
                product_id: gutch_repo.clone(),
                gutch_repo_id: gutch_repo,
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
            g::write_gutch_repo_manifest(&repo_paths, &repo_manifest)?;
            build_progress.finish_and_clear();
        }

        TargetCommands::Delete { repo, target, yes } => {
            let (repo_paths, _repo_manifest, _, _tm) =
                load_target_manifest_from_repo(&repo, &target)?;
            let manifest_path = g::target_manifest_path(&repo_paths, &target);
            let manifest_bin_path = g::target_manifest_bin_path(&repo_paths, &target);
            if !yes {
                eprintln!(
                    "WARNING: this will remove '{}' and related target artifacts, and may orphan bundles. Use --yes to confirm.",
                    manifest_path.display()
                );
                return Err(g::Error::manifest("deletion requires --yes flag"));
            }

            for path in [&manifest_path, &manifest_bin_path] {
                if path.exists() {
                    if path.is_dir() {
                        std::fs::remove_dir_all(path)?;
                    } else {
                        std::fs::remove_file(path)?;
                    }
                }
            }
            if crate::cli::verbose_output_enabled() {
                println!("removed target: {}", target);
            }

            let remaining_targets: std::collections::HashSet<String> = {
                let mut ids = std::collections::HashSet::new();
                if repo_paths.targets_root.exists() {
                    for entry in std::fs::read_dir(&repo_paths.targets_root)? {
                        let entry = entry?;
                        if entry.path().is_file()
                            && entry.path().extension().and_then(|value| value.to_str())
                                == Some("json")
                        {
                            if let Some(target_id) =
                                entry.path().file_stem().and_then(|value| value.to_str())
                            {
                                ids.insert(target_id.to_string());
                            }
                        }
                    }
                }
                ids
            };

            let chunk_index = g::load_chunk_index(&repo_paths)?;
            let mut required_bundle_ids = std::collections::HashSet::new();
            for target_id in &remaining_targets {
                let manifest = g::load_target_manifest(&repo_paths, target_id)?;
                required_bundle_ids.extend(g::repo::required_bundle_ids(&manifest, &chunk_index));
            }

            let mut orphan_bundles = Vec::new();
            if repo_paths.bundles_root.exists() {
                for entry in walkdir::WalkDir::new(&repo_paths.bundles_root).sort_by_file_name() {
                    let entry = entry?;
                    if !entry.path().is_file() {
                        continue;
                    }
                    if entry.path().extension().and_then(|e| e.to_str()) != Some("bundle") {
                        continue;
                    }
                    if let Some(bundle_id) = entry.path().file_stem().and_then(|s| s.to_str()) {
                        if !required_bundle_ids.contains(bundle_id) {
                            orphan_bundles.push(entry.path().to_path_buf());
                        }
                    }
                }
            }

            for orphan_path in &orphan_bundles {
                std::fs::remove_file(orphan_path)?;
                if crate::cli::verbose_output_enabled() {
                    println!("removed orphaned bundle: {}", orphan_path.display());
                }
            }

            println!(
                "deleted target '{}': removed manifest, {} orphaned bundles cleaned up",
                target,
                orphan_bundles.len()
            );
        }

        TargetCommands::Extract {
            repo,
            target,
            output,
            memory_mode,
            max_memory_mib,
            jobs,
            summary_out,
            stats_json_out,
        } => {
            let (repo_paths, _repo_manifest, _manifest_path, tm) =
                load_target_manifest_from_repo(&repo, &target)?;
            let bundle_paths =
                crate::cli::common::resolve_required_bundle_paths(&repo_paths.bundles_root)?;
            let chunk_index = g::load_chunk_index(&repo_paths)?;

            let runtime_hints = g::RuntimeResourceHints {
                memory_mode,
                max_memory_mib,
                jobs,
            };
            let (_, timings) = g::reconstruct_from_bundle_paths_with_timings_and_hints(
                &tm,
                &bundle_paths,
                &output,
                &runtime_hints,
            )?;
            if verbose_output_enabled() {
                println!("reconstructed to: {}", output.display());
            }
            print_reconstruct_timings(&timings);

            let stats = TargetExportStats {
                command: "target extract",
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
