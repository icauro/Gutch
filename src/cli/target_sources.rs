use crate::cli::TargetSourcesCommands;
use crate::cli::actions::target_sources::{TargetSourcesSyncOptions, sync_target_sources};
use crate::cli::reporting::ensure_parent_directory;
use gutch as g;

pub fn run(command: TargetSourcesCommands) -> g::Result<()> {
    match command {
        TargetSourcesCommands::Sync {
            repo,
            root,
            no_prune,
            yes,
            json_out,
        } => {
            let result = sync_target_sources(
                &repo,
                &root,
                TargetSourcesSyncOptions { no_prune, yes },
            )?;

            if let Some(json_out) = json_out.as_ref() {
                ensure_parent_directory(json_out)?;
                crate::helpers::write(json_out, serde_json::to_vec_pretty(&result)?)?;
            }

            Ok(())
        }
    }
}
