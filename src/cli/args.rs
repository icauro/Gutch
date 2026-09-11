use std::path::PathBuf;

use clap::Args;
use clap::Parser;
use clap::Subcommand;
use clap::ValueEnum;

pub(crate) fn parse_absolute_path(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value.trim());
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(String::from("path must be absolute"))
    }
}

#[derive(Debug, Clone, Copy, Args)]
pub(crate) struct RuntimeArgs {
    #[arg(long = "jobs")]
    pub(crate) jobs: Option<usize>,
}

impl RuntimeArgs {
    pub(crate) fn hints(self) -> crate::api::RuntimeResourceHints {
        crate::api::RuntimeResourceHints { jobs: self.jobs }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum TransferPolicyArg {
    Fixed,
    Adaptive,
}

#[derive(Debug, Clone, Copy, Args)]
pub(crate) struct TransferArgs {
    #[arg(
        long = "transfer-policy",
        value_enum,
        default_value_t = TransferPolicyArg::Fixed
    )]
    pub(crate) policy: TransferPolicyArg,
    #[arg(
        long = "transfer-concurrency",
        default_value_t = crate::installation::DEFAULT_TRANSFER_CONCURRENCY
    )]
    pub(crate) concurrency: usize,
    #[arg(
        long = "transfer-max-concurrency",
        default_value_t = crate::installation::DEFAULT_ADAPTIVE_TRANSFER_MAXIMUM
    )]
    pub(crate) maximum: usize,
}

impl TransferArgs {
    pub(crate) fn policy(self) -> crate::api::TransferConcurrencyPolicy {
        match self.policy {
            TransferPolicyArg::Fixed => {
                crate::api::TransferConcurrencyPolicy::fixed(self.concurrency)
            }
            TransferPolicyArg::Adaptive => {
                crate::api::TransferConcurrencyPolicy::adaptive(self.concurrency, self.maximum)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Args)]
pub(crate) struct ArtifactBuildProfileArgs {
    #[arg(short = 'C', long = "chunk-profile", value_enum, default_value_t = crate::manifest::ArtifactSizingPreset::Chunk128KiB)]
    pub(crate) chunk_profile: crate::manifest::ArtifactSizingPreset,
    #[arg(short = 'B', long = "bundle-profile", value_enum, default_value_t = crate::manifest::BundleSizingPreset::Bundle8MiB)]
    pub(crate) bundle_profile: crate::manifest::BundleSizingPreset,
    #[arg(long = "compression-algorithm", value_enum, default_value_t = crate::manifest::CompressionKind::Zstd)]
    pub(crate) compression_algorithm: crate::manifest::CompressionKind,
    #[arg(long = "compression-level")]
    pub(crate) compression_level: Option<i32>,
    #[arg(long = "storage-mode", value_enum, default_value_t = crate::manifest::ArtifactStorageMode::Bundled)]
    pub(crate) storage_mode: crate::manifest::ArtifactStorageMode,
}

#[derive(Parser)]
#[command(
    name = "gutch",
    version,
    about = "Generic Gutch artifact and repo engine"
)]
pub(crate) struct Cli {
    #[arg(long, short = 'v', global = true)]
    pub(crate) verbose: bool,

    #[arg(long = "trace-out", global = true, value_parser = parse_absolute_path)]
    pub(crate) trace_out: Option<PathBuf>,

    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    Repo {
        #[command(subcommand)]
        command: RepoCommands,
    },
    Snapshot {
        #[command(subcommand)]
        command: SnapshotCommands,
    },
    Reconstructed {
        #[command(subcommand)]
        command: ReconstructedCommands,
    },
    Bundle {
        #[command(subcommand)]
        command: BundleCommands,
    },
    Path {
        #[command(subcommand)]
        command: PathCommands,
    },
}

#[derive(Subcommand)]
pub(crate) enum RepoCommands {
    /// Verify artifacts and regenerate the chunk map; report only unless --publish is given.
    RegenerateChunkMap {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long)]
        publish: bool,
        #[arg(long = "json-out", value_parser = parse_absolute_path)]
        json_out: Option<PathBuf>,
    },
    Init {
        #[arg(long = "path", value_parser = parse_absolute_path)]
        path: PathBuf,
        #[arg(long = "force")]
        force: bool,
    },
    Push {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "yes")]
        yes: bool,
    },
    Pull {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "remote")]
        remote: Option<String>,
        #[arg(long = "on-conflict", default_value = "keep-newer")]
        on_conflict: String,
        #[arg(long = "prune")]
        prune: bool,
        #[arg(long = "yes")]
        yes: bool,
    },
    Remote {
        #[command(subcommand)]
        command: RemoteCommands,
    },
    Link {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "scope-id")]
        scope_id: String,
    },
    Bind {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "remote-repo-id")]
        remote_repo_id: String,
    },
    Unlink {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "delete-remote")]
        delete_remote: bool,
    },
    Name {
        #[command(subcommand)]
        command: RepoNameCommands,
    },
    Tags {
        #[command(subcommand)]
        command: RepoTagsCommands,
    },
    #[command(name = "status")]
    Status {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "json-out", value_parser = parse_absolute_path)]
        json_out: Option<PathBuf>,
    },
    Verify {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "snapshot-id")]
        snapshot_id: Option<String>,
        #[arg(long = "strict")]
        strict: bool,
        #[arg(long = "json-out", value_parser = parse_absolute_path)]
        json_out: Option<PathBuf>,
    },
    #[command(name = "sources")]
    Sources {
        #[command(subcommand)]
        command: SnapshotSourcesCommands,
    },
}

