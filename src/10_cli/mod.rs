//! Generic CLI parsing, command adapters, progress presentation, and reporting.

pub(crate) mod arguments;
pub(crate) mod bundle;
pub(crate) mod command_support;
pub(crate) mod dispatch;
pub(crate) mod installation;
pub(crate) mod output;
pub(crate) mod progress;
pub(crate) mod repo;
pub(crate) mod repo_remote;
pub(crate) mod reporting;
mod run;
mod session;
pub(crate) mod snapshot;
pub(crate) mod snapshot_transfer;
pub(crate) mod tree;

pub(crate) use arguments::BundleCommands;
pub(crate) use arguments::Cli;
pub(crate) use arguments::Commands;
pub(crate) use arguments::ConstraintCommands;
pub(crate) use arguments::DebugCommands;
pub(crate) use arguments::InstallationCommands;
pub(crate) use arguments::RemoteCommands;
pub(crate) use arguments::RepoCommands;
pub(crate) use arguments::RepoTagsCommands;
pub(crate) use arguments::SnapshotCommands;
pub(crate) use arguments::TreeCommands;
pub(crate) use command_support::prompt_yes_no;
pub use run::{
    requires_remote_provider, run_from_args, run_from_args_with_remote_provider,
    run_from_args_with_remote_provider_named,
};
pub(crate) use session::{
    Profile, SECTION_SEPARATOR, control_with_profile, verbose_output_enabled,
};
