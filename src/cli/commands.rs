use crate::cli::{Cli, Commands, auth, bundle, path};
use gutch as g;

pub(crate) fn run(cli: Cli) -> g::Result<()> {
    match cli.command {
        Commands::Auth { command } => auth::run(command),
        Commands::Repo { command } => crate::cli::repo::run(command),
        Commands::Interactive => crate::cli::interactive::run(),
        Commands::Target { command } => crate::cli::target::run(command),
        Commands::TargetInstall { command } => crate::cli::target_install::run(command),
        Commands::Bundle { command } => match command {
            crate::cli::BundleCommands::Inspect { bundle } => bundle::inspect(bundle),
        },
        Commands::Path { command } => match command {
            crate::cli::PathCommands::Compare {
                expected,
                actual,
                summary_out,
                stats_json_out,
            } => path::compare(expected, actual, summary_out, stats_json_out),
        },
    }
}
