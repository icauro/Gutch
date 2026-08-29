pub mod actions;
pub mod args;
pub(crate) mod bundle;
pub(crate) mod commands;
pub(crate) mod common;
pub(crate) mod path;
pub(crate) mod path_compare;
pub(crate) mod progress;
pub(crate) mod remote;
pub mod repo;
pub(crate) mod reporting;
pub mod snapshot;
pub(crate) mod snapshot_ops;
pub mod snapshot_sources;

pub use args::{
    BundleCommands, Cli, Commands, PathCommands, RemoteCommands, RepoCommands, RepoNameCommands,
    RepoTagSchemaCommands, RepoTagsCommands, SnapshotCommands, SnapshotSourcesCommands,
    parse_absolute_path,
};
pub use common::prompt_yes_no;

use std::sync::atomic::{AtomicBool, Ordering};

static VERBOSE_OUTPUT_ENABLED: AtomicBool = AtomicBool::new(false);
pub(crate) const SECTION_SEPARATOR: &str = "================================";

pub(crate) fn set_verbose_output_enabled(enabled: bool) {
    VERBOSE_OUTPUT_ENABLED.store(enabled, Ordering::Relaxed);
}

pub(crate) fn verbose_output_enabled() -> bool {
    VERBOSE_OUTPUT_ENABLED.load(Ordering::Relaxed)
}

pub fn requires_remote_provider<I, T>(args: I) -> bool
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    use clap::Parser;

    let Ok(cli) = Cli::try_parse_from(args) else {
        return false;
    };
    match cli.command {
        Commands::Repo { command } => match command {
            RepoCommands::Push { .. }
            | RepoCommands::Pull { .. }
            | RepoCommands::Remote { .. }
            | RepoCommands::Link { .. } => true,
            RepoCommands::Unlink { delete_remote, .. } => delete_remote,
            RepoCommands::Tags {
                command: RepoTagsCommands::Requirements { .. },
            } => true,
            _ => false,
        },
        _ => false,
    }
}

pub fn run_from_args<I, T>(args: I) -> crate::Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    run_from_args_with_remote_provider(args, None)
}

pub fn run_from_args_with_remote_provider<I, T>(
    args: I,
    remote_provider: Option<&dyn crate::RemoteRepositoryCommandProvider>,
) -> crate::Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    use clap::Parser;

    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            error
                .print()
                .map_err(|err| crate::Error::manifest(err.to_string()))?;
            return Ok(());
        }
        Err(error) => return Err(crate::Error::manifest(error.to_string())),
    };
    set_verbose_output_enabled(cli.verbose);
    commands::run(cli, remote_provider)
}

#[cfg(test)]
mod tests {
    use super::requires_remote_provider;

    #[test]
    fn identifies_only_commands_that_need_a_remote_provider() {
        let repo = if cfg!(windows) {
            r"D:\Repos\creator"
        } else {
            "/repos/creator"
        };
        assert!(requires_remote_provider([
            "gutch", "repo", "push", "--repo", repo
        ]));
        assert!(requires_remote_provider([
            "gutch",
            "repo",
            "tags",
            "requirements",
            "--repo",
            repo
        ]));
        assert!(requires_remote_provider([
            "gutch",
            "repo",
            "unlink",
            "--repo",
            repo,
            "--delete-remote"
        ]));
        assert!(!requires_remote_provider(["gutch", "repo", "init"]));
        assert!(!requires_remote_provider(["gutch", "repo", "bind"]));
        assert!(!requires_remote_provider(["gutch", "repo", "unlink"]));
        assert!(!requires_remote_provider(["gutch", "snapshot", "list"]));
        assert!(!requires_remote_provider([
            "gutch",
            "snapshot",
            "fetch",
            "--repo",
            repo,
            "--snapshot-id",
            "snapshot",
            "--metadata-url",
            "https://example.test/metadata",
            "--blueprint-url",
            "https://example.test/blueprint",
            "--session-id",
            "artifact-session"
        ]));
    }
}
