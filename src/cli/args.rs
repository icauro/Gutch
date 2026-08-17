use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand};
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
    name = "hostapp-cli gutch",
    version,
    about = hostapp_common::GUTCH_ABOUT
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
    Target {
        #[command(subcommand)]
        command: TargetCommands,
    },
    #[command(name = "target_install")]
    TargetInstall {
        #[command(subcommand)]
        command: TargetInstallCommands,
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
    #[command(name = "rename")]
    Rename {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "name")]
        name: String,
    },
    #[command(name = "schema-set")]
    SchemaSet {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "schema", value_parser = parse_absolute_path)]
        schema: Option<PathBuf>,
        #[arg(long = "clear")]
        clear: bool,
    },
    #[command(name = "refresh")]
    Refresh {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "json-out", value_parser = parse_absolute_path)]
        json_out: Option<PathBuf>,
    },
    Verify {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "target")]
        target: Option<String>,
        #[arg(long = "strict")]
        strict: bool,
        #[arg(long = "json-out", value_parser = parse_absolute_path)]
        json_out: Option<PathBuf>,
    },
    #[command(name = "target-sources")]
    TargetSources {
        #[command(subcommand)]
        command: TargetSourcesCommands,
    },
    #[command(name = "list")]
    List {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "json-out", value_parser = parse_absolute_path)]
        json_out: Option<PathBuf>,
    },
    #[command(name = "push")]
    Push {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "endpoint", default_value_t = g::remote::auth::default_backend_url())]
        endpoint: String,
        #[arg(long = "session-id")]
        session_id: Option<String>,
        #[arg(long = "yes")]
        yes: bool,
    },
    #[command(name = "pull")]
    Pull {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "remote")]
        remote: Option<String>,
        #[arg(long = "endpoint", default_value_t = g::remote::auth::default_backend_url())]
        endpoint: String,
        #[arg(long = "session-id")]
        session_id: Option<String>,
        #[arg(long = "on-conflict", default_value = "keep-newer")]
        on_conflict: String,
        #[arg(long = "prune")]
        prune: bool,
        #[arg(long = "yes")]
        yes: bool,
    },
    #[command(name = "remote")]
    Remote {
        #[command(subcommand)]
        command: RemoteCommands,
    },
    Link {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "endpoint", default_value_t = g::remote::auth::default_backend_url())]
        endpoint: String,
        #[arg(long = "session-id")]
        session_id: Option<String>,
        #[arg(long = "product-id")]
        product_id: Option<i64>,
    },
    Unlink {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "delete-remote")]
        delete_remote: bool,
        #[arg(long = "endpoint", default_value_t = g::remote::auth::default_backend_url())]
        endpoint: String,
        #[arg(long = "session-id")]
        session_id: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum TargetSourcesCommands {
    Sync {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "root", value_parser = parse_absolute_path)]
        root: PathBuf,
        #[arg(long = "no-prune")]
        no_prune: bool,
        #[arg(long = "yes")]
        yes: bool,
        #[arg(long = "json-out", value_parser = parse_absolute_path)]
        json_out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum RemoteCommands {
    List {
        #[arg(long = "endpoint", default_value_t = g::remote::auth::default_backend_url())]
        endpoint: String,
        #[arg(long = "session-id")]
        session_id: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum TargetCommands {
    #[command(name = "launch-profile-assign")]
    LaunchProfileAssign {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "target")]
        target: String,
        #[arg(long = "profile")]
        profile: String,
    },
    #[command(name = "commit")]
    Commit {
        #[arg(short, long, value_parser = parse_absolute_path)]
        dir: Vec<PathBuf>,
        #[arg(long = "product-dir", value_parser = parse_absolute_path)]
        product_dir: Option<PathBuf>,
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
        #[arg(long = "target")]
        target: String,
        #[arg(long = "yes")]
        yes: bool,
    },
    Extract {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "target")]
        target: String,
        #[arg(short, long, value_parser = parse_absolute_path)]
        output: PathBuf,
        #[command(flatten)]
        runtime: RuntimeArgs,
        #[arg(long = "no-restore-verify", action = ArgAction::SetFalse, default_value_t = true)]
        restore_verify: bool,
        #[arg(long = "stats-json-out", value_parser = parse_absolute_path)]
        stats_json_out: Option<PathBuf>,
    },
    Estimate {
        #[arg(short, long, value_parser = parse_absolute_path)]
        dir: PathBuf,
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: Option<PathBuf>,
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
}

#[derive(Subcommand)]
pub enum TargetInstallCommands {
    Fetch {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "target")]
        target: String,
        #[arg(long = "metadata-url")]
        metadata_url: String,
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
    Apply {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "target")]
        target: String,
        #[arg(short, long, value_parser = parse_absolute_path)]
        dir: PathBuf,
        #[arg(long = "staging-dir", value_parser = parse_absolute_path)]
        staging_dir: Option<PathBuf>,
        #[arg(long = "source-target")]
        source_target: Option<String>,
        #[arg(long = "source-dir", value_parser = parse_absolute_path)]
        source_dir: Option<PathBuf>,
        #[command(flatten)]
        runtime: RuntimeArgs,
        #[arg(long = "stats-json-out", value_parser = parse_absolute_path)]
        stats_json_out: Option<PathBuf>,
    },
    Verify {
        #[arg(long = "repo", value_parser = parse_absolute_path)]
        repo: PathBuf,
        #[arg(long = "target")]
        target: String,
        #[arg(short, long, value_parser = parse_absolute_path)]
        dir: PathBuf,
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
            "refresh",
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
        Cli::try_parse_from(["hostapp-cli gutch", "repo", "refresh", "--repo", repo])
            .expect("absolute repository path");
    }
}
