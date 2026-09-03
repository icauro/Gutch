pub(crate) mod actions;
pub(crate) mod args;
pub(crate) mod bundle;
pub(crate) mod commands;
pub(crate) mod common;
pub(crate) mod path;
pub(crate) mod path_compare;
pub(crate) mod progress;
pub(crate) mod remote;
pub(crate) mod repo;
pub(crate) mod reporting;
pub(crate) mod snapshot;
pub(crate) mod snapshot_ops;
pub(crate) mod snapshot_sources;

use std::sync::Arc;

pub(crate) use args::{
    BundleCommands, Cli, Commands, PathCommands, ReconstructedCommands, RemoteCommands,
    RepoCommands, RepoNameCommands, RepoTagSchemaCommands, RepoTagsCommands, SnapshotCommands,
    SnapshotSourcesCommands,
};
pub(crate) use common::prompt_yes_no;

pub(crate) type Profile = Option<Arc<crate::GutchProfiler>>;

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
    let trace_out = cli.trace_out.clone();
    let profiler = trace_out
        .as_ref()
        .map(|_| Arc::new(crate::GutchProfiler::new()));
    let command_result = {
        let _command_span = profiler
            .as_ref()
            .map(|profiler| profiler.span("cli.command"));
        commands::run(cli, remote_provider, &profiler)
    };
    let trace_result = match (trace_out.as_ref(), profiler.as_ref()) {
        (Some(path), Some(profiler)) => profiler.write_to(path),
        _ => Ok(()),
    };
    match (command_result, trace_result) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

pub(crate) fn control_with_profile(profile: &Profile) -> crate::GutchOperationControl {
    profile
        .as_ref()
        .map_or_else(crate::GutchOperationControl::default, |profiler| {
            crate::GutchOperationControl::default().with_profiler(Arc::clone(profiler))
        })
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
