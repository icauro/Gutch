use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use gutch as g;

fn parse_absolute_path(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value.trim());
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(String::from("path must be absolute"))
    }
}

#[derive(Debug, Clone, Copy, Args)]
pub struct RuntimeArgs {
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
pub struct ArtifactBuildProfileArgs {
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
    about = "Generic Gutch artifact and repository engine"
)]
pub struct Cli {
    #[arg(long, short = 'v', global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
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
pub enum RepoCommands {
    Init {
        #[arg(long = "path", value_parser = parse_absolute_path)]
        path: PathBuf,
        #[arg(long = "schema", value_parser = parse_absolute_path)]
        schema: Option<PathBuf>,
        #[arg(long = "force")]
        force: bool,
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
pub enum RepoNameCommands {
    Set {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "name")]
        name: String,
    },
}

#[derive(Subcommand)]
pub enum RepoTagsCommands {
    Schema {
        #[command(subcommand)]
        command: RepoTagSchemaCommands,
    },
}

#[derive(Subcommand)]
pub enum RepoTagSchemaCommands {
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
pub enum SnapshotSourcesCommands {
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
pub enum SnapshotCommands {
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

#[derive(Subcommand)]
pub enum BundleCommands {
    Inspect {
        #[arg(short, long, value_parser = parse_absolute_path)]
        bundle: PathBuf,
        #[arg(short, long, value_parser = parse_absolute_path)]
        repo: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum PathCommands {
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
mod tests {
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
        .expect("relative repository path must be rejected");
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
            .expect("absolute repository path");
    }
}
