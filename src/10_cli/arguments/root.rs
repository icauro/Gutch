//! The root command, its global options and the `debug` family.

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
    /// Print a single structured result on stdout.
    #[arg(long, global = true, conflicts_with = "json_lines", help_heading = GLOBAL)]
    pub(crate) json: bool,
    /// Stream structured events and a final summary.
    #[arg(long, global = true, help_heading = GLOBAL)]
    pub(crate) json_lines: bool,
    /// Write the selected output format to FILE instead of stdout.
    #[arg(long, global = true, value_name = "FILE", value_parser = parse_cli_path, help_heading = GLOBAL)]
    pub(crate) output: Option<PathBuf>,
    /// Write a structured failure report, including filename violations and
    /// promotion recovery details, to FILE.
    #[arg(long, global = true, value_name = "FILE", value_parser = parse_cli_path, help_heading = GLOBAL)]
    pub(crate) error_output: Option<PathBuf>,
    /// Print additional details.
    #[arg(long, short = 'v', global = true, help_heading = GLOBAL)]
    pub(crate) verbose: bool,
    /// Diagnostic profiling: off, coarse summary, or sampled detailed timelines.
    #[arg(long, global = true, value_enum, default_value_t = ProfileMode::Off, help_heading = GLOBAL)]
    pub(crate) profile: ProfileMode,
    /// Profile report directory; required for detailed, optional for summary.
    #[arg(long = "profile-dir", global = true, value_name = "DIR", value_parser = parse_cli_path, help_heading = GLOBAL)]
    pub(crate) profile_out: Option<PathBuf>,
    #[command(subcommand)]
    pub(crate) command: Commands,
}

const GLOBAL: &str = "Global options";

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Manage repositories, constraints, indexes and remote synchronization.
    Repo {
        #[command(subcommand)]
        command: RepoCommands,
    },
    /// Capture, list, compare and manage stored snapshots.
    Snapshot {
        #[command(subcommand)]
        command: SnapshotCommands,
    },
    /// Preview, reconstruct, verify and repair installation trees.
    Installation {
        #[command(subcommand)]
        command: InstallationCommands,
    },
    /// Diagnostic tools for testing and support.
    Debug {
        #[command(subcommand)]
        command: DebugCommands,
    },
}

#[derive(Subcommand)]
pub(crate) enum DebugCommands {
    /// Compare two physical filesystem trees and fail on any difference.
    CompareTrees {
        /// Tree or file with the expected content.
        #[arg(long, value_name = "PATH", value_parser = parse_cli_path)]
        expected: PathBuf,
        /// Tree or file to check.
        #[arg(long, value_name = "PATH", value_parser = parse_cli_path)]
        actual: PathBuf,
    },
    /// Damage an installation as a JSON plan describes, to exercise repair.
    CorruptInstallation {
        /// Installation directory to damage.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        destination: PathBuf,
        /// JSON corruption plan.
        #[arg(long, value_name = "FILE", value_parser = parse_cli_path)]
        plan: PathBuf,
        /// Apply the plan without asking for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// List the entries stored in a bundle file.
    InspectBundle {
        /// Bundle file to read.
        #[arg(long, value_name = "FILE", value_parser = parse_cli_path)]
        bundle: PathBuf,
    },
}

#[cfg(test)]
#[path = "root_tests.rs"]
mod tests;
