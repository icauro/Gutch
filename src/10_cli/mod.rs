//! Generic CLI parsing, command adapters, progress presentation, and reporting.

pub(crate) mod arguments;
pub(crate) mod bundle;
pub(crate) mod command_support;
pub(crate) mod config;
pub(crate) mod dispatch;
pub(crate) mod installation;
pub(crate) mod output;
pub(crate) mod progress;
pub(crate) mod repo;
pub(crate) mod repo_remote;
pub(crate) mod reporting;
pub(crate) mod snapshot;
pub(crate) mod snapshot_transfer;
pub(crate) mod tags;
pub(crate) mod tree;

use std::sync::Arc;

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

pub(crate) type Profile = Option<Arc<Profiler>>;

use crate::filesystem::write_json;
use crate::profiling::ProfileMode;
use crate::profiling::Profiler;
use crate::progress::OperationControl;
use crate::sync::remote::RemoteRepositoryCommandProvider;
use std::ffi::OsString;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

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
    T: Into<OsString> + Clone,
{
    use clap::Parser;

    let Ok(cli) = Cli::try_parse_from(args) else {
        return false;
    };
    match cli.command {
        Commands::Repo { command } => match command {
            RepoCommands::Push { .. } | RepoCommands::Pull { .. } => true,
            RepoCommands::Remote { command } => match command {
                RemoteCommands::Bind { .. } => false,
                RemoteCommands::Unbind { delete_remote, .. } => delete_remote,
                _ => true,
            },
            RepoCommands::Tags { .. } => true,
            _ => false,
        },
        _ => false,
    }
}

pub fn run_from_args<I, T>(args: I) -> crate::Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    run_from_args_with_remote_provider(args, None)
}

pub fn run_from_args_with_remote_provider<I, T>(
    args: I,
    remote_provider: Option<&dyn RemoteRepositoryCommandProvider>,
) -> crate::Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    run_from_args_with_remote_provider_named(args, remote_provider, "gutch")
}

/// Runs the shared command parser with a caller-selected display name.
///
/// The command grammar and execution path remain owned by Gutch; callers can
/// present the same commands under their own executable name.
pub fn run_from_args_with_remote_provider_named<I, T>(
    args: I,
    remote_provider: Option<&dyn RemoteRepositoryCommandProvider>,
    bin_name: &str,
) -> crate::Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    use clap::CommandFactory;
    use clap::FromArgMatches;

    let command = Cli::command().bin_name(bin_name);
    let cli = match command.try_get_matches_from(args) {
        Ok(matches) => Cli::from_arg_matches(&matches)
            .map_err(|error| crate::Error::manifest(error.to_string()))?,
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
    let profile_out = cli.profile_out.clone();
    let error_json_out = cli.error_json_out.clone();
    if (cli.profile == ProfileMode::Off && profile_out.is_some())
        || (cli.profile == ProfileMode::Detailed && profile_out.is_none())
    {
        return Err(crate::Error::manifest(
            "--profile detailed requires --profile-dir; off forbids profile output",
        ));
    }
    let mode = cli.profile;
    let profiler = (mode != ProfileMode::Off).then(|| Arc::new(Profiler::new(mode)));
    output::begin(&cli)?;
    let command_result = {
        let _command_span = profiler
            .as_ref()
            .map(|profiler| profiler.span("cli.command"));
        dispatch::run(cli, remote_provider, &profiler)
    };
    let output_result = output::finish(&command_result);
    let command_result = command_result.and(output_result);
    if let (Err(error), Some(path)) = (&command_result, error_json_out) {
        let details = error.diagnostic_report();
        if let Err(export_error) =
            reporting::ensure_parent_directory(&path).and_then(|_| write_json(&path, &details))
        {
            eprintln!("Error details could not be saved: {export_error}");
        }
    }
    let profile_result = match (profile_out.as_ref(), profiler.as_ref()) {
        (Some(path), Some(profiler)) => profiler.finish(path, command_result.is_ok()),
        _ => Ok(()),
    };
    match (command_result, profile_result) {
        (Err(error), profile_result) => {
            if let Err(profile_error) = profile_result {
                eprintln!("Profiling export also failed: {profile_error}");
            }
            Err(error)
        }
        (Ok(()), Err(error)) => Err(crate::Error::manifest(format!(
            "Operation succeeded but profiling export failed: {error}"
        ))),
        (Ok(()), Ok(())) => Ok(()),
    }
}

pub(crate) fn control_with_profile(profile: &Profile) -> OperationControl {
    profile
        .as_ref()
        .map_or_else(OperationControl::default, |profiler| {
            OperationControl::default().with_profiler(Arc::clone(profiler))
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
            "references",
            "--repo",
            repo
        ]));
        assert!(requires_remote_provider([
            "gutch",
            "repo",
            "remote",
            "unbind",
            "--repo",
            repo,
            "--delete-remote"
        ]));
        assert!(!requires_remote_provider(["gutch", "repo", "init"]));
        assert!(!requires_remote_provider([
            "gutch",
            "repo",
            "remote",
            "bind",
            "--repo",
            repo,
            "--remote-repo-id",
            "id"
        ]));
        assert!(!requires_remote_provider([
            "gutch", "repo", "remote", "unbind", "--repo", repo
        ]));
        assert!(!requires_remote_provider(["gutch", "snapshot", "list"]));
        assert!(!requires_remote_provider([
            "gutch",
            "snapshot",
            "fetch",
            "--repo",
            repo,
            "--snapshot",
            "snapshot",
            "--snapshot-metadata-url",
            "https://example.test/metadata",
            "--blueprint-url",
            "https://example.test/blueprint",
            "--session-id",
            "artifact-session"
        ]));
    }
}
