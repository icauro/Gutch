use crate::cli::{Cli, Commands, bundle, path};
use gutch as g;

pub(crate) fn run(cli: Cli) -> g::Result<()> {
    match cli.command {
        Commands::Repo { command } => crate::cli::repo::run(command),
        Commands::Target { command } => crate::cli::target::run(command),
        Commands::TargetInstall { command } => crate::cli::target_install::run(command),
        Commands::Bundle { command } => match command {
            crate::cli::BundleCommands::Inspect { bundle } => bundle::inspect(bundle),
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
