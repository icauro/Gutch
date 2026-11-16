//! Command-line entry points: parse, set up profiling and error export, dispatch.

use super::arguments::Cli;
use super::arguments::Commands;
use super::arguments::RemoteCommands;
use super::arguments::RepoCommands;
use super::session::set_verbose_output_enabled;
use super::{dispatch, output, reporting};
use crate::filesystem::write_json;
use crate::profiling::ProfileMode;
use crate::profiling::Profiler;
use crate::sync::remote::RemoteRepositoryCommandProvider;
use std::ffi::OsString;
use std::sync::Arc;

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
            .map_err(|error| crate::Error::Usage(error.render().to_string()))?,
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            error.print()?;
            return Ok(());
        }
        Err(error) => return Err(crate::Error::Usage(error.render().to_string())),
    };
    set_verbose_output_enabled(cli.verbose);
    let profile_out = cli.profile_out.clone();
    let error_json_out = cli.error_json_out.clone();
    if (cli.profile == ProfileMode::Off && profile_out.is_some())
        || (cli.profile == ProfileMode::Detailed && profile_out.is_none())
    {
        return Err(crate::Error::invalid_input(
            "--profile detailed requires --profile-dir; off forbids profile output",
        ));
    }
    let mode = cli.profile;
    let profiler = (mode != ProfileMode::Off).then(|| Arc::new(Profiler::new(mode)));
    // Begin inside the exported result so its failure still reaches --error-json-out.
    let command_result = output::begin(&cli).and_then(|()| {
        let _command_span = profiler
            .as_ref()
            .map(|profiler| profiler.span("cli.command"));
        dispatch::run(cli, remote_provider, &profiler)
    });
    let output_result = output::finish(&command_result);
    let command_result = command_result.and(output_result);
    if let (Err(error), Some(path)) = (&command_result, error_json_out) {
        let details = output::versioned(error.diagnostic_report());
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
        (Ok(()), Err(error)) => Err(crate::Error::invalid_input(format!(
            "Operation succeeded but profiling export failed: {error}"
        ))),
        (Ok(()), Ok(())) => Ok(()),
    }
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
