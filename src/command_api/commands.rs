use crate::command_api::{Cli, Commands, bundle, path};
use gutch as g;

pub(crate) fn run(
    cli: Cli,
    remote_provider: Option<&dyn g::RemoteRepositoryCommandProvider>,
    profile: &crate::command_api::Profile,
) -> g::Result<()> {
    match cli.command {
        Commands::Repo { command } => crate::command_api::repo::run(command, remote_provider, profile),
        Commands::Snapshot { command } => crate::command_api::snapshot::run(command, profile),
        Commands::Reconstructed { command } => {
            crate::command_api::snapshot_ops::run_reconstructed(command, profile)
        }
        Commands::Bundle { command } => match command {
            crate::command_api::BundleCommands::Inspect { bundle, repo } => bundle::inspect(bundle, repo),
        },
        Commands::Path { command } => match command {
            crate::command_api::PathCommands::Compare {
                expected,
                actual,
                stats_json_out,
            } => path::compare(expected, actual, stats_json_out),
        },
    }
}
