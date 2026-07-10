use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand};
use gutch as g;

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
    name = "plctl gutch",
    version,
    about = "Launcher artifact engine"
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
        #[arg(long = "path", default_value = ".")]
        path: PathBuf,
        #[arg(long = "schema")]
        schema: Option<PathBuf>,
        #[arg(long = "force")]
        force: bool,
    },
    #[command(name = "schema-set")]
    SchemaSet {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,
        #[arg(long = "schema")]
        schema: Option<PathBuf>,
        #[arg(long = "clear")]
        clear: bool,
    },
    #[command(name = "launch-set")]
    LaunchSet {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,
        #[arg(long = "launch")]
        launch: Option<PathBuf>,
        #[arg(long = "clear")]
        clear: bool,
    },
    Check {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,
        #[arg(long = "json-out")]
        json_out: Option<PathBuf>,
    },
    Verify {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,
        #[arg(long = "target")]
        target: Option<String>,
        #[arg(long = "strict")]
        strict: bool,
        #[arg(long = "json-out")]
        json_out: Option<PathBuf>,
    },
    #[command(name = "target-sources")]
    TargetSources {
        #[command(subcommand)]
        command: TargetSourcesCommands,
    },
    #[command(name = "list")]
    List {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,
        #[arg(long = "json-out")]
        json_out: Option<PathBuf>,
    },
    #[command(name = "sync-up")]
    SyncUp {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,
        #[arg(long = "endpoint", default_value_t = g::remote::auth::default_backend_url())]
        endpoint: String,
        #[arg(long = "session-id")]
        session_id: Option<String>,
        #[arg(long = "yes")]
        yes: bool,
    },
    Link {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,
        #[arg(long = "endpoint", default_value_t = g::remote::auth::default_backend_url())]
        endpoint: String,
        #[arg(long = "session-id")]
        session_id: Option<String>,
        #[arg(long = "product-id")]
        product_id: Option<i64>,
    },
    Unlink {
        #[arg(long = "repo", default_value = ".")]
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
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,
        #[arg(long = "root")]
        root: PathBuf,
        #[arg(long = "no-prune")]
        no_prune: bool,
        #[arg(long = "yes")]
        yes: bool,
        #[arg(long = "json-out")]
        json_out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum TargetCommands {
    #[command(name = "launch-set")]
    LaunchSet {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,
        #[arg(long = "target")]
        target: String,
        #[arg(long = "launch")]
        launch: Option<PathBuf>,
        #[arg(long = "clear")]
        clear: bool,
    },
    Insert {
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[command(flatten)]
        build_profile: ArtifactBuildProfileArgs,
        #[command(flatten)]
        runtime: RuntimeArgs,
        #[arg(long = "stats-json-out")]
        stats_json_out: Option<PathBuf>,
        #[arg(long = "yes")]
        yes: bool,
    },
    #[command(name = "insert-many")]
    InsertMany {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,
        #[arg(long = "product-dir")]
        product_dir: Option<PathBuf>,
        #[arg(long = "dir")]
        dirs: Vec<PathBuf>,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[command(flatten)]
        build_profile: ArtifactBuildProfileArgs,
        #[command(flatten)]
        runtime: RuntimeArgs,
        #[arg(long = "stats-json-out")]
        stats_json_out: Option<PathBuf>,
        #[arg(long = "yes")]
        yes: bool,
    },
    Delete {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,
        #[arg(long = "target")]
        target: String,
        #[arg(long = "yes")]
        yes: bool,
    },
    Extract {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,
        #[arg(long = "target")]
        target: String,
        #[arg(short, long)]
        output: PathBuf,
        #[command(flatten)]
        runtime: RuntimeArgs,
        #[arg(long = "no-restore-verify", action = ArgAction::SetFalse, default_value_t = true)]
        restore_verify: bool,
        #[arg(long = "stats-json-out")]
        stats_json_out: Option<PathBuf>,
    },
    Estimate {
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,
        #[arg(long = "repo")]
        repo: Option<PathBuf>,
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
}

#[derive(Subcommand)]
pub enum TargetInstallCommands {
    Fetch {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,
        #[arg(long = "target")]
        target: String,
        #[arg(long = "manifest-url")]
        manifest_url: String,
        #[arg(long = "repo-manifest-url")]
        repo_manifest_url: Option<String>,
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
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,
        #[arg(long = "target")]
        target: String,
        #[arg(short, long)]
        dir: PathBuf,
        #[arg(long = "staging-dir")]
        staging_dir: Option<PathBuf>,
        #[arg(long = "source-target")]
        source_target: Option<String>,
        #[arg(long = "source-dir")]
        source_dir: Option<PathBuf>,
        #[command(flatten)]
        runtime: RuntimeArgs,
        #[arg(long = "stats-json-out")]
        stats_json_out: Option<PathBuf>,
    },
    Verify {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,
        #[arg(long = "target")]
        target: String,
        #[arg(short, long)]
        dir: PathBuf,
        #[arg(long = "strict")]
        strict: bool,
        #[command(flatten)]
        runtime: RuntimeArgs,
        #[arg(long = "stats-json-out")]
        stats_json_out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum BundleCommands {
    Inspect {
        #[arg(short, long)]
        bundle: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum PathCommands {
    Compare {
        #[arg(long = "expected")]
        expected: PathBuf,
        #[arg(long = "actual")]
        actual: PathBuf,
        #[arg(long = "stats-json-out")]
        stats_json_out: Option<PathBuf>,
    },
}
