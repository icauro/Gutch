//! `snapshot` command arguments.

use std::path::PathBuf;

use clap::Args;
use clap::Subcommand;

use super::shared::{
    ArtifactBuildProfileArgs, ConfirmArgs, FetchSourceArgs, RuntimeArgs, TransferArgs,
    parse_cli_path,
};

#[derive(Subcommand)]
pub(crate) enum SnapshotCommands {
    /// Capture source trees as deduplicated, compressed snapshots.
    Capture(CaptureArgs),
    /// Measure fresh-capture storage with default packaging presets and no repository reuse.
    Measure {
        /// Directory or file to measure.
        #[arg(long = "source", value_name = "PATH", value_parser = parse_cli_path)]
        source: PathBuf,
        /// Snapshot tag, as KEY=VALUE; repeatable.
        #[arg(long = "tag", value_name = "KEY=VALUE")]
        tags: Vec<String>,
    },
    /// List stored snapshots.
    List {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        repo: PathBuf,
    },
    /// Compare stored snapshot trees without reconstructing files.
    Diff {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        repo: PathBuf,
        /// Snapshot to compare from.
        #[arg(long, value_name = "ID")]
        from: String,
        /// Snapshot to compare to.
        #[arg(long, value_name = "ID")]
        to: String,
    },
    /// Download snapshot data and artifacts into a repository without reconstructing files.
    Fetch(FetchArgs),
    /// Delete a snapshot and the artifacts only it uses.
    Delete {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        repo: PathBuf,
        /// Snapshot to delete.
        #[arg(long = "snapshot", value_name = "ID")]
        snapshot_id: String,
        #[command(flatten)]
        confirm: ConfirmArgs,
    },
    /// Replace snapshot tags without recapturing content.
    Tags {
        #[command(subcommand)]
        command: SnapshotTagsCommands,
    },
}

#[derive(Args)]
pub(crate) struct CaptureArgs {
    /// Repository directory.
    #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
    pub(crate) repo: PathBuf,
    /// Directory or file to capture; repeatable.
    #[arg(long = "source", value_name = "PATH", value_parser = parse_cli_path)]
    pub(crate) sources: Vec<PathBuf>,
    /// Capture every configured source found under DIR.
    #[arg(long = "discover-from", value_name = "DIR", value_parser = parse_cli_path)]
    pub(crate) discover_from: Option<PathBuf>,
    /// Snapshot tag, as KEY=VALUE; repeatable.
    #[arg(long = "tag", value_name = "KEY=VALUE")]
    pub(crate) tags: Vec<String>,
    /// Reuse content whose bundles only the bound remote holds, as in a copy
    /// pulled with --metadata-only.
    #[arg(long)]
    pub(crate) reuse_remote_bundles: bool,
    #[command(flatten)]
    pub(crate) build_profile: ArtifactBuildProfileArgs,
    #[command(flatten)]
    pub(crate) runtime: RuntimeArgs,
    #[command(flatten)]
    pub(crate) confirm: ConfirmArgs,
}

#[derive(Args)]
pub(crate) struct FetchArgs {
    /// Repository directory.
    #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
    pub(crate) repo: PathBuf,
    /// Snapshot to download.
    #[arg(long = "snapshot", value_name = "ID")]
    pub(crate) snapshot_id: String,
    /// Authorized URL of the snapshot metadata.
    #[arg(long = "snapshot-metadata-url", value_name = "URL")]
    pub(crate) metadata_url: String,
    /// Authorized URL of the snapshot blueprint.
    #[arg(long = "blueprint-url", value_name = "URL")]
    pub(crate) blueprint_url: String,
    #[command(flatten)]
    pub(crate) sources: FetchSourceArgs,
    #[command(flatten)]
    pub(crate) transfer: TransferArgs,
}

#[derive(Subcommand)]
pub(crate) enum SnapshotTagsCommands {
    /// Replace the complete tag set; omitted keys are removed.
    Replace {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        repo: PathBuf,
        /// Snapshot whose tags to replace.
        #[arg(
            long = "snapshot",
            value_name = "ID",
            required_unless_present = "tags_file",
            conflicts_with = "tags_file"
        )]
        snapshot_id: Option<String>,
        /// New tag, as KEY=VALUE; repeatable.
        #[arg(long = "tag", value_name = "KEY=VALUE", conflicts_with = "tags_file")]
        tags: Vec<String>,
        /// JSON object mapping snapshot IDs to complete {"key_value": {...}} tag sets.
        #[arg(long, value_name = "FILE", value_parser = parse_cli_path)]
        tags_file: Option<PathBuf>,
        #[command(flatten)]
        confirm: ConfirmArgs,
    },
}
