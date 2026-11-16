//! Path parsing and the argument groups shared by several commands.

use crate::artifact::profiles::ArtifactStorageMode;
use crate::artifact::profiles::BundleSizingPreset;
use crate::artifact::profiles::ChunkSizingPreset;
use crate::artifact::profiles::Chunker;
use crate::artifact::profiles::CompressionKind;
use crate::runtime::RuntimeResourceHints;
use crate::transfer::types::DEFAULT_ADAPTIVE_TRANSFER_MAXIMUM;
use crate::transfer::types::DEFAULT_TRANSFER_CONCURRENCY;
use crate::transfer::types::TransferConcurrencyPolicy;
use clap::Args;
use clap::ValueEnum;
use std::env::current_dir;
use std::num::NonZeroU64;
use std::path::PathBuf;

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

/// Optional download sources shared by `snapshot fetch` and
/// `installation reconstruct --fetch`. Its argument group stays populated only
/// while it flattens no other argument struct.
#[derive(Args)]
pub(crate) struct FetchSourceArgs {
    #[arg(long = "product-metadata-url")]
    pub(crate) product_metadata_url: Option<String>,
    #[arg(long = "repo-metadata-url")]
    pub(crate) repo_metadata_url: Option<String>,
    #[arg(long = "chunk-index-url")]
    pub(crate) chunk_index_url: Option<String>,
    #[arg(long = "bundle-downloads-endpoint")]
    pub(crate) bundle_downloads_endpoint: Option<String>,
    #[arg(long = "session-id")]
    pub(crate) session_id: Option<String>,
    #[arg(long = "artifact-url")]
    pub(crate) artifacts: Vec<String>,
    #[arg(long = "artifact-url-manifest", value_parser = parse_cli_path)]
    pub(crate) artifact_url_manifest: Option<PathBuf>,
}

/// `--dry-run` for commands that change state without asking first.
#[derive(Debug, Clone, Copy, Args)]
pub(crate) struct DryRunArgs {
    /// Show what would change without applying it.
    #[arg(long)]
    pub(crate) dry_run: bool,
}

/// `--dry-run` and `--yes` for commands that show a plan and ask before
/// applying it.
#[derive(Debug, Clone, Copy, Args)]
pub(crate) struct ConfirmArgs {
    /// Show the plan without applying it.
    #[arg(long, conflicts_with = "yes")]
    pub(crate) dry_run: bool,
    /// Apply the plan without asking for confirmation.
    #[arg(long)]
    pub(crate) yes: bool,
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
