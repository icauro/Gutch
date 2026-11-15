//! `snapshot` command arguments.

use std::path::PathBuf;

use clap::Subcommand;

use super::{ArtifactBuildProfileArgs, RuntimeArgs, TransferArgs, parse_cli_path};

#[derive(Subcommand)]
pub(crate) enum SnapshotCommands {
    /// Compare stored snapshot trees without reconstructing files.
    Diff {
        #[arg(long, value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
    },
    /// Capture source trees as deduplicated, compressed snapshots.
    #[command(name = "capture")]
    Capture {
        #[arg(short, long = "source", value_parser = parse_cli_path)]
        source_dirs: Vec<PathBuf>,
        #[arg(long = "discover-from", value_parser = parse_cli_path)]
        source_root: Option<PathBuf>,
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[command(flatten)]
        build_profile: ArtifactBuildProfileArgs,
        #[command(flatten)]
        runtime: RuntimeArgs,
        #[arg(skip)]
        stats_json_out: Option<PathBuf>,
        #[arg(long = "yes")]
        yes: bool,
    },
    Delete {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long = "snapshot")]
        snapshot_id: String,
        #[arg(long = "yes")]
        yes: bool,
    },
    /// Replace snapshot tags without recapturing content.
    Tags {
        #[command(subcommand)]
        command: SnapshotTagsCommands,
    },
    List {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(skip)]
        json_out: Option<PathBuf>,
    },
    /// Measure fresh-capture storage with default packaging presets and no repository reuse.
    Measure {
        #[arg(short, long = "source", value_parser = parse_cli_path)]
        source_dir: PathBuf,
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
    /// Download snapshot data and artifacts into a repository without reconstructing files.
    Fetch {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long = "snapshot")]
        snapshot_id: String,
        #[arg(long = "snapshot-metadata-url")]
        metadata_url: String,
        #[arg(long = "product-metadata-url")]
        product_metadata_url: Option<String>,
        #[arg(long = "blueprint-url")]
        blueprint_url: String,
        #[arg(long = "repo-metadata-url")]
        repo_metadata_url: Option<String>,
        #[arg(long = "chunk-index-url")]
        chunk_index_url: Option<String>,
        #[arg(long = "bundle-downloads-endpoint")]
        bundle_downloads_endpoint: Option<String>,
        #[arg(long = "session-id")]
        session_id: Option<String>,
        #[arg(long = "artifact-url")]
        artifacts: Vec<String>,
        #[arg(long = "artifact-url-manifest", value_parser = parse_cli_path)]
        artifact_url_manifest: Option<PathBuf>,
        #[command(flatten)]
        transfer: TransferArgs,
    },
}

#[derive(Subcommand)]
pub(crate) enum SnapshotTagsCommands {
    /// Replace the complete tag set; omitted keys are removed.
    Replace {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(
            long = "snapshot",
            required_unless_present = "tags_file",
            conflicts_with = "tags_file"
        )]
        snapshot_id: Option<String>,
        #[arg(long = "tag", conflicts_with = "tags_file")]
        tags: Vec<String>,
        /// JSON object mapping snapshot IDs to complete {"key_value": {...}} tag sets.
        #[arg(long, value_parser = parse_cli_path)]
        tags_file: Option<PathBuf>,
        #[arg(long = "yes")]
        yes: bool,
    },
}
