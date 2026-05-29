#![warn(clippy::all)]
#![warn(rust_2024_compatibility)]

mod auth;
mod cli;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use gutch as g;

#[derive(Parser)]
#[command(name = "gutch", version, about = "Launcher artifact engine")]
struct Cli {
    #[arg(long, short = 'v', global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Auth {
        #[command(subcommand)]
        command: AuthCommands,
    },

    Repo {
        #[command(subcommand)]
        command: RepoCommands,
    },

    Interactive,

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
enum AuthCommands {
    Login {
        #[arg(long = "endpoint", default_value = auth::DEFAULT_BACKEND_URL)]
        endpoint: String,

        #[arg(long = "no-open")]
        no_open: bool,
    },

    Logout {
        #[arg(long = "endpoint", default_value = auth::DEFAULT_BACKEND_URL)]
        endpoint: String,

        #[arg(long = "session-id")]
        session_id: Option<String>,
    },

    Status {
        #[arg(long = "endpoint", default_value = auth::DEFAULT_BACKEND_URL)]
        endpoint: String,

        #[arg(long = "session-id")]
        session_id: Option<String>,
    },
}

#[derive(Subcommand)]
enum RepoCommands {
    Create {
        #[arg(long = "content-repo")]
        content_repo: String,

        #[arg(long = "schema")]
        schema: Option<PathBuf>,

        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,
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

    Inspect {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,

        #[arg(long = "json-out")]
        json_out: Option<PathBuf>,
    },

    #[command(name = "discover-source-targets")]
    DiscoverSourceTargets {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,

        #[arg(long = "source-targets-root")]
        source_targets_root: PathBuf,

        #[arg(long = "json-out")]
        json_out: Option<PathBuf>,
    },

    #[command(name = "list")]
    List {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,

        #[arg(long = "json-out")]
        json_out: Option<PathBuf>,
    },

    #[command(name = "delete-target")]
    DeleteTarget {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,

        #[arg(long = "target")]
        target: String,

        #[arg(long = "yes")]
        yes: bool,
    },

    #[command(name = "sync-up")]
    SyncUp {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,

        #[arg(long = "endpoint", default_value = auth::DEFAULT_BACKEND_URL)]
        endpoint: String,

        #[arg(long = "product-id")]
        product_id: i64,

        #[arg(long = "content-repo-id")]
        content_repo_id: i64,

        #[arg(long = "session-id")]
        session_id: Option<String>,
    },
}

#[derive(Subcommand)]
enum TargetCommands {
    Insert {
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,

        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,

        #[arg(long)]
        version: Option<String>,

        #[arg(short = 'P', long, visible_alias = "os")]
        platform: Option<String>,

        #[arg(short = 'A', long, visible_alias = "arch")]
        architecture: Option<String>,

        #[arg(short, long)]
        tier: Option<String>,

        #[arg(long = "tag")]
        tags: Vec<String>,

        #[arg(short = 'C', long = "chunk-profile", value_enum, default_value_t = g::manifest::ArtifactSizingPreset::Balanced)]
        chunk_profile: g::manifest::ArtifactSizingPreset,

        #[arg(short = 'B', long = "bundle-profile", value_enum, default_value_t = g::manifest::BundleSizingPreset::Balanced)]
        bundle_profile: g::manifest::BundleSizingPreset,

        #[arg(long = "memory-mode", value_enum, default_value_t = g::MemoryMode::Auto)]
        memory_mode: g::MemoryMode,

        #[arg(long = "max-memory-mib")]
        max_memory_mib: Option<u64>,

        #[arg(long = "jobs")]
        jobs: Option<usize>,

        #[arg(long = "summary-out")]
        summary_out: Option<PathBuf>,

        #[arg(long = "stats-json-out")]
        stats_json_out: Option<PathBuf>,
    },

    Export {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,

        #[arg(long = "target")]
        target: String,

        #[arg(short, long)]
        output: PathBuf,

        #[arg(long = "memory-mode", value_enum, default_value_t = g::MemoryMode::Auto)]
        memory_mode: g::MemoryMode,

        #[arg(long = "max-memory-mib")]
        max_memory_mib: Option<u64>,

        #[arg(long = "jobs")]
        jobs: Option<usize>,

        #[arg(long = "summary-out")]
        summary_out: Option<PathBuf>,

        #[arg(long = "stats-json-out")]
        stats_json_out: Option<PathBuf>,
    },

    Estimate {
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,

        #[arg(long)]
        version: String,
    },
}

#[derive(Subcommand)]
enum TargetInstallCommands {
    Fetch {
        #[arg(long = "repo", default_value = ".")]
        repo: PathBuf,

        #[arg(long = "target")]
        target: String,

        #[arg(long = "manifest-url")]
        manifest_url: String,

        #[arg(long = "manifest-sig-url")]
        manifest_sig_url: Option<String>,

        #[arg(long = "bundle-urls-endpoint")]
        bundle_urls_endpoint: Option<String>,

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

        #[arg(long = "memory-mode", value_enum, default_value_t = g::MemoryMode::Auto)]
        memory_mode: g::MemoryMode,

        #[arg(long = "max-memory-mib")]
        max_memory_mib: Option<u64>,

        #[arg(long = "jobs")]
        jobs: Option<usize>,

        #[arg(long = "summary-out")]
        summary_out: Option<PathBuf>,

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

        #[arg(long = "memory-mode", value_enum, default_value_t = g::MemoryMode::Auto)]
        memory_mode: g::MemoryMode,

        #[arg(long = "max-memory-mib")]
        max_memory_mib: Option<u64>,

        #[arg(long = "jobs")]
        jobs: Option<usize>,

        #[arg(long = "summary-out")]
        summary_out: Option<PathBuf>,

        #[arg(long = "stats-json-out")]
        stats_json_out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum BundleCommands {
    Inspect {
        #[arg(short, long)]
        bundle: PathBuf,
    },
}

#[derive(Subcommand)]
enum PathCommands {
    Compare {
        #[arg(long = "expected")]
        expected: PathBuf,

        #[arg(long = "actual")]
        actual: PathBuf,

        #[arg(long = "summary-out")]
        summary_out: Option<PathBuf>,

        #[arg(long = "stats-json-out")]
        stats_json_out: Option<PathBuf>,
    },
}

fn main() {
    let cli = Cli::parse();
    crate::cli::set_verbose_output_enabled(cli.verbose);

    if let Err(e) = cli::commands::run(cli) {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