#[derive(Subcommand)]
pub(crate) enum RemoteCommands {
    List,
}

#[derive(Subcommand)]
pub(crate) enum RepoNameCommands {
    Set {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "name")]
        name: String,
    },
}

#[derive(Subcommand)]
pub(crate) enum RepoTagsCommands {
    References {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "json-out", value_parser = parse_absolute_path)]
        json_out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub(crate) enum SnapshotSourcesCommands {
    #[command(name = "capture")]
    Capture {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "source-root", value_parser = parse_absolute_path)]
        source_root: PathBuf,
        #[arg(long = "yes")]
        yes: bool,
        #[arg(long = "json-out", value_parser = parse_absolute_path)]
        json_out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub(crate) enum SnapshotCommands {
    #[command(name = "capture")]
    Capture {
        #[arg(short, long = "source-dir", value_parser = parse_absolute_path)]
        source_dirs: Vec<PathBuf>,
        #[arg(long = "source-root", value_parser = parse_absolute_path)]
        source_root: Option<PathBuf>,
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[command(flatten)]
        build_profile: ArtifactBuildProfileArgs,
        #[command(flatten)]
        runtime: RuntimeArgs,
        #[arg(long = "stats-json-out", value_parser = parse_absolute_path)]
        stats_json_out: Option<PathBuf>,
        #[arg(long = "yes")]
        yes: bool,
    },
    Delete {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "snapshot-id")]
        snapshot_id: String,
        #[arg(long = "yes")]
        yes: bool,
    },
    #[command(name = "retag")]
    Retag {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "snapshot-id")]
        snapshot_id: String,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long = "yes")]
        yes: bool,
    },
    List {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "json-out", value_parser = parse_absolute_path)]
        json_out: Option<PathBuf>,
    },
    #[command(name = "reconstruct")]
    Reconstruct {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "snapshot-id")]
        snapshot_id: String,
        #[arg(short, long, value_parser = parse_absolute_path)]
        destination: PathBuf,
        #[arg(long = "staging-dir", value_parser = parse_absolute_path)]
        staging_dir: Option<PathBuf>,
        #[arg(long = "source-snapshot-id")]
        source_snapshot_id: Option<String>,
        #[arg(long = "source-dir", value_parser = parse_absolute_path)]
        source_dir: Option<PathBuf>,
        #[command(flatten)]
        runtime: RuntimeArgs,
        #[arg(long = "stats-json-out", value_parser = parse_absolute_path)]
        stats_json_out: Option<PathBuf>,
    },
    #[command(name = "fetch-reconstruct")]
    FetchReconstruct {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "snapshot-id")]
        snapshot_id: String,
        #[arg(short, long, value_parser = parse_absolute_path)]
        destination: PathBuf,
        #[arg(long = "staging-dir", value_parser = parse_absolute_path)]
        staging_dir: Option<PathBuf>,
        #[arg(long = "source-snapshot-id")]
        source_snapshot_id: Option<String>,
        #[arg(long = "source-dir", value_parser = parse_absolute_path)]
        source_dir: Option<PathBuf>,
        #[arg(long = "metadata-url")]
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
        #[arg(long = "bundle")]
        bundles: Vec<String>,
        #[arg(long = "artifact-url-manifest", value_parser = parse_absolute_path)]
        artifact_url_manifest: Option<PathBuf>,
        #[command(flatten)]
        runtime: RuntimeArgs,
        #[command(flatten)]
        transfer: TransferArgs,
        #[arg(long = "stats-json-out", value_parser = parse_absolute_path)]
        stats_json_out: Option<PathBuf>,
    },
    Estimate {
        #[arg(short, long = "source-dir", value_parser = parse_absolute_path)]
        source_dir: PathBuf,
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: Option<PathBuf>,
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
    Fetch {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "snapshot-id")]
        snapshot_id: String,
        #[arg(long = "metadata-url")]
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
        #[arg(long = "bundle")]
        bundles: Vec<String>,
        #[arg(long = "artifact-url-manifest", value_parser = parse_absolute_path)]
        artifact_url_manifest: Option<PathBuf>,
        #[command(flatten)]
        transfer: TransferArgs,
    },
}

