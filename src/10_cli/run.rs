//! Command-line entry points: parse, set up profiling and error export, dispatch.

use super::arguments::Cli;
use super::session::set_verbose_output_enabled;
use super::{dispatch, output, reporting};
use crate::filesystem::write_json;
use crate::profiling::ProfileMode;
use crate::profiling::Profiler;
use crate::sync::remote::RemoteRepoService;
use std::ffi::OsString;
use std::sync::Arc;

/// Creates the remote provider when a command first needs one, so hosts
/// authenticate only for remote commands.
pub type RemoteProviderFactory<'a> =
    dyn Fn() -> crate::Result<Box<dyn RemoteRepoService + 'a>> + 'a;

/// How an embedding host presents and backs the Gutch command line.
///
/// The command grammar and execution path remain owned by Gutch; hosts choose
/// the displayed executable name and may supply remote repository access.
#[derive(Clone, Copy)]
pub struct CommandLineHost<'a> {
    pub bin_name: &'a str,
    pub remote_provider: Option<&'a RemoteProviderFactory<'a>>,
}

impl Default for CommandLineHost<'_> {
    fn default() -> Self {
        Self {
            bin_name: "gutch",
            remote_provider: None,
        }
    }
}

impl<'a> CommandLineHost<'a> {
    /// Creates the provider for a command that cannot run without one.
    pub(crate) fn require_remote_provider(&self) -> crate::Result<Box<dyn RemoteRepoService + 'a>> {
        self.optional_remote_provider()?.ok_or_else(|| {
            crate::Error::invalid_input(
                "remote commands are unavailable: this program has no remote provider",
            )
        })
    }

    /// Creates the provider when the host supplies one.
    pub(crate) fn optional_remote_provider(
        &self,
    ) -> crate::Result<Option<Box<dyn RemoteRepoService + 'a>>> {
        self.remote_provider.map(|create| create()).transpose()
    }
}

/// Parses and runs one command line, returning failures to the host.
pub fn run_command_line<I, T>(args: I, host: &CommandLineHost<'_>) -> crate::Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    use clap::CommandFactory;
    use clap::FromArgMatches;

    let command = Cli::command().bin_name(host.bin_name);
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
    let error_output = cli.error_output.clone();
    if (cli.profile == ProfileMode::Off && profile_out.is_some())
        || (cli.profile == ProfileMode::Detailed && profile_out.is_none())
    {
        return Err(crate::Error::invalid_input(
            "--profile detailed requires --profile-dir; off forbids profile output",
        ));
    }
    let mode = cli.profile;
    let profiler = (mode != ProfileMode::Off).then(|| Arc::new(Profiler::new(mode)));
    // Begin inside the exported result so its failure still reaches --error-output.
    let command_result = output::begin(&cli).and_then(|()| {
        let _command_span = profiler
            .as_ref()
            .map(|profiler| profiler.span("cli.command"));
        dispatch::run(cli, host, &profiler)
    });
    let output_result = output::finish(&command_result);
    let command_result = command_result.and(output_result);
    if let (Err(error), Some(path)) = (&command_result, error_output) {
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
