//! Routes a parsed command to its family handler.

use crate::cli::Cli;
use crate::cli::Commands;
use crate::cli::DebugCommands;
use crate::cli::Profile;
use crate::cli::bundle;
use crate::cli::installation;
use crate::cli::installation::CorruptCommand;
use crate::cli::installation::run_installation;
use crate::cli::repo;
use crate::cli::snapshot;
use crate::cli::tree;
use crate::sync::remote::RemoteRepositoryCommandProvider;

pub(crate) fn run(
    cli: Cli,
    remote_provider: Option<&dyn RemoteRepositoryCommandProvider>,
    profile: &Profile,
) -> crate::Result<()> {
    match cli.command {
        Commands::Repo { command } => repo::run(command, remote_provider, profile),
        Commands::Snapshot { command } => snapshot::run(command, profile),
        Commands::Installation { command } => run_installation(command, profile),
        Commands::Debug { command } => match command {
            DebugCommands::CompareTrees { expected, actual } => tree::compare(expected, actual),
            DebugCommands::CorruptInstallation {
                destination,
                plan,
                yes,
            } => installation::corrupt(
                CorruptCommand {
                    destination,
                    plan,
                    yes,
                },
                profile,
            ),
            DebugCommands::InspectBundle { bundle } => bundle::inspect(bundle),
        },
    }
}
