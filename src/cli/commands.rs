use crate::cli::Cli;
use crate::cli::Commands;
use crate::cli::bundle;
use crate::cli::path;

pub(crate) fn run(
    cli: Cli,
    remote_provider: Option<&dyn crate::api::RemoteRepositoryCommandProvider>,
    profile: &crate::cli::Profile,
) -> crate::api::Result<()> {
    match cli.command {
        Commands::Repo { command } => crate::cli::repo::run(command, remote_provider, profile),
        Commands::Snapshot { command } => crate::cli::snapshot::run(command, profile),
        Commands::Reconstructed { command } => {
            crate::cli::installation::run_reconstructed(command, profile)
        }
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
