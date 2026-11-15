//! `repo` command arguments.

use std::path::PathBuf;

use clap::Subcommand;

use super::parse_cli_path;
use crate::manifest::constraints::FilesystemTarget;
use crate::sync::remote::PullConflictPolicy;

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
        targets: Vec<FilesystemTarget>,
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
        #[arg(long = "on-conflict", value_enum, default_value = "keep-newer")]
        on_conflict: PullConflictPolicy,
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
        targets: Vec<FilesystemTarget>,
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
    },
}

/// The CLI spells filesystem targets the way repository files store them.
impl clap::ValueEnum for FilesystemTarget {
    fn value_variants<'a>() -> &'a [Self] {
        &Self::ALL
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        Some(clap::builder::PossibleValue::new(self.as_str()))
    }
}
