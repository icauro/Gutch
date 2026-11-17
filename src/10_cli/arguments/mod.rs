//! Command-line grammar: the root command, shared argument groups and the
//! command families, each in its own file.

mod installation;
mod repo;
mod root;
mod shared;
mod snapshot;

pub(crate) use installation::{
    InstallationCommands, PreviewArgs, PreviewPublication, ReconstructArgs,
};
pub(crate) use repo::{
    ConstraintCommands, RemoteCommands, RepoCommands, RepoCredentialCommands, RepoIndexCommands,
};
pub(crate) use root::{Cli, Commands, DebugCommands};
pub(crate) use shared::{ConfirmArgs, DryRunArgs, FetchSourceArgs, RuntimeArgs};
pub(crate) use snapshot::{CaptureArgs, FetchArgs, SnapshotCommands, SnapshotTagsCommands};
