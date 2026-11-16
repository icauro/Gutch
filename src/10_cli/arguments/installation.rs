//! `installation` command arguments.

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use super::shared::{ConfirmArgs, FetchSourceArgs, RuntimeArgs, TransferArgs, parse_cli_path};

#[derive(Args)]
pub(crate) struct ReconstructArgs {
    #[arg(long = "repo", value_parser = parse_cli_path)]
    pub(crate) repo: PathBuf,
    #[arg(long = "snapshot")]
    pub(crate) snapshot_id: String,
    #[arg(short, long, value_parser = parse_cli_path)]
    pub(crate) destination: PathBuf,
    #[arg(long = "staging-dir", value_parser = parse_cli_path)]
    pub(crate) staging_dir: Option<PathBuf>,
    #[arg(long = "reuse-snapshot")]
    pub(crate) source_snapshot_id: Option<String>,
    #[arg(long = "reuse-from", value_parser = parse_cli_path)]
    pub(crate) source_dir: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) runtime: RuntimeArgs,
    /// Show what would change without applying it.
    #[arg(long, conflicts_with = "fetch")]
    pub(crate) dry_run: bool,
    /// Fetch authorized snapshot data and artifacts while reconstructing.
    #[arg(long)]
    pub(crate) fetch: bool,
    #[arg(
        long = "snapshot-metadata-url",
        required_if_eq("fetch", "true"),
        requires = "fetch"
    )]
    pub(crate) metadata_url: Option<String>,
    #[arg(
        long = "blueprint-url",
        required_if_eq("fetch", "true"),
        requires = "fetch"
    )]
    pub(crate) blueprint_url: Option<String>,
    /// Used only with `--fetch`.
    #[command(flatten)]
    pub(crate) sources: FetchSourceArgs,
    #[command(flatten)]
    pub(crate) transfer: TransferArgs,
}

/// How a previewed installation would be published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum PreviewPublication {
    /// Replace only missing or damaged entries, as repair does.
    Selective,
    /// Rebuild in staging and replace the whole destination, as reconstruct does.
    ReplaceDirectory,
}

#[derive(Args)]
pub(crate) struct PreviewArgs {
    #[arg(long, value_parser = parse_cli_path)]
    pub(crate) repo: PathBuf,
    #[arg(long = "snapshot")]
    pub(crate) snapshot_id: String,
    #[arg(long, value_parser = parse_cli_path)]
    pub(crate) destination: PathBuf,
    #[arg(long = "staging-dir", value_parser = parse_cli_path)]
    pub(crate) staging_dir: Option<PathBuf>,
    #[arg(long = "mode", value_enum, default_value = "selective")]
    pub(crate) publication: PreviewPublication,
    /// Remove entries absent from the target snapshot; they are kept by default.
    #[arg(long = "remove-extra-files")]
    pub(crate) remove_extra_files: bool,
    #[arg(long)]
    pub(crate) metadata_only: bool,
    #[arg(long = "reuse-snapshot")]
    pub(crate) source_snapshot: Option<String>,
    #[arg(long = "reuse-from", value_parser = parse_cli_path)]
    pub(crate) source_dir: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) runtime: RuntimeArgs,
}

#[derive(Subcommand)]
pub(crate) enum InstallationCommands {
    /// Reconstruct a verified tree, replacing the entire destination including extra files.
    #[command(mut_group("FetchSourceArgs", |group| group.requires("fetch")))]
    Reconstruct(Box<ReconstructArgs>),

    /// Preview installation changes without modifying files.
    Preview(PreviewArgs),

    /// Verify expected content hashes and report missing, corrupt and extra entries.
    Verify {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long = "snapshot")]
        snapshot_id: String,
        #[arg(short, long, value_parser = parse_cli_path)]
        destination: PathBuf,
        /// Additionally verify chunk hashes. Default installation verification hashes full files.
        #[arg(long = "strict")]
        strict: bool,
        #[command(flatten)]
        runtime: RuntimeArgs,
    },
    /// Repair missing or corrupt content, preserving extra files unless removal is requested.
    Repair {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long = "snapshot")]
        snapshot_id: String,
        #[arg(short, long, value_parser = parse_cli_path)]
        destination: PathBuf,
        #[arg(long = "staging-dir", value_parser = parse_cli_path)]
        staging_dir: Option<PathBuf>,
        /// Remove entries absent from the target snapshot. Repair preserves them by default.
        #[arg(long = "remove-extra-files")]
        remove_extra_files: bool,
        #[command(flatten)]
        runtime: RuntimeArgs,
        #[command(flatten)]
        confirm: ConfirmArgs,
    },
}
