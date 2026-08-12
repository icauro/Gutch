use crate::cli::TargetInstallCommands;
use crate::cli::actions;
use crate::cli::common::parse_bundle_downloads;
use crate::cli::progress::CliProgressSink;
use crate::cli::verbose_output_enabled;
use gutch as g;

pub(crate) fn run(command: TargetInstallCommands) -> g::Result<()> {
    match command {
        TargetInstallCommands::Fetch {
            repo,
            target,
            metadata_url,
            blueprint_url,
            repo_metadata_url,
            chunk_index_url,
            bundle_downloads_endpoint,
            session_id,
            bundles,
        } => {
            let progress = CliProgressSink::new();
            let result = g::fetch_target_install_content(&g::TargetInstallFetchRequest {
                repo_path: repo,
                target_id: target,
                repo_metadata_url,
                metadata_url,
                blueprint_url,
                chunk_index_url,
                bundle_downloads_endpoint,
                session_id,
                bundles: parse_bundle_downloads(&bundles)?,
                control: g::GutchOperationControl::default().with_progress(progress),
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
            runtime,
            stats_json_out,
        } => actions::target_install::apply_install(actions::target_install::ApplyCommand {
            repo,
            target,
            dir,
            staging_dir,
            source_target,
            source_dir,
            runtime_hints: runtime.hints(),
            stats_json_out,
        })?,

        TargetInstallCommands::Verify {
            repo,
            target,
            dir,
            strict,
            runtime,
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
            runtime_hints: runtime.hints(),
            stats_json_out,
        })?,
    }

    Ok(())
}
