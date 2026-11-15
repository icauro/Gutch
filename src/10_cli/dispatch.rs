//! Routes a parsed command to its family handler.

use crate::cli::BundleCommands;
use crate::cli::Cli;
use crate::cli::Commands;
use crate::cli::DebugCommands;
use crate::cli::InstallationCommands;
use crate::cli::Profile;
use crate::cli::RepoCommands;
use crate::cli::SnapshotCommands;
use crate::cli::TreeCommands;
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
    if cli.dry_run {
        let supported = matches!(
            &cli.command,
            Commands::Repo {
                command: RepoCommands::Init { .. }
                    | RepoCommands::Rename { .. }
                    | RepoCommands::Constraints { .. }
                    | RepoCommands::Index { .. }
                    | RepoCommands::Push { .. }
                    | RepoCommands::Pull { .. }
            } | Commands::Snapshot {
                command: SnapshotCommands::Capture(_)
                    | SnapshotCommands::Delete { .. }
                    | SnapshotCommands::Tags { .. }
            } | Commands::Installation {
                command: InstallationCommands::Preview(_) | InstallationCommands::Repair { .. }
            }
        ) || matches!(
            &cli.command,
            Commands::Installation { command: InstallationCommands::Reconstruct(arguments) }
                if !arguments.fetch
        );
        if !supported {
            return Err(crate::Error::invalid_input(
                "--dry-run is not supported for this command",
            ));
        }
    }
    match cli.command {
        Commands::Repo { command } => repo::run(command, remote_provider, profile),
        Commands::Snapshot { command } => snapshot::run(command, profile),
        Commands::Installation { command } => run_installation(command, profile),
        Commands::Debug {
            command:
                DebugCommands::CorruptInstallation {
                    destination,
                    plan,
                    stats_json_out,
                    yes,
                },
        } => installation::corrupt(
            CorruptCommand {
                destination,
                plan,
                stats_json_out,
                yes,
            },
            profile,
        ),
        Commands::Bundle { command } => match command {
            BundleCommands::Inspect { bundle } => bundle::inspect(bundle),
        },
        Commands::Tree { command } => match command {
            TreeCommands::Compare {
                expected,
                actual,
                stats_json_out,
            } => tree::compare(expected, actual, stats_json_out),
        },
    }
}
