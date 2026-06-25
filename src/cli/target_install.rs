use crate::cli::TargetInstallCommands;
use crate::cli::actions;
use crate::cli::common::parse_bundle_downloads;
use crate::cli::verbose_output_enabled;
use gutch as g;

pub(crate) fn run(command: TargetInstallCommands) -> g::Result<()> {
    match command {
        TargetInstallCommands::Fetch {
            repo,
            target,
            manifest_url,
            chunk_index_url,
            bundle_urls_endpoint,
            session_id,
            bundles,
        } => {
            let result = g::fetch_target_install_content(&g::TargetInstallFetchRequest {
                repo_path: repo,
                target_id: target,
                manifest_url,
                chunk_index_url,
                bundle_urls_endpoint,
                session_id,
                bundles: parse_bundle_downloads(&bundles)?,
                progress: None,
            })?;

            if verbose_output_enabled() {
                println!(
                    "fetched target install content for {} into {}",
                    result.target_id,
                    result.repo_path.display()
                );
                println!(
                    "bundles: required={}, downloaded={}, existing={}",
                    result.required_bundle_count,
                    result.downloaded_bundle_count,
                    result.skipped_existing_bundle_count
                );
            }
        }

        TargetInstallCommands::Apply {
            repo,
            target,
            dir,
            staging_dir,
            source_target,
            source_dir,
            memory_mode,
            max_memory_mib,
            jobs,
            summary_out,
            stats_json_out,
        } => actions::target_install::apply_install(actions::target_install::ApplyCommand {
            repo,
            target,
            dir,
            staging_dir,
            source_target,
            source_dir,
            memory_mode,
            max_memory_mib,
            jobs,
            summary_out,
            stats_json_out,
        })?,

        TargetInstallCommands::Verify {
            repo,
            target,
            dir,
            strict,
            memory_mode,
            max_memory_mib,
            jobs,
            summary_out,
            stats_json_out,
        } => actions::target_install::verify_install(actions::target_install::VerifyCommand {
            repo,
            target,
            dir,
            verify_mode: if strict {
                g::VerifyMode::Strict
            } else {
                g::VerifyMode::Fast
            },
            memory_mode,
            max_memory_mib,
            jobs,
            summary_out,
            stats_json_out,
        })?,
    }

    Ok(())
}
