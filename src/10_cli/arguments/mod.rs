//! Command-line grammar: the root command, shared argument groups and the
//! small command families; larger families live in their own files.

mod installation;
mod repo;
mod snapshot;

pub(crate) use installation::{
    InstallationCommands, PreviewArgs, PreviewPublication, ReconstructArgs,
};
pub(crate) use repo::{
    ConstraintCommands, RemoteCommands, RepoCommands, RepoIndexCommands, RepoTagsCommands,
};
pub(crate) use snapshot::{CaptureArgs, SnapshotCommands, SnapshotTagsCommands};

use std::path::PathBuf;

use crate::artifact::profiles::ArtifactStorageMode;
use crate::artifact::profiles::BundleSizingPreset;
use crate::artifact::profiles::ChunkSizingPreset;
use crate::artifact::profiles::Chunker;
use crate::artifact::profiles::CompressionKind;
use crate::profiling::ProfileMode;
use crate::runtime::RuntimeResourceHints;
use crate::transfer::types::DEFAULT_ADAPTIVE_TRANSFER_MAXIMUM;
use crate::transfer::types::DEFAULT_TRANSFER_CONCURRENCY;
use crate::transfer::types::TransferConcurrencyPolicy;
use clap::Args;
use clap::Parser;
use clap::Subcommand;
use clap::ValueEnum;
use std::env::current_dir;
use std::num::NonZeroU64;

pub(crate) fn parse_cli_path(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if path.as_os_str().is_empty() {
        return Err("path must not be empty".into());
    }
    if path.is_absolute() {
        Ok(path)
    } else {
        current_dir()
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
    pub(crate) fn hints(self) -> RuntimeResourceHints {
        RuntimeResourceHints {
            jobs: self.jobs,
            memory_budget_bytes: self
                .memory_budget_mib
                .and_then(|mib| NonZeroU64::new(mib * 1024 * 1024)),
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
        default_value_t = DEFAULT_TRANSFER_CONCURRENCY
    )]
    pub(crate) concurrency: usize,
    #[arg(
        long = "transfer-max-concurrency",
        default_value_t = DEFAULT_ADAPTIVE_TRANSFER_MAXIMUM
    )]
    pub(crate) maximum: usize,
}

impl TransferArgs {
    pub(crate) fn policy(self) -> TransferConcurrencyPolicy {
        match self.policy {
            TransferPolicyArg::Fixed => TransferConcurrencyPolicy::fixed(self.concurrency),
            TransferPolicyArg::Adaptive => {
                TransferConcurrencyPolicy::adaptive(self.concurrency, self.maximum)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Args)]
pub(crate) struct ArtifactBuildProfileArgs {
    /// Content-defined chunker; JC uses the pinned Plakar-compatible variant.
    #[arg(long, value_enum, default_value_t = crate::artifact::profiles::Chunker::FastCdc)]
    pub(crate) chunker: Chunker,
    /// Chunk sizing preset (algorithm-specific target, not a fixed chunk size).
    #[arg(short = 'C', long = "chunk-preset", value_enum, default_value_t = crate::artifact::profiles::ChunkSizingPreset::Chunk128KiB)]
    pub(crate) chunk_profile: ChunkSizingPreset,
    /// Bundle sizing preset.
    #[arg(short = 'B', long = "bundle-preset", value_enum, default_value_t = crate::artifact::profiles::BundleSizingPreset::Bundle8MiB)]
    pub(crate) bundle_profile: BundleSizingPreset,
    #[arg(long = "compression-algorithm", value_enum, default_value_t = crate::artifact::profiles::CompressionKind::Zstd)]
    pub(crate) compression_algorithm: CompressionKind,
    #[arg(long = "compression-level")]
    pub(crate) compression_level: Option<i32>,
    #[arg(long = "storage-mode", value_enum, default_value_t = crate::artifact::profiles::ArtifactStorageMode::Bundled)]
    pub(crate) storage_mode: ArtifactStorageMode,
}

#[derive(Parser)]
#[command(
    name = "gutch",
    version,
    about = "Capture snapshots and reconstruct verified installation trees"
)]
pub(crate) struct Cli {
    /// Write structured failures, including filename violations and promotion recovery details.
    #[arg(long = "error-json-out", global = true, value_parser = parse_cli_path)]
    pub(crate) error_json_out: Option<PathBuf>,
    #[arg(long, short = 'v', global = true)]
    pub(crate) verbose: bool,

    /// Diagnostic profiling: off, coarse summary, or sampled detailed timelines.
    #[arg(long, global = true, value_enum, default_value_t = crate::profiling::ProfileMode::Off)]
    pub(crate) profile: ProfileMode,
    /// Report directory; required for detailed, optional for summary.
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
pub(crate) enum DebugCommands {
    CorruptInstallation {
        #[arg(short, long, value_parser = parse_cli_path)]
        destination: PathBuf,
        #[arg(long = "plan", value_parser = parse_cli_path)]
        plan: PathBuf,
        #[arg(long = "yes")]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum BundleCommands {
    Inspect {
        #[arg(short, long, value_parser = parse_cli_path)]
        bundle: PathBuf,
    },
}

#[derive(Subcommand)]
pub(crate) enum TreeCommands {
    Compare {
        #[arg(long = "expected", value_parser = parse_cli_path)]
        expected: PathBuf,
        #[arg(long = "actual", value_parser = parse_cli_path)]
        actual: PathBuf,
    },
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