#[derive(Subcommand)]
pub(crate) enum ReconstructedCommands {
    Verify {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "snapshot-id")]
        snapshot_id: String,
        #[arg(short, long, value_parser = parse_absolute_path)]
        destination: PathBuf,
        #[arg(long = "strict")]
        strict: bool,
        #[command(flatten)]
        runtime: RuntimeArgs,
        #[arg(long = "stats-json-out", value_parser = parse_absolute_path)]
        stats_json_out: Option<PathBuf>,
    },
    Repair {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "snapshot-id")]
        snapshot_id: String,
        #[arg(short, long, value_parser = parse_absolute_path)]
        destination: PathBuf,
        #[arg(long = "staging-dir", value_parser = parse_absolute_path)]
        staging_dir: Option<PathBuf>,
        #[arg(long = "remove-added-files")]
        remove_added_files: bool,
        #[command(flatten)]
        runtime: RuntimeArgs,
        #[arg(long = "stats-json-out", value_parser = parse_absolute_path)]
        stats_json_out: Option<PathBuf>,
        #[arg(long = "yes")]
        yes: bool,
    },
    Corrupt {
        #[arg(short, long, value_parser = parse_absolute_path)]
        destination: PathBuf,
        #[arg(long = "plan", value_parser = parse_absolute_path)]
        plan: PathBuf,
        #[arg(long = "stats-json-out", value_parser = parse_absolute_path)]
        stats_json_out: Option<PathBuf>,
        #[arg(long = "yes")]
        yes: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use super::Commands;
    use super::ReconstructedCommands;
    use super::RemoteCommands;
    use super::RepoCommands;
    use super::RepoTagsCommands;
    use super::SnapshotCommands;
    use super::TransferPolicyArg;
    use clap::Parser;

    #[test]
    fn parses_provider_neutral_remote_commands() {
        let cli = Cli::try_parse_from([
            "gutch",
            "repo",
            "pull",
            "--repo",
            if cfg!(windows) {
                r"D:\Repos\reconstruct"
            } else {
                "/repos/reconstruct"
            },
            "--remote",
            "scope/repo",
            "--on-conflict",
            "keep-local",
            "--prune",
            "--yes",
        ])
        .expect("remote pull args should parse");

        assert!(matches!(
            cli.command,
            Commands::Repo {
                command: RepoCommands::Pull {
                    prune: true,
                    yes: true,
                    ..
                }
            }
        ));
    }

    #[test]
    fn parses_generic_scope_and_tag_reference_commands() {
        let cli = Cli::try_parse_from([
            "gutch",
            "repo",
            "link",
            "--repo",
            if cfg!(windows) {
                r"D:\Repos\creator"
            } else {
                "/repos/creator"
            },
            "--scope-id",
            "creator-scope",
        ])
        .expect("generic link args should parse");
        assert!(matches!(
            cli.command,
            Commands::Repo {
                command: RepoCommands::Link { scope_id, .. }
            } if scope_id == "creator-scope"
        ));

        let cli = Cli::try_parse_from([
            "gutch",
            "repo",
            "bind",
            "--repo",
            if cfg!(windows) {
                r"D:\Repos\creator"
            } else {
                "/repos/creator"
            },
            "--remote-repo-id",
            "remote-repository",
        ])
        .expect("generic bind args should parse");
        assert!(matches!(
            cli.command,
            Commands::Repo {
                command: RepoCommands::Bind { remote_repo_id, .. }
            } if remote_repo_id == "remote-repository"
        ));

        let cli = Cli::try_parse_from([
            "gutch",
            "repo",
            "tags",
            "references",
            "--repo",
            if cfg!(windows) {
                r"D:\Repos\creator"
            } else {
                "/repos/creator"
            },
        ])
        .expect("tag reference args should parse");
        assert!(matches!(
            cli.command,
            Commands::Repo {
                command: RepoCommands::Tags {
                    command: RepoTagsCommands::References { .. }
                }
            }
        ));

        let cli = Cli::try_parse_from(["gutch", "repo", "remote", "list"])
            .expect("remote list args should parse");
        assert!(matches!(
            cli.command,
            Commands::Repo {
                command: RepoCommands::Remote {
                    command: RemoteCommands::List
                }
            }
        ));
    }

    #[test]
    fn parses_fetch_reconstruct_command() {
        let cli = Cli::try_parse_from([
            "gutch",
            "snapshot",
            "fetch-reconstruct",
            "--repo",
            if cfg!(windows) {
                r"D:\Repos\fetched"
            } else {
                "/repos/fetched"
            },
            "--snapshot-id",
            "snapshot-1",
            "--destination",
            if cfg!(windows) {
                r"D:\Installs\product"
            } else {
                "/installs/product"
            },
            "--metadata-url",
            "https://objects.example/snapshot.json",
            "--blueprint-url",
            "https://objects.example/blueprint.gtcbin",
            "--transfer-policy",
            "adaptive",
            "--transfer-concurrency",
            "4",
            "--transfer-max-concurrency",
            "12",
        ])
        .expect("fetch-reconstruct args should parse");

        let Commands::Snapshot {
            command: SnapshotCommands::FetchReconstruct { transfer, .. },
        } = cli.command
        else {
            panic!("expected fetch-reconstruct command");
        };
        assert!(matches!(transfer.policy, TransferPolicyArg::Adaptive));
        assert_eq!(transfer.concurrency, 4);
        assert_eq!(transfer.maximum, 12);
    }

    #[test]
    fn parses_reconstructed_destination_commands() {
        let destination = if cfg!(windows) {
            r"D:\Installs\product"
        } else {
            "/installs/product"
        };
        let repo = if cfg!(windows) {
            r"D:\Repos\product"
        } else {
            "/repos/product"
        };

        let cli = Cli::try_parse_from([
            "gutch",
            "reconstructed",
            "verify",
            "--repo",
            repo,
            "--snapshot-id",
            "snapshot-1",
            "--destination",
            destination,
        ])
        .expect("reconstructed verify args should parse");
        assert!(matches!(
            cli.command,
            Commands::Reconstructed {
                command: ReconstructedCommands::Verify { strict: false, .. }
            }
        ));

        let cli = Cli::try_parse_from([
            "gutch",
            "reconstructed",
            "repair",
            "--repo",
            repo,
            "--snapshot-id",
            "snapshot-1",
            "--destination",
            destination,
            "--remove-added-files",
            "--yes",
        ])
        .expect("reconstructed repair args should parse");
        assert!(matches!(
            cli.command,
            Commands::Reconstructed {
                command: ReconstructedCommands::Repair {
                    remove_added_files: true,
                    yes: true,
                    ..
                }
            }
        ));

        let plan = if cfg!(windows) {
            r"D:\Plans\corrupt.json"
        } else {
            "/plans/corrupt.json"
        };
        let cli = Cli::try_parse_from([
            "gutch",
            "reconstructed",
            "corrupt",
            "--destination",
            destination,
            "--plan",
            plan,
            "--yes",
        ])
        .expect("reconstructed corrupt args should parse");
        assert!(matches!(
            cli.command,
            Commands::Reconstructed {
                command: ReconstructedCommands::Corrupt { yes: true, .. }
            }
        ));
    }
}

