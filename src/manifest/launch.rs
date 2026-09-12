use std::collections::HashSet;

use serde::Deserialize;
use serde::Serialize;

use super::snapshot::SnapshotManifest;
use crate::Error;
use crate::Result;

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

pub(crate) fn validate_resolved_launch_for_snapshot(
    configuration: &ResolvedLaunchConfiguration,
    snapshot: &SnapshotManifest,
) -> Result<()> {
    validate_resolved_launch_for_snapshot_os(configuration, snapshot, None)
}

pub(crate) fn validate_resolved_launch_for_snapshot_os(
    configuration: &ResolvedLaunchConfiguration,
    snapshot: &SnapshotManifest,
    target_os: Option<&str>,
) -> Result<()> {
    validate_action_names_and_primary(
        configuration
            .actions
            .iter()
            .map(|action| action.name.as_str()),
        configuration.primary.as_deref(),
        true,
    )?;

    let snapshot_os =
        target_os.unwrap_or_else(|| snapshot.snapshot_tags.get("os").unwrap_or_default());
    let manifest_paths = snapshot
        .files()
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
                        "launch action '{}' working directory '{}' does not exist in the snapshot",
                        action.name, working_directory
                    )));
                }
            }
        }

        if manifest_paths.contains(action.executable.as_str()) {
            continue;
        }
        let app_bundle_prefix = format!("{}/", action.executable.trim_end_matches('/'));
        let is_macos_bundle = snapshot_os == "macos"
            && action.executable.to_ascii_lowercase().ends_with(".app")
            && manifest_paths
                .iter()
                .any(|path| path.starts_with(&app_bundle_prefix));
        if !is_macos_bundle {
            return Err(Error::manifest(format!(
                "launch action '{}' executable '{}' is not contained in the snapshot",
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

pub(crate) fn validate_launch_path(value: &str, allow_install_root: bool) -> Result<()> {
    crate::filesystem::paths::validate_manifest_relative_path(value, allow_install_root).map_err(
        |error| {
            Error::manifest(format!(
                "launch path must be normalized inside the installation: {error}"
            ))
        },
    )
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
