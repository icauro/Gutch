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
    /// Shared working-memory budget in MiB (default: 25% of usable RAM).
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=u64::MAX / (1024 * 1024)))]
    pub(crate) memory_budget_mib: Option<u64>,
}

impl RuntimeArgs {
    pub(crate) fn hints(self) -> crate::api::RuntimeResourceHints {
        crate::api::RuntimeResourceHints {
            jobs: self.jobs,
            memory_budget_bytes: self
                .memory_budget_mib
                .and_then(|mib| std::num::NonZeroU64::new(mib * 1024 * 1024)),
        }
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
    /// Content-defined chunker; JC uses the pinned Plakar-compatible variant.
    #[arg(long, value_enum, default_value_t = crate::manifest::Chunker::FastCdc)]
    pub(crate) chunker: crate::manifest::Chunker,
    /// Chunk sizing preset (algorithm-specific target, not a fixed chunk size).
    #[arg(short = 'C', long = "chunk-preset", value_enum, default_value_t = crate::manifest::ChunkSizingPreset::Chunk128KiB)]
    pub(crate) chunk_profile: crate::manifest::ChunkSizingPreset,
    /// Bundle sizing preset.
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
    about = "Capture snapshots and reconstruct verified installation trees"
)]
pub(crate) struct Cli {
    /// Write structured failure details, including every filename violation.
    #[arg(long = "error-json-out", global = true, value_parser = parse_cli_path)]
    pub(crate) error_json_out: Option<PathBuf>,
    #[arg(long, short = 'v', global = true)]
    pub(crate) verbose: bool,

    /// Diagnostic profiling: off, coarse summary, or sampled detailed timelines.
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
pub(crate) enum RepoCommands {
    /// Show or change filename compatibility, tag rules and matching tag keys.
    Constraints {
        #[command(subcommand)]
        command: ConstraintCommands,
    },
    /// Inspect or rebuild the repository chunk index.
    Index {
        #[command(subcommand)]
        command: RepoIndexCommands,
    },
    Init {
        /// JSON file containing the complete repository constraints object.
        #[arg(long = "constraints-file", value_parser = parse_cli_path, conflicts_with = "targets")]
        constraints: Option<PathBuf>,
        #[arg(long = "filesystem", value_enum, value_delimiter = ',')]
        targets: Vec<crate::manifest::constraints::FilesystemTarget>,
        #[arg(long = "repo", value_parser = parse_cli_path)]
        path: PathBuf,
        #[arg(long = "force")]
        force: bool,
    },
    /// Synchronize local repository state to its bound remote.
    Push {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long = "yes")]
        yes: bool,
    },
    /// Synchronize remote repository data locally; does not update installations.
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
    /// Change the local display name, leaving the directory and remote name unchanged.
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
    /// Show local repository state.
    #[command(name = "status")]
    Status {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(skip)]
        json_out: Option<PathBuf>,
    },
    /// Verify stored repository metadata and artifacts.
    Verify {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long = "snapshot")]
        snapshot_id: Option<String>,
        /// Use strict artifact verification.
        #[arg(long = "strict")]
        strict: bool,
        #[arg(skip)]
        json_out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub(crate) enum RepoIndexCommands {
    /// Verify artifacts and rebuild the chunk index; report only unless --apply is given.
    Rebuild {
        #[arg(long = "repo", value_parser = parse_cli_path)]
        repo: PathBuf,
        /// Replace the local chunk index after a complete, successful rebuild.
        #[arg(long)]
        apply: bool,
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
            required_unless_present = "constraints"
        )]
        targets: Vec<crate::manifest::constraints::FilesystemTarget>,
        /// JSON file containing the complete replacement constraints object.
        #[arg(long = "constraints-file", value_parser = parse_cli_path, conflicts_with = "targets")]
        constraints: Option<PathBuf>,
    },

    Clear {
        #[arg(long, value_parser = parse_cli_path)]
        repo: PathBuf,
    },
}

