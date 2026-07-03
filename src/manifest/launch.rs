use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::target::TargetManifest;
use crate::{Error, Result};

const CANONICAL_OPERATING_SYSTEMS: &[&str] = &["windows", "linux", "macos"];
const CANONICAL_ARCHITECTURES: &[&str] = &["x86_64", "x86_32", "arm64"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoLaunchConfiguration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
    #[serde(default)]
    pub actions: Vec<RepoLaunchAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoLaunchAction {
    pub name: String,
    #[serde(default)]
    pub commands: Vec<RepoLaunchCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoLaunchCommand {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<LaunchPlatform>,
    pub executable: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchPlatform {
    pub os: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub architectures: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedLaunchConfiguration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
    #[serde(default)]
    pub actions: Vec<ResolvedLaunchAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedLaunchAction {
    pub name: String,
    pub executable: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
}

pub fn validate_repo_launch_configuration(configuration: &RepoLaunchConfiguration) -> Result<()> {
    validate_action_names_and_primary(
        configuration
            .actions
            .iter()
            .map(|action| action.name.as_str()),
        configuration.primary.as_deref(),
        false,
    )?;

    for action in &configuration.actions {
        if action.commands.is_empty() {
            return Err(Error::manifest(format!(
                "launch action '{}' must contain at least one command",
                action.name
            )));
        }
        for command in &action.commands {
            validate_repo_launch_command(command)?;
        }
        for (index, command) in action.commands.iter().enumerate() {
            for other in action.commands.iter().skip(index + 1) {
                if platform_selectors_overlap(command.platform.as_ref(), other.platform.as_ref()) {
                    return Err(Error::manifest(format!(
                        "launch action '{}' has overlapping platform commands",
                        action.name
                    )));
                }
            }
        }
    }
    Ok(())
}

pub fn resolve_repo_launch_for_target(
    configuration: &RepoLaunchConfiguration,
    target: &TargetManifest,
) -> Result<ResolvedLaunchConfiguration> {
    validate_repo_launch_configuration(configuration)?;
    let target_os = target.target_tags.get_exclusive("os").unwrap_or_default();
    let target_arch = target.target_tags.get_exclusive("arch").unwrap_or_default();
    let mut actions = Vec::new();

    for action in &configuration.actions {
        let matching_commands = action
            .commands
            .iter()
            .filter(|command| command_matches_target(command, target_os, target_arch))
            .collect::<Vec<_>>();
        if matching_commands.len() > 1 {
            return Err(Error::manifest(format!(
                "launch action '{}' resolves to multiple commands for target '{}'",
                action.name, target.target_id
            )));
        }
        let Some(command) = matching_commands.first() else {
            continue;
        };
        actions.push(ResolvedLaunchAction {
            name: action.name.clone(),
            executable: command.executable.clone(),
            arguments: command.arguments.clone(),
            working_directory: command.working_directory.clone(),
        });
    }

    let resolved = ResolvedLaunchConfiguration {
        primary: if actions.is_empty() {
            None
        } else {
            configuration.primary.clone()
        },
        actions,
    };
    validate_resolved_launch_for_target(&resolved, target)?;
    Ok(resolved)
}

pub fn validate_resolved_launch_for_target(
    configuration: &ResolvedLaunchConfiguration,
    target: &TargetManifest,
) -> Result<()> {
    validate_action_names_and_primary(
        configuration
            .actions
            .iter()
            .map(|action| action.name.as_str()),
        configuration.primary.as_deref(),
        true,
    )?;

    let target_os = target.target_tags.get_exclusive("os").unwrap_or_default();
    let manifest_paths = target
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<HashSet<_>>();

    for action in &configuration.actions {
        validate_launch_path(&action.executable, false)?;
        validate_arguments(&action.arguments)?;
        if let Some(working_directory) = action.working_directory.as_deref() {
            validate_launch_path(working_directory, true)?;
            if working_directory != "." {
                let prefix = format!("{}/", working_directory.trim_end_matches('/'));
                if !manifest_paths.iter().any(|path| path.starts_with(&prefix)) {
                    return Err(Error::manifest(format!(
                        "launch action '{}' working directory '{}' does not exist in the target",
                        action.name, working_directory
                    )));
                }
            }
        }

        if manifest_paths.contains(action.executable.as_str()) {
            continue;
        }
        let app_bundle_prefix = format!("{}/", action.executable.trim_end_matches('/'));
        let is_macos_bundle = target_os == "macos"
            && action.executable.to_ascii_lowercase().ends_with(".app")
            && manifest_paths
                .iter()
                .any(|path| path.starts_with(&app_bundle_prefix));
        if !is_macos_bundle {
            return Err(Error::manifest(format!(
                "launch action '{}' executable '{}' is not contained in the target",
                action.name, action.executable
            )));
        }
        if action.working_directory.is_some() {
            return Err(Error::manifest(format!(
                "launch action '{}' must not set working_directory for a macOS app bundle",
                action.name
            )));
        }
    }
    Ok(())
}

pub fn validate_launch_path(value: &str, allow_install_root: bool) -> Result<()> {
    let value = value.trim();
    if value.is_empty() || value.contains('\0') || value.contains('\\') || value.starts_with('/') {
        return Err(Error::manifest(format!(
            "launch path must be a normalized relative path inside the installation: '{}'",
            value
        )));
    }
    if value == "." {
        if allow_install_root {
            return Ok(());
        }
        return Err(Error::manifest(
            "launch executable cannot be the installation root",
        ));
    }
    let mut segments = value.split('/');
    let Some(first) = segments.next() else {
        return Err(Error::manifest("launch path is empty"));
    };
    if first.ends_with(':')
        || first.is_empty()
        || first == "."
        || first == ".."
        || segments.any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(Error::manifest(format!(
            "launch path must stay inside the installation: '{}'",
            value
        )));
    }
    Ok(())
}

fn validate_repo_launch_command(command: &RepoLaunchCommand) -> Result<()> {
    validate_launch_path(&command.executable, false)?;
    validate_arguments(&command.arguments)?;
    if let Some(working_directory) = command.working_directory.as_deref() {
        validate_launch_path(working_directory, true)?;
    }
    if let Some(platform) = command.platform.as_ref() {
        if !CANONICAL_OPERATING_SYSTEMS.contains(&platform.os.as_str()) {
            return Err(Error::manifest(format!(
                "launch command has unsupported operating system '{}'",
                platform.os
            )));
        }
        let mut architectures = HashSet::new();
        for architecture in &platform.architectures {
            if !CANONICAL_ARCHITECTURES.contains(&architecture.as_str()) {
                return Err(Error::manifest(format!(
                    "launch command has unsupported architecture '{}'",
                    architecture
                )));
            }
            if !architectures.insert(architecture) {
                return Err(Error::manifest(format!(
                    "launch command repeats architecture '{}'",
                    architecture
                )));
            }
        }
    }
    Ok(())
}

fn validate_arguments(arguments: &[String]) -> Result<()> {
    if arguments.iter().any(|argument| argument.contains('\0')) {
        return Err(Error::manifest(
            "launch arguments must not contain NUL characters",
        ));
    }
    Ok(())
}

fn validate_action_names_and_primary<'a>(
    action_names: impl Iterator<Item = &'a str>,
    primary: Option<&str>,
    require_primary_for_multiple: bool,
) -> Result<()> {
    let names = action_names.collect::<Vec<_>>();
    let mut normalized_names = HashSet::new();
    for name in &names {
        if name.trim().is_empty() {
            return Err(Error::manifest("launch action name is required"));
        }
        if !normalized_names.insert(name.trim().to_lowercase()) {
            return Err(Error::manifest(format!(
                "launch action name '{}' is duplicated",
                name
            )));
        }
    }
    if names.is_empty() {
        if primary.is_some() {
            return Err(Error::manifest(
                "empty launch configuration must not set primary",
            ));
        }
        return Ok(());
    }
    if require_primary_for_multiple && names.len() > 1 && primary.is_none() {
        return Err(Error::manifest("multiple launch actions require primary"));
    }
    if let Some(primary) = primary
        && !names.contains(&primary)
    {
        return Err(Error::manifest(format!(
            "primary launch action '{}' does not exist",
            primary
        )));
    }
    Ok(())
}

fn command_matches_target(command: &RepoLaunchCommand, target_os: &str, target_arch: &str) -> bool {
    let Some(platform) = command.platform.as_ref() else {
        return true;
    };
    platform.os == target_os
        && (platform.architectures.is_empty()
            || platform
                .architectures
                .iter()
                .any(|architecture| architecture == target_arch))
}

fn platform_selectors_overlap(
    left: Option<&LaunchPlatform>,
    right: Option<&LaunchPlatform>,
) -> bool {
    let (Some(left), Some(right)) = (left, right) else {
        return true;
    };
    if left.os != right.os {
        return false;
    }
    left.architectures.is_empty()
        || right.architectures.is_empty()
        || left
            .architectures
            .iter()
            .any(|architecture| right.architectures.contains(architecture))
}

#[cfg(test)]
#[path = "launch_tests.rs"]
mod tests;
