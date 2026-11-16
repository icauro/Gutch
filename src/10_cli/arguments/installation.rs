//! `installation` command arguments.

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use super::shared::{ConfirmArgs, FetchSourceArgs, RuntimeArgs, TransferArgs, parse_cli_path};

#[derive(Subcommand)]
pub(crate) enum InstallationCommands {
    /// Preview installation changes without modifying files.
    Preview(PreviewArgs),
    /// Reconstruct a verified tree, replacing the entire destination including extra files.
    #[command(mut_group("FetchSourceArgs", |group| group.requires("fetch")))]
    Reconstruct(Box<ReconstructArgs>),
    /// Verify expected content hashes and report missing, corrupt and extra entries.
    Verify {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        repo: PathBuf,
        /// Snapshot the installation should match.
        #[arg(long = "snapshot", value_name = "ID")]
        snapshot_id: String,
        /// Installation directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        destination: PathBuf,
        /// Additionally verify chunk hashes. Default installation verification hashes full files.
        #[arg(long)]
        strict: bool,
        #[command(flatten)]
        runtime: RuntimeArgs,
    },
    /// Repair missing or corrupt content, preserving extra files unless removal is requested.
    Repair {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        repo: PathBuf,
        /// Snapshot the installation should match.
        #[arg(long = "snapshot", value_name = "ID")]
        snapshot_id: String,
        /// Installation directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        destination: PathBuf,
        /// Directory for staged files; defaults to one beside the destination.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        staging_dir: Option<PathBuf>,
        /// Remove entries absent from the target snapshot. Repair preserves them by default.
        #[arg(long)]
        remove_extra_files: bool,
        #[command(flatten)]
        runtime: RuntimeArgs,
        #[command(flatten)]
        confirm: ConfirmArgs,
    },
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
    /// Repository directory.
    #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
    pub(crate) repo: PathBuf,
    /// Snapshot to install.
    #[arg(long = "snapshot", value_name = "ID")]
    pub(crate) snapshot_id: String,
    /// Installation directory.
    #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
    pub(crate) destination: PathBuf,
    /// Directory for staged files; defaults to one beside the destination.
    #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
    pub(crate) staging_dir: Option<PathBuf>,
    /// Which publication to preview: repair or whole reconstruction.
    #[arg(long = "mode", value_enum, default_value = "selective")]
    pub(crate) publication: PreviewPublication,
    /// Remove entries absent from the target snapshot; they are kept by default.
    #[arg(long)]
    pub(crate) remove_extra_files: bool,
    /// Compare metadata only, without hashing file content.
    #[arg(long)]
    pub(crate) metadata_only: bool,
    /// Installed snapshot whose content may be reused.
    #[arg(long = "reuse-snapshot", value_name = "ID")]
    pub(crate) source_snapshot: Option<String>,
    /// Directory holding the reusable installed snapshot.
    #[arg(long = "reuse-from", value_name = "DIR", value_parser = parse_cli_path)]
    pub(crate) source_dir: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) runtime: RuntimeArgs,
}

#[derive(Args)]
pub(crate) struct ReconstructArgs {
    /// Repository directory.
    #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
    pub(crate) repo: PathBuf,
    /// Snapshot to reconstruct.
    #[arg(long = "snapshot", value_name = "ID")]
    pub(crate) snapshot_id: String,
    /// Installation directory; its current content is replaced.
    #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
    pub(crate) destination: PathBuf,
    /// Directory for staged files; defaults to one beside the destination.
    #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
    pub(crate) staging_dir: Option<PathBuf>,
    /// Installed snapshot whose content may be reused.
    #[arg(long = "reuse-snapshot", value_name = "ID")]
    pub(crate) reuse_snapshot_id: Option<String>,
    /// Directory holding the reusable installed snapshot.
    #[arg(long = "reuse-from", value_name = "DIR", value_parser = parse_cli_path)]
    pub(crate) source_dir: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) runtime: RuntimeArgs,
    /// Show what would change without applying it.
    #[arg(long, conflicts_with = "fetch")]
    pub(crate) dry_run: bool,
    /// Fetch authorized snapshot data and artifacts while reconstructing.
    #[arg(long)]
    pub(crate) fetch: bool,
    /// Authorized URL of the snapshot metadata; requires --fetch.
    #[arg(
        long = "snapshot-metadata-url",
        value_name = "URL",
        required_if_eq("fetch", "true"),
        requires = "fetch"
    )]
    pub(crate) metadata_url: Option<String>,
    /// Authorized URL of the snapshot blueprint; requires --fetch.
    #[arg(
        long = "blueprint-url",
        value_name = "URL",
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
