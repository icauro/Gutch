use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use gutch as g;

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
    #[arg(long = "memory-mode", value_enum, default_value_t = g::MemoryMode::Auto)]
    pub(crate) memory_mode: g::MemoryMode,
    #[arg(long = "max-memory-mib")]
    pub(crate) max_memory_mib: Option<u64>,
    #[arg(long = "jobs")]
    pub(crate) jobs: Option<usize>,
}

impl RuntimeArgs {
    pub(crate) fn hints(self) -> g::RuntimeResourceHints {
        g::RuntimeResourceHints {
            memory_mode: self.memory_mode,
            max_memory_mib: self.max_memory_mib,
            jobs: self.jobs,
        }
    }
}

#[derive(Debug, Clone, Copy, Args)]
pub(crate) struct ArtifactBuildProfileArgs {
    #[arg(short = 'C', long = "chunk-profile", value_enum, default_value_t = g::manifest::ArtifactSizingPreset::Chunk128KiB)]
    pub(crate) chunk_profile: g::manifest::ArtifactSizingPreset,
    #[arg(short = 'B', long = "bundle-profile", value_enum, default_value_t = g::manifest::BundleSizingPreset::Bundle16MiB)]
    pub(crate) bundle_profile: g::manifest::BundleSizingPreset,
    #[arg(long = "compression-profile", value_enum, default_value_t = g::manifest::CompressionPreset::Zstd3)]
    pub(crate) compression_profile: g::manifest::CompressionPreset,
    #[arg(long = "storage-mode", value_enum, default_value_t = g::manifest::ArtifactStorageMode::Bundled)]
    pub(crate) storage_mode: g::manifest::ArtifactStorageMode,
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
    Init {
        #[arg(long = "path", value_parser = parse_absolute_path)]
        path: PathBuf,
        #[arg(long = "schema", value_parser = parse_absolute_path)]
        schema: Option<PathBuf>,
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
    Schema {
        #[command(subcommand)]
        command: RepoTagSchemaCommands,
    },
    Requirements {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "check")]
        check: bool,
        #[arg(long = "json-out", value_parser = parse_absolute_path)]
        json_out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub(crate) enum RepoTagSchemaCommands {
    Set {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "schema", value_parser = parse_absolute_path)]
        schema: Option<PathBuf>,
        #[arg(long = "clear")]
        clear: bool,
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
        #[arg(long = "no-prune")]
        no_prune: bool,
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
    },
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
}

#[cfg(test)]
mod tests {
    use super::{Cli, Commands, RemoteCommands, RepoCommands, RepoTagsCommands};
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
    fn parses_generic_scope_and_tag_requirement_commands() {
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
            "requirements",
            "--repo",
            if cfg!(windows) {
                r"D:\Repos\creator"
            } else {
                "/repos/creator"
            },
            "--check",
        ])
        .expect("tag requirement args should parse");
        assert!(matches!(
            cli.command,
            Commands::Repo {
                command: RepoCommands::Tags {
                    command: RepoTagsCommands::Requirements { check: true, .. }
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
        let error = Cli::try_parse_from([
            "hostapp-cli gutch",
            "repo",
            "status",
            "--repo",
            "relative/repo",
        ])
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
}
