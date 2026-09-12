use crate::cli::Cli;
use crate::cli::Commands;
use crate::cli::bundle;
use crate::cli::path;

pub(crate) fn run(
    cli: Cli,
    remote_provider: Option<&dyn crate::api::RemoteRepositoryCommandProvider>,
    profile: &crate::cli::Profile,
) -> crate::api::Result<()> {
    if cli.dry_run {
        let supported = matches!(
            &cli.command,
            Commands::Repo {
                command: crate::cli::RepoCommands::Init { .. }
                    | crate::cli::RepoCommands::Rename { .. }
                    | crate::cli::RepoCommands::Constraints { .. }
                    | crate::cli::RepoCommands::RebuildIndex { .. }
                    | crate::cli::RepoCommands::Push { .. }
                    | crate::cli::RepoCommands::Pull { .. }
            } | Commands::Snapshot {
                command: crate::cli::SnapshotCommands::Capture { .. }
                    | crate::cli::SnapshotCommands::Reconstruct { .. }
                    | crate::cli::SnapshotCommands::Delete { .. }
                    | crate::cli::SnapshotCommands::Retag { .. }
            } | Commands::Installation {
                command: crate::cli::InstallationCommands::Plan { .. }
                    | crate::cli::InstallationCommands::Repair { .. }
            }
        );
        if !supported {
            return Err(crate::api::Error::manifest(
                "--dry-run is not supported for this command",
            ));
        }
    }
    match cli.command {
        Commands::Repo { command } => crate::cli::repo::run(command, remote_provider, profile),
        Commands::Snapshot { command } => crate::cli::snapshot::run(command, profile),
        Commands::Installation { command } => {
            crate::cli::installation::run_installation(command, profile)
        }
        Commands::Debug {
            command:
                crate::cli::DebugCommands::CorruptInstallation {
                    destination,
                    plan,
                    stats_json_out,
                    yes,
                },
        } => crate::cli::installation::corrupt(
            crate::cli::installation::CorruptCommand {
                destination,
                plan,
                stats_json_out,
                yes,
            },
            profile,
        ),
        Commands::Bundle { command } => match command {
            crate::cli::BundleCommands::Inspect { bundle, repo } => bundle::inspect(bundle, repo),
        },
        Commands::Path { command } => match command {
            crate::cli::PathCommands::Compare {
                expected,
                actual,
                stats_json_out,
            } => path::compare(expected, actual, stats_json_out),
        },
    }
}
