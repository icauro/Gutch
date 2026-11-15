//! The root command and the small command families: debug, bundle and tree.

use super::installation::InstallationCommands;
use super::repo::RepoCommands;
use super::shared::parse_cli_path;
use super::snapshot::SnapshotCommands;
use crate::profiling::ProfileMode;
use clap::Parser;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "gutch",
    version,
    about = "Capture snapshots and reconstruct verified installation trees"
)]
pub(crate) struct Cli {
    /// Write structured failures, including filename violations and promotion recovery details.
    #[arg(long = "error-json-out", global = true, value_parser = parse_cli_path)]
    pub(crate) error_json_out: Option<PathBuf>,
    #[arg(long, short = 'v', global = true)]
    pub(crate) verbose: bool,

    /// Diagnostic profiling: off, coarse summary, or sampled detailed timelines.
    #[arg(long, global = true, value_enum, default_value_t = crate::profiling::ProfileMode::Off)]
    pub(crate) profile: ProfileMode,
    /// Report directory; required for detailed, optional for summary.
    #[arg(long = "profile-dir", global = true, value_parser = parse_cli_path)]
    pub(crate) profile_out: Option<PathBuf>,

    /// Print a single structured result on stdout.
    #[arg(long, global = true, conflicts_with = "json_lines")]
    pub(crate) json: bool,
    /// Stream structured events and a final summary.
    #[arg(long, global = true)]
    pub(crate) json_lines: bool,
    /// Write the selected output format to a file.
    #[arg(long, global = true, value_parser = parse_cli_path)]
    pub(crate) output: Option<PathBuf>,
    /// Preview the selected operation without applying changes.
    #[arg(long, global = true)]
    pub(crate) dry_run: bool,
    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Manage repositories, constraints, indexes and remote synchronization.
    Repo {
        #[command(subcommand)]
        command: RepoCommands,
    },
    /// Capture and manage stored snapshots.
    Snapshot {
        #[command(subcommand)]
        command: SnapshotCommands,
    },
    /// Preview, reconstruct, verify and repair installation trees.
    Installation {
        #[command(subcommand)]
        command: InstallationCommands,
    },
    Debug {
        #[command(subcommand)]
        command: DebugCommands,
    },
    Bundle {
        #[command(subcommand)]
        command: BundleCommands,
    },
    /// Compare physical filesystem trees.
    Tree {
        #[command(subcommand)]
        command: TreeCommands,
    },
}

#[derive(Subcommand)]
pub(crate) enum DebugCommands {
    CorruptInstallation {
        #[arg(short, long, value_parser = parse_cli_path)]
        destination: PathBuf,
        #[arg(long = "plan", value_parser = parse_cli_path)]
        plan: PathBuf,
        #[arg(long = "yes")]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum BundleCommands {
    Inspect {
        #[arg(short, long, value_parser = parse_cli_path)]
        bundle: PathBuf,
    },
}

#[derive(Subcommand)]
pub(crate) enum TreeCommands {
    Compare {
        #[arg(long = "expected", value_parser = parse_cli_path)]
        expected: PathBuf,
        #[arg(long = "actual", value_parser = parse_cli_path)]
        actual: PathBuf,
    },
}

#[cfg(test)]
#[path = "root_tests.rs"]
mod tests;
