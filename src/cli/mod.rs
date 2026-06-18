pub mod actions;
pub mod args;
pub(crate) mod auth;
pub(crate) mod bundle;
pub(crate) mod commands;
pub(crate) mod common;
pub(crate) mod interactive;
pub(crate) mod path;
pub(crate) mod path_compare;
pub(crate) mod progress;
pub mod repo;
pub(crate) mod reporting;
pub mod target;
pub(crate) mod target_install;

pub use args::{
    AuthCommands, BundleCommands, Cli, Commands, PathCommands, RepoCommands, TargetCommands,
    TargetInstallCommands,
};

use std::sync::atomic::{AtomicBool, Ordering};

static VERBOSE_OUTPUT_ENABLED: AtomicBool = AtomicBool::new(false);
pub(crate) const SECTION_SEPARATOR: &str = "================================";

pub(crate) fn set_verbose_output_enabled(enabled: bool) {
    VERBOSE_OUTPUT_ENABLED.store(enabled, Ordering::Relaxed);
}

pub(crate) fn verbose_output_enabled() -> bool {
    VERBOSE_OUTPUT_ENABLED.load(Ordering::Relaxed)
}

pub fn run_from_env() -> crate::Result<()> {
    run_from_args(std::env::args_os())
}

pub fn run_from_args<I, T>(args: I) -> crate::Result<()>
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
    commands::run(cli)
}
