use crate::command_api::SnapshotSourcesCommands;
use crate::command_api::actions::snapshot_sources::{
    CliSnapshotSourcesCaptureOptions, capture_snapshot_sources,
};
use crate::command_api::reporting::ensure_parent_directory;
use gutch as g;

pub(crate) fn run(
    command: SnapshotSourcesCommands,
    profile: &crate::command_api::Profile,
) -> g::Result<()> {
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
                crate::helpers::write(json_out, serde_json::to_vec_pretty(&result)?)?;
            }

            Ok(())
        }
    }
}