#[derive(Subcommand)]
pub(crate) enum RemoteCommands {
    List,
    /// Create a remote repository and bind the local repository to it.
    Create {
        #[arg(long, value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long)]
        scope_id: String,
    },
    /// Bind to an existing remote repository without synchronizing content.
    Bind {
        #[arg(long, value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long)]
        remote_repo_id: String,
    },
    /// Remove the binding; retain remote content unless --delete-remote is supplied.
    Unbind {
        #[arg(long, value_parser = parse_cli_path)]
        repo: PathBuf,
        #[arg(long)]
        delete_remote: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum RepoTagsCommands {
    /// Inspect advisory tag references supplied by the remote provider.
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

#[derive(Args)]
pub(crate) struct ReconstructionArgs {
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
    /// Fetch authorized snapshot data and artifacts while reconstructing.
    #[arg(long)]
    pub(crate) fetch: bool,
    #[arg(
        long = "snapshot-metadata-url",
        required_if_eq("fetch", "true"),
        requires = "fetch"
    )]
    pub(crate) metadata_url: Option<String>,
    #[arg(long = "product-metadata-url", requires = "fetch")]
    pub(crate) product_metadata_url: Option<String>,
    #[arg(
        long = "blueprint-url",
        required_if_eq("fetch", "true"),
        requires = "fetch"
    )]
    pub(crate) blueprint_url: Option<String>,
    #[arg(long = "repo-metadata-url", requires = "fetch")]
    pub(crate) repo_metadata_url: Option<String>,
    #[arg(long = "chunk-index-url", requires = "fetch")]
    pub(crate) chunk_index_url: Option<String>,
    #[arg(long = "bundle-downloads-endpoint", requires = "fetch")]
    pub(crate) bundle_downloads_endpoint: Option<String>,
    #[arg(long = "session-id", requires = "fetch")]
    pub(crate) session_id: Option<String>,
    #[arg(long = "artifact-url", requires = "fetch")]
    pub(crate) artifacts: Vec<String>,
    #[arg(long = "artifact-url-manifest", requires = "fetch", value_parser = parse_cli_path)]
    pub(crate) artifact_url_manifest: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) transfer: TransferArgs,
    #[arg(skip)]
    pub(crate) stats_json_out: Option<PathBuf>,
}

#[derive(Subcommand)]
pub(crate) enum InstallationCommands {
    /// Reconstruct a verified tree, replacing the entire destination including extra files.
    Reconstruct(Box<ReconstructionArgs>),

    /// Preview installation changes without modifying files.
    Preview {
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
        /// Remove entries absent from the target snapshot. Repair preserves them by default.
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
        #[arg(skip)]
        stats_json_out: Option<PathBuf>,
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
    fn parses_reconstruction_with_fetch() {
        let cli = Cli::try_parse_from([
            "gutch",
            "installation",
            "reconstruct",
            "--fetch",
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
        .expect("reconstruction fetch args should parse");

        let Commands::Installation {
            command: InstallationCommands::Reconstruct(arguments),
        } = cli.command
        else {
            panic!("expected reconstruction with fetch");
        };
        assert!(arguments.fetch);
        assert!(matches!(
            arguments.transfer.policy,
            TransferPolicyArg::Adaptive
        ));
        assert_eq!(arguments.transfer.concurrency, 4);
        assert_eq!(arguments.transfer.maximum, 12);
        assert_eq!(
            arguments.artifacts,
            ["chunk-1=https://objects.example/chunk"]
        );
    }

    #[test]
    fn rejects_retired_command_group() {
        let error = Cli::try_parse_from(["gutch", "reconstructed", "--help"])
            .err()
            .expect("the retired command group must be rejected");
        assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn reconstruction_requires_explicit_fetch_and_complete_metadata_sources() {
        let base = [
            "gutch",
            "installation",
            "reconstruct",
            "--repo",
            "repo",
            "--snapshot",
            "snapshot",
            "--destination",
            "install",
        ];
        Cli::try_parse_from(base).expect("local reconstruction needs no remote inputs");
        for options in [
            vec!["--fetch"],
            vec![
                "--fetch",
                "--snapshot-metadata-url",
                "https://example.invalid/meta",
            ],
            vec!["--fetch", "--blueprint-url", "https://example.invalid/tree"],
            vec![
                "--snapshot-metadata-url",
                "https://example.invalid/meta",
                "--blueprint-url",
                "https://example.invalid/tree",
            ],
            vec!["--artifact-url", "id=https://example.invalid/artifact"],
        ] {
            let error = Cli::try_parse_from(base.into_iter().chain(options))
                .err()
                .expect("incomplete or implicit fetch must fail during parsing");
            assert_eq!(
                error.kind(),
                clap::error::ErrorKind::MissingRequiredArgument
            );
        }
    }

    #[test]
    fn rejects_retired_operation_names() {
        for command in [
            vec!["snapshot", "reconstruct"],
            vec!["snapshot", "fetch-reconstruct"],
            vec!["snapshot", "estimate"],
            vec!["snapshot", "retag"],
            vec!["installation", "plan"],
            vec!["repo", "rebuild-index"],
            vec!["path", "compare"],
        ] {
            let error = Cli::try_parse_from(std::iter::once("gutch").chain(command))
                .err()
                .expect("retired command names have no aliases");
            assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
        }
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
pub(crate) enum TreeCommands {
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
