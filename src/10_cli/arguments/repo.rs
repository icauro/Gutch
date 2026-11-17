//! `repo` command arguments.

use std::path::PathBuf;

use clap::Subcommand;

use super::shared::{ConfirmArgs, DryRunArgs, parse_cli_path};
use crate::manifest::constraints::FilesystemTarget;
use crate::sync::remote::PullConflictPolicy;

#[derive(Subcommand)]
pub(crate) enum RepoCredentialCommands {
    /// Let another credential unlock the repository. The repository is
    /// unlocked with GUTCH_REPO_PASSWORD or GUTCH_REPO_KEY_FILE.
    Add {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        repo: PathBuf,
        /// Add the password in GUTCH_REPO_NEW_PASSWORD.
        #[arg(
            long,
            conflicts_with = "new_key_file",
            required_unless_present = "new_key_file"
        )]
        new_password: bool,
        /// Add a new random key file written here.
        #[arg(long, value_name = "FILE", value_parser = parse_cli_path)]
        new_key_file: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub(crate) enum RepoCommands {
    /// Create a repository.
    Init {
        /// Directory to initialize.
        #[arg(long = "repo", value_name = "DIR", value_parser = parse_cli_path)]
        path: PathBuf,
        /// Filesystems whose filename rules every snapshot must satisfy.
        #[arg(
            long = "filesystem",
            value_name = "TARGET",
            value_enum,
            value_delimiter = ','
        )]
        targets: Vec<FilesystemTarget>,
        /// JSON file containing the complete repository constraints object.
        #[arg(long = "constraints-file", value_name = "FILE", value_parser = parse_cli_path, conflicts_with = "targets")]
        constraints: Option<PathBuf>,
        /// Initialize even if the directory is not empty.
        #[arg(long)]
        force: bool,
        /// Encrypt the repository with the password in GUTCH_REPO_PASSWORD or
        /// the key file named by GUTCH_REPO_KEY_FILE.
        #[arg(long, conflicts_with = "new_key_file")]
        encrypt: bool,
        /// Encrypt the repository with a new random key file written here.
        #[arg(long, value_name = "FILE", value_parser = parse_cli_path)]
        new_key_file: Option<PathBuf>,
        #[command(flatten)]
        dry_run: DryRunArgs,
    },
    /// Manage the credentials that unlock an encrypted repository.
    Credential {
        #[command(subcommand)]
        command: RepoCredentialCommands,
    },
    /// Show local repository state.
    Status {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        repo: PathBuf,
    },
    /// Change the local display name, leaving the directory and remote name unchanged.
    Rename {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        repo: PathBuf,
        /// New display name.
        #[arg(long)]
        name: String,
        #[command(flatten)]
        dry_run: DryRunArgs,
    },
    /// Verify stored repository metadata and artifacts.
    Verify {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        repo: PathBuf,
        /// Verify only this snapshot.
        #[arg(long = "snapshot", value_name = "ID")]
        snapshot_id: Option<String>,
        /// Also verify every chunk hash.
        #[arg(long)]
        strict: bool,
    },
    /// Show or change filename compatibility, tag rules and matching tag keys.
    Constraints {
        #[command(subcommand)]
        command: ConstraintCommands,
    },
    /// Rebuild the repository chunk index.
    Index {
        #[command(subcommand)]
        command: RepoIndexCommands,
    },
    /// Create, bind and inspect the remote repository.
    Remote {
        #[command(subcommand)]
        command: RemoteCommands,
    },
    /// Synchronize local repository state to its bound remote.
    Push {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[command(flatten)]
        confirm: ConfirmArgs,
    },
    /// Synchronize remote repository data locally; does not update installations.
    Pull {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        repo: PathBuf,
        /// Remote repository ID; defaults to the bound remote.
        #[arg(long, value_name = "ID")]
        remote_repo_id: Option<String>,
        /// How to resolve snapshots changed on both sides.
        #[arg(long, value_enum, default_value = "keep-newer")]
        on_conflict: PullConflictPolicy,
        /// Delete local snapshots that the remote no longer has.
        #[arg(long)]
        prune: bool,
        /// Download only this snapshot; repeat for more. Pull without it
        /// before pushing, since a push removes remote snapshots this copy
        /// lacks.
        #[arg(long = "snapshot", value_name = "ID")]
        snapshot_ids: Vec<String>,
        /// Download snapshot metadata but no bundles; a later pull without
        /// it downloads them.
        #[arg(long)]
        metadata_only: bool,
        #[command(flatten)]
        confirm: ConfirmArgs,
    },
}

#[derive(Subcommand)]
pub(crate) enum RepoIndexCommands {
    /// Verify artifacts and rebuild the chunk index, replacing it only after a
    /// complete, successful rebuild.
    Rebuild {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[command(flatten)]
        dry_run: DryRunArgs,
    },
}

#[derive(Subcommand)]
pub(crate) enum ConstraintCommands {
    /// Show the repository constraints.
    Show {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        repo: PathBuf,
    },
    /// Replace the repository constraints.
    Set {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        repo: PathBuf,
        /// Filesystems whose filename rules every snapshot must satisfy.
        #[arg(
            long = "filesystem",
            value_name = "TARGET",
            value_enum,
            value_delimiter = ',',
            required_unless_present = "constraints"
        )]
        targets: Vec<FilesystemTarget>,
        /// JSON file containing the complete replacement constraints object.
        #[arg(long = "constraints-file", value_name = "FILE", value_parser = parse_cli_path, conflicts_with = "targets")]
        constraints: Option<PathBuf>,
        #[command(flatten)]
        dry_run: DryRunArgs,
    },
    /// Remove all repository constraints.
    Clear {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        repo: PathBuf,
        #[command(flatten)]
        dry_run: DryRunArgs,
    },
}

#[derive(Subcommand)]
pub(crate) enum RemoteCommands {
    /// List the remote repositories the provider can access.
    List,
    /// Create a remote repository and bind the local repository to it.
    Create {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        repo: PathBuf,
        /// Provider-defined scope that will own the remote repository.
        #[arg(long, value_name = "ID")]
        scope_id: String,
    },
    /// Bind to an existing remote repository without synchronizing content.
    Bind {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        repo: PathBuf,
        /// Remote repository to bind.
        #[arg(long, value_name = "ID")]
        remote_repo_id: String,
    },
    /// Remove the binding; retain remote content unless --delete-remote is supplied.
    Unbind {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
        repo: PathBuf,
        /// Also delete the remote repository.
        #[arg(long)]
        delete_remote: bool,
    },
    /// Inspect advisory tag references supplied by the remote provider.
    TagReferences {
        /// Repository directory.
        #[arg(long, value_name = "DIR", value_parser = parse_cli_path)]
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
