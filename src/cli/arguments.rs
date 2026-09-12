use std::path::PathBuf;

use clap::Args;
use clap::Parser;
use clap::Subcommand;
use clap::ValueEnum;

pub(crate) fn parse_cli_path(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if path.as_os_str().is_empty() {
        return Err("path must not be empty".into());
    }
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|error| error.to_string())
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
        default_value_t = crate::transfer::types::DEFAULT_TRANSFER_CONCURRENCY
    )]
    pub(crate) concurrency: usize,
    #[arg(
        long = "transfer-max-concurrency",
        default_value_t = crate::transfer::types::DEFAULT_ADAPTIVE_TRANSFER_MAXIMUM
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
    #[arg(short = 'C', long = "chunk-preset", value_enum, default_value_t = crate::manifest::ChunkSizingPreset::Chunk128KiB)]
    pub(crate) chunk_profile: crate::manifest::ChunkSizingPreset,
    #[arg(short = 'B', long = "bundle-preset", value_enum, default_value_t = crate::manifest::BundleSizingPreset::Bundle8MiB)]
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
    /// Write structured failure details, including every filename violation.
    #[arg(long = "error-json-out", global = true, value_parser = parse_cli_path)]
    pub(crate) error_json_out: Option<PathBuf>,
    #[arg(long, short = 'v', global = true)]
    pub(crate) verbose: bool,

    /// Diagnostic collection: off, coarse summary, or sampled detailed timelines.
    #[arg(long, global = true, value_enum, default_value_t = crate::profile::GutchProfileMode::Off)]
    pub(crate) profile: crate::profile::GutchProfileMode,
    /// Absolute report directory; required for detailed, optional for summary.
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
    Repo {
        #[command(subcommand)]
        command: RepoCommands,
    },
    Snapshot {
        #[command(subcommand)]
        command: SnapshotCommands,
    },
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
    Path {
        #[command(subcommand)]
        command: PathCommands,
    },
}

#[derive(Subcommand)]
pub(crate) enum RepoCommands {
    /// Show or change filename compatibility constraints.
    Constraints {
        #[command(subcommand)]
        command: ConstraintCommands,
    },
    /// Verify artifacts and regenerate the chunk map; report only unless --publish is given.
    RebuildIndex {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long)]
        publish: bool,
        #[arg(skip)]
        json_out: Option<PathBuf>,
    },
    Init {
        #[arg(long = "filesystem", value_enum, value_delimiter = ',')]
        targets: Vec<crate::manifest::constraints::FilesystemTarget>,
        #[arg(long = "repo", value_parser = parse_cli_path)]
        path: PathBuf,
        #[arg(long = "force")]
        force: bool,
    },
    Push {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long = "yes")]
        yes: bool,
    },
    Pull {
        #[arg(long = "repo", value_parser = parse_cli_path)]
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
    Rename {
        #[arg(long, value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long)]
        name: String,
    },
    Tags {
        #[command(subcommand)]
        command: RepoTagsCommands,
    },
    #[command(name = "status")]
    Status {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(skip)]
        json_out: Option<PathBuf>,
    },
    Verify {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long = "snapshot")]
        snapshot_id: Option<String>,
        #[arg(long = "strict")]
        strict: bool,
        #[arg(skip)]
        json_out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub(crate) enum ConstraintCommands {
    Show {
        #[arg(long, value_parser = parse_cli_path)]
        repo: PathBuf,
    },
    Set {
        #[arg(long, value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(
            long = "filesystem",
            value_enum,
            value_delimiter = ',',
            required = true
        )]
        targets: Vec<crate::manifest::constraints::FilesystemTarget>,
    },
    Clear {
        #[arg(long, value_parser = parse_cli_path)]
        repo: PathBuf,
    },
}

