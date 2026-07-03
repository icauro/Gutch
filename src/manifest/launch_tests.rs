use super::*;
use crate::manifest::{FileManifest, TargetTagSet, target::TargetManifest};

fn target(os: &str, arch: &str, paths: &[&str]) -> TargetManifest {
    let mut tags = TargetTagSet::default();
    tags.upsert_exclusive("os", os);
    tags.upsert_exclusive("arch", arch);
    let mut target = TargetManifest::new("target".into(), tags);
    target.files = paths
        .iter()
        .map(|path| FileManifest {
            path: (*path).to_string(),
            size: 1,
            executable: true,
            full_file_hash: String::new(),
            chunks: Vec::new(),
        })
        .collect();
    target
}

fn command(os: &str, architectures: &[&str], executable: &str) -> RepoLaunchCommand {
    RepoLaunchCommand {
        platform: Some(LaunchPlatform {
            os: os.into(),
            architectures: architectures.iter().map(|value| (*value).into()).collect(),
        }),
        executable: executable.into(),
        arguments: Vec::new(),
        working_directory: None,
    }
}

#[test]
fn resolves_only_the_matching_platform_command() {
    let configuration = RepoLaunchConfiguration {
        primary: None,
        actions: vec![RepoLaunchAction {
            name: "Play".into(),
            commands: vec![
                command("windows", &["x86_64"], "Game.exe"),
                command("linux", &["x86_64"], "game"),
            ],
        }],
    };
    let resolved =
        resolve_repo_launch_for_target(&configuration, &target("windows", "x86_64", &["Game.exe"]))
            .unwrap();
    assert_eq!(resolved.actions.len(), 1);
    assert_eq!(resolved.actions[0].executable, "Game.exe");
}

#[test]
fn rejects_absolute_and_traversal_paths() {
    for path in [
        "../outside.exe",
        "bin/../../outside.exe",
        "C:/outside.exe",
        "\\\\server\\game.exe",
        "/outside",
    ] {
        assert!(
            validate_launch_path(path, false).is_err(),
            "accepted {path}"
        );
    }
}

#[test]
fn validates_executable_is_in_target_manifest() {
    let configuration = ResolvedLaunchConfiguration {
        primary: None,
        actions: vec![ResolvedLaunchAction {
            name: "Play".into(),
            executable: "Missing.exe".into(),
            arguments: Vec::new(),
            working_directory: None,
        }],
    };
    assert!(
        validate_resolved_launch_for_target(
            &configuration,
            &target("windows", "x86_64", &["Game.exe"]),
        )
        .is_err()
    );
}

#[test]
fn accepts_macos_app_bundle_and_rejects_working_directory_for_it() {
    let target = target(
        "macos",
        "arm64",
        &[
            "Game.app/Contents/MacOS/Game",
            "Game.app/Contents/Info.plist",
        ],
    );
    let mut configuration = ResolvedLaunchConfiguration {
        primary: None,
        actions: vec![ResolvedLaunchAction {
            name: "Play".into(),
            executable: "Game.app".into(),
            arguments: Vec::new(),
            working_directory: None,
        }],
    };
    validate_resolved_launch_for_target(&configuration, &target).unwrap();
    configuration.actions[0].working_directory = Some(".".into());
    assert!(validate_resolved_launch_for_target(&configuration, &target).is_err());
}

#[test]
fn rejects_overlapping_repo_commands() {
    let configuration = RepoLaunchConfiguration {
        primary: None,
        actions: vec![RepoLaunchAction {
            name: "Play".into(),
            commands: vec![
                command("windows", &[], "Game.exe"),
                command("windows", &["x86_64"], "Game64.exe"),
            ],
        }],
    };
    assert!(validate_repo_launch_configuration(&configuration).is_err());
}
