use crate::cli::SnapshotSourcesCommands;
use crate::cli::actions::snapshot_sources::CliSnapshotSourcesCaptureOptions;
use crate::cli::actions::snapshot_sources::capture_snapshot_sources;
use crate::cli::reporting::ensure_parent_directory;

pub(crate) fn run(
    command: SnapshotSourcesCommands,
    profile: &crate::cli::Profile,
) -> crate::api::Result<()> {
    match command {
        SnapshotSourcesCommands::Capture {
            repo,
            source_root,
            yes,
            json_out,
        } => {
            let result = capture_snapshot_sources(
                &repo,
                &source_root,
                CliSnapshotSourcesCaptureOptions { yes },
                profile,
            )?;

            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                crate::filesystem::write(json_out, serde_json::to_vec_pretty(&result)?)?;
            }

            Ok(())
        }
    }
}