#[derive(Subcommand)]
pub(crate) enum RemoteCommands {
    List,
    Create {
        #[arg(long, value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long)]
        scope_id: String,
    },
    Bind {
        #[arg(long, value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long)]
        remote_repo_id: String,
    },
    Unbind {
        #[arg(long, value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long)]
        delete_remote: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum RepoTagsCommands {
    References {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(skip)]
        json_out: Option<PathBuf>,
    },
}

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
    #[command(name = "retag")]
    Retag {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long = "snapshot")]
        snapshot_id: String,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long = "yes")]
        yes: bool,
    },
    List {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(skip)]
        json_out: Option<PathBuf>,
    },
    #[command(name = "reconstruct")]
    Reconstruct {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long = "snapshot")]
        snapshot_id: String,
        #[arg(short, long, value_parser = parse_cli_path)]
        destination: PathBuf,
        #[arg(long = "staging-dir", value_parser = parse_cli_path)]
        staging_dir: Option<PathBuf>,
        #[arg(long = "reuse-snapshot")]
        source_snapshot_id: Option<String>,
        #[arg(long = "reuse-from", value_parser = parse_cli_path)]
        source_dir: Option<PathBuf>,
        #[command(flatten)]
        runtime: RuntimeArgs,
        #[arg(skip)]
        stats_json_out: Option<PathBuf>,
    },
    #[command(name = "fetch-reconstruct")]
    FetchReconstruct {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long = "snapshot")]
        snapshot_id: String,
        #[arg(short, long, value_parser = parse_cli_path)]
        destination: PathBuf,
        #[arg(long = "staging-dir", value_parser = parse_cli_path)]
        staging_dir: Option<PathBuf>,
        #[arg(long = "reuse-snapshot")]
        source_snapshot_id: Option<String>,
        #[arg(long = "reuse-from", value_parser = parse_cli_path)]
        source_dir: Option<PathBuf>,
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
        runtime: RuntimeArgs,
        #[command(flatten)]
        transfer: TransferArgs,
        #[arg(skip)]
        stats_json_out: Option<PathBuf>,
    },
    Estimate {
        #[arg(short, long = "source", value_parser = parse_cli_path)]
        source_dir: PathBuf,
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
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
pub(crate) enum InstallationCommands {
    /// Preview installation changes without modifying files.
    Plan {
        #[arg(long, value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long = "snapshot")]
        snapshot_id: String,
        #[arg(long, value_parser = parse_cli_path)]
        destination: PathBuf,
        #[arg(long = "staging-dir", value_parser = parse_cli_path)]
        staging_dir: Option<PathBuf>,
        #[arg(long = "mode", value_parser = ["selective", "replace-directory"], default_value = "selective")]
        publication: String,
        #[arg(long = "remove-extra-files")]
        remove_added_files: bool,
        #[arg(long)]
        metadata_only: bool,
        #[arg(long = "reuse-snapshot")]
        source_snapshot: Option<String>,
        #[arg(long = "reuse-from", value_parser = parse_cli_path)]
        source_dir: Option<PathBuf>,
        #[command(flatten)]
        runtime: RuntimeArgs,
    },

    Verify {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long = "snapshot")]
        snapshot_id: String,
        #[arg(short, long, value_parser = parse_cli_path)]
        destination: PathBuf,
        #[arg(long = "strict")]
        strict: bool,
        #[command(flatten)]
        runtime: RuntimeArgs,
        #[arg(skip)]
        stats_json_out: Option<PathBuf>,
    },
    Repair {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long = "snapshot")]
        snapshot_id: String,
        #[arg(short, long, value_parser = parse_cli_path)]
        destination: PathBuf,
        #[arg(long = "staging-dir", value_parser = parse_cli_path)]
        staging_dir: Option<PathBuf>,
        #[arg(long = "remove-extra-files")]
        remove_added_files: bool,
        #[command(flatten)]
        runtime: RuntimeArgs,
        #[arg(skip)]
        stats_json_out: Option<PathBuf>,
        #[arg(long = "yes")]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum DebugCommands {
    CorruptInstallation {
        #[arg(short, long, value_parser = parse_cli_path)]
        destination: PathBuf,
        #[arg(long = "plan", value_parser = parse_cli_path)]
        plan: PathBuf,
        #[arg(skip)]
        stats_json_out: Option<PathBuf>,
        #[arg(long = "yes")]
        yes: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use super::Commands;
    use super::InstallationCommands;
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
            "remote",
            "create",
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
                command: RepoCommands::Remote { command: RemoteCommands::Create { scope_id, .. } }
            } if scope_id == "creator-scope"
        ));

        let cli = Cli::try_parse_from([
            "gutch",
            "repo",
            "remote",
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
                command: RepoCommands::Remote { command: RemoteCommands::Bind { remote_repo_id, .. } }
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
            "--snapshot",
            "snapshot-1",
            "--destination",
            if cfg!(windows) {
                r"D:\Installs\product"
            } else {
                "/installs/product"
            },
            "--snapshot-metadata-url",
            "https://objects.example/snapshot.json",
            "--blueprint-url",
            "https://objects.example/blueprint.gtcbin",
            "--artifact-url",
            "chunk-1=https://objects.example/chunk",
            "--transfer-policy",
            "adaptive",
            "--transfer-concurrency",
            "4",
            "--transfer-max-concurrency",
            "12",
        ])
        .expect("fetch-reconstruct args should parse");

        let Commands::Snapshot {
            command:
                SnapshotCommands::FetchReconstruct {
                    transfer,
                    artifacts,
                    ..
                },
        } = cli.command
        else {
            panic!("expected fetch-reconstruct command");
        };
        assert!(matches!(transfer.policy, TransferPolicyArg::Adaptive));
        assert_eq!(transfer.concurrency, 4);
        assert_eq!(transfer.maximum, 12);
        assert_eq!(artifacts, ["chunk-1=https://objects.example/chunk"]);
    }

    #[test]
    fn rejects_retired_command_group() {
        let error = Cli::try_parse_from(["gutch", "reconstructed", "--help"])
            .err()
            .expect("the retired command group must be rejected");
        assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn parses_installation_destination_commands() {
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
            "installation",
            "verify",
            "--repo",
            repo,
            "--snapshot",
            "snapshot-1",
            "--destination",
            destination,
        ])
        .expect("installation verify args should parse");
        assert!(matches!(
            cli.command,
            Commands::Installation {
                command: InstallationCommands::Verify { strict: false, .. }
            }
        ));

        let cli = Cli::try_parse_from([
            "gutch",
            "installation",
            "repair",
            "--repo",
            repo,
            "--snapshot",
            "snapshot-1",
            "--destination",
            destination,
            "--remove-extra-files",
            "--yes",
        ])
        .expect("installation repair args should parse");
        assert!(matches!(
            cli.command,
            Commands::Installation {
                command: InstallationCommands::Repair {
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
            "debug",
            "corrupt-installation",
            "--destination",
            destination,
            "--plan",
            plan,
            "--yes",
        ])
        .expect("installation corrupt args should parse");
        assert!(matches!(
            cli.command,
            Commands::Debug {
                command: super::DebugCommands::CorruptInstallation { yes: true, .. }
            }
        ));
    }
}