#[derive(Subcommand)]
pub(crate) enum BundleCommands {
    Inspect {
        #[arg(short, long, value_parser = parse_absolute_path)]
        bundle: PathBuf,
        #[arg(short, long, value_parser = parse_absolute_path)]
        repo: PathBuf,
    },
}

#[derive(Subcommand)]
pub(crate) enum PathCommands {
    Compare {
        #[arg(long = "expected", value_parser = parse_absolute_path)]
        expected: PathBuf,
        #[arg(long = "actual", value_parser = parse_absolute_path)]
        actual: PathBuf,
        #[arg(long = "stats-json-out", value_parser = parse_absolute_path)]
        stats_json_out: Option<PathBuf>,
    },
}

#[cfg(test)]
mod path_tests {
    use super::Cli;
    use clap::Parser;

    #[test]
    fn cli_rejects_relative_filesystem_paths() {
        let error =
            Cli::try_parse_from(["hostapp-cli", "repo", "status", "--repo", "relative/repo"])
                .err()
                .expect("relative repo path must be rejected");
        assert!(error.to_string().contains("path must be absolute"));
    }

    #[test]
    fn cli_accepts_absolute_filesystem_paths() {
        let repo = if cfg!(windows) {
            r"D:\Repos\game"
        } else {
            "/repos/game"
        };
        Cli::try_parse_from(["gutch", "repo", "status", "--repo", repo])
            .expect("absolute repo path");
    }

    #[test]
    fn cli_accepts_absolute_trace_output_and_rejects_relative_output() {
        let trace_path = if cfg!(windows) {
            r"D:\Artifacts\gutch.trace.json"
        } else {
            "/artifacts/gutch.trace.json"
        };
        let cli =
            Cli::try_parse_from(["gutch", "repo", "remote", "list", "--trace-out", trace_path])
                .expect("absolute trace path");
        assert_eq!(
            cli.trace_out.as_deref().unwrap().to_string_lossy(),
            trace_path
        );

        let error = Cli::try_parse_from([
            "gutch",
            "repo",
            "remote",
            "list",
            "--trace-out",
            "relative.trace.json",
        ])
        .err()
        .expect("relative trace path must be rejected");
        assert!(error.to_string().contains("path must be absolute"));
    }
}