#[derive(Subcommand)]
pub(crate) enum BundleCommands {
    Inspect {
        #[arg(short, long, value_parser = parse_cli_path)]
        bundle: PathBuf,
        #[arg(short, long, value_parser = parse_cli_path)]
        repo: PathBuf,
    },
}

#[derive(Subcommand)]
pub(crate) enum PathCommands {
    Compare {
        #[arg(long = "expected", value_parser = parse_cli_path)]
        expected: PathBuf,
        #[arg(long = "actual", value_parser = parse_cli_path)]
        actual: PathBuf,
        #[arg(skip)]
        stats_json_out: Option<PathBuf>,
    },
}

#[cfg(test)]
mod path_tests {
    use super::Cli;
    use clap::Parser;

    #[test]
    fn cli_resolves_relative_filesystem_paths() {
        let cli = Cli::try_parse_from(["gutch-cli", "repo", "status", "--repo", "relative/repo"])
            .unwrap();
        let super::Commands::Repo {
            command: super::RepoCommands::Status { repo, .. },
        } = cli.command
        else {
            panic!("expected status");
        };
        assert_eq!(repo, std::env::current_dir().unwrap().join("relative/repo"));
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
    fn cli_resolves_absolute_and_relative_profile_output() {
        let trace_path = if cfg!(windows) {
            r"D:\Artifacts\gutch.trace.json"
        } else {
            "/artifacts/gutch.trace.json"
        };
        let cli = Cli::try_parse_from([
            "gutch",
            "repo",
            "remote",
            "list",
            "--profile-dir",
            trace_path,
        ])
        .expect("absolute trace path");
        assert_eq!(
            cli.profile_out.as_deref().unwrap().to_string_lossy(),
            trace_path
        );

        let cli = Cli::try_parse_from([
            "gutch",
            "repo",
            "remote",
            "list",
            "--profile-dir",
            "relative.trace.json",
        ])
        .unwrap();
        assert_eq!(
            cli.profile_out.unwrap(),
            std::env::current_dir().unwrap().join("relative.trace.json")
        );
    }
}
