use super::super::InstallationCommands;
use super::super::RemoteCommands;
use super::super::RepoCommands;
use super::super::shared::ConfirmArgs;
use super::super::shared::TransferPolicyArg;
use super::Cli;
use super::Commands;
use super::DebugCommands;
use clap::Parser;
use std::env::current_dir;
use std::iter;

#[test]
fn parses_provider_neutral_remote_commands() {
    let cli = Cli::try_parse_from([
        "gutch",
        "repo",
        "pull",
        "--repo",
        if cfg!(windows) {
            r"D:\Repos\reconstruct"
        } else {
            "/repos/reconstruct"
        },
        "--remote-repo-id",
        "scope/repo",
        "--on-conflict",
        "keep-local",
        "--prune",
        "--yes",
    ])
    .expect("remote pull args should parse");

    assert!(matches!(
        cli.command,
        Commands::Repo {
            command: RepoCommands::Pull {
                prune: true,
                confirm: ConfirmArgs {
                    dry_run: false,
                    yes: true
                },
                ..
            }
        }
    ));
}

#[test]
fn parses_generic_scope_and_tag_reference_commands() {
    let cli = Cli::try_parse_from([
        "gutch",
        "repo",
        "remote",
        "create",
        "--repo",
        if cfg!(windows) {
            r"D:\Repos\creator"
        } else {
            "/repos/creator"
        },
        "--scope-id",
        "creator-scope",
    ])
    .expect("generic link args should parse");
    assert!(matches!(
        cli.command,
        Commands::Repo {
            command: RepoCommands::Remote { command: RemoteCommands::Create { scope_id, .. } }
        } if scope_id == "creator-scope"
    ));

    let cli = Cli::try_parse_from([
        "gutch",
        "repo",
        "remote",
        "bind",
        "--repo",
        if cfg!(windows) {
            r"D:\Repos\creator"
        } else {
            "/repos/creator"
        },
        "--remote-repo-id",
        "remote-repository",
    ])
    .expect("generic bind args should parse");
    assert!(matches!(
        cli.command,
        Commands::Repo {
            command: RepoCommands::Remote { command: RemoteCommands::Bind { remote_repo_id, .. } }
        } if remote_repo_id == "remote-repository"
    ));

    let cli = Cli::try_parse_from([
        "gutch",
        "repo",
        "remote",
        "tag-references",
        "--repo",
        if cfg!(windows) {
            r"D:\Repos\creator"
        } else {
            "/repos/creator"
        },
    ])
    .expect("tag reference args should parse");
    assert!(matches!(
        cli.command,
        Commands::Repo {
            command: RepoCommands::Remote {
                command: RemoteCommands::TagReferences { .. }
            }
        }
    ));

    let cli = Cli::try_parse_from(["gutch", "repo", "remote", "list"])
        .expect("remote list args should parse");
    assert!(matches!(
        cli.command,
        Commands::Repo {
            command: RepoCommands::Remote {
                command: RemoteCommands::List
            }
        }
    ));
}

#[test]
fn parses_reconstruction_with_fetch() {
    let cli = Cli::try_parse_from([
        "gutch",
        "installation",
        "reconstruct",
        "--fetch",
        "--repo",
        if cfg!(windows) {
            r"D:\Repos\fetched"
        } else {
            "/repos/fetched"
        },
        "--snapshot",
        "snapshot-1",
        "--destination",
        if cfg!(windows) {
            r"D:\Installs\product"
        } else {
            "/installs/product"
        },
        "--snapshot-metadata-url",
        "https://objects.example/snapshot.json",
        "--blueprint-url",
        "https://objects.example/blueprint.gtcbin",
        "--artifact-url",
        "chunk-1=https://objects.example/chunk",
        "--transfer-policy",
        "adaptive",
        "--transfer-concurrency",
        "4",
        "--transfer-max-concurrency",
        "12",
    ])
    .expect("reconstruction fetch args should parse");

    let Commands::Installation {
        command: InstallationCommands::Reconstruct(arguments),
    } = cli.command
    else {
        panic!("expected reconstruction with fetch");
    };
    assert!(arguments.fetch);
    assert!(matches!(
        arguments.transfer.policy,
        TransferPolicyArg::Adaptive
    ));
    assert_eq!(arguments.transfer.concurrency, 4);
    assert_eq!(arguments.transfer.maximum, 12);
    assert_eq!(
        arguments.sources.artifacts,
        ["chunk-1=https://objects.example/chunk"]
    );
}

#[test]
fn rejects_retired_command_group() {
    let error = Cli::try_parse_from(["gutch", "reconstructed", "--help"])
        .err()
        .expect("the retired command group must be rejected");
    assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
}

#[test]
fn dry_run_is_accepted_only_by_commands_that_change_state() {
    for args in [
        &["repo", "init", "--repo", "repo", "--dry-run"][..],
        &["repo", "index", "rebuild", "--repo", "repo", "--dry-run"],
        &[
            "snapshot",
            "delete",
            "--repo",
            "repo",
            "--snapshot",
            "s",
            "--dry-run",
        ],
        &[
            "installation",
            "reconstruct",
            "--repo",
            "repo",
            "--snapshot",
            "s",
            "--destination",
            "out",
            "--dry-run",
        ],
    ] {
        Cli::try_parse_from(iter::once("gutch").chain(args.iter().copied()))
            .unwrap_or_else(|error| panic!("{args:?} should parse: {error}"));
    }
    for args in [
        &["snapshot", "list", "--repo", "repo", "--dry-run"][..],
        &["repo", "status", "--repo", "repo", "--dry-run"],
        &["--dry-run", "repo", "init", "--repo", "repo"],
    ] {
        let error = Cli::try_parse_from(iter::once("gutch").chain(args.iter().copied()))
            .err()
            .unwrap_or_else(|| panic!("{args:?} must reject --dry-run"));
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::UnknownArgument,
            "{args:?}"
        );
    }
}

#[test]
fn dry_run_conflicts_with_yes_and_fetch() {
    for args in [
        &[
            "snapshot",
            "delete",
            "--repo",
            "repo",
            "--snapshot",
            "s",
            "--dry-run",
            "--yes",
        ][..],
        &[
            "installation",
            "reconstruct",
            "--repo",
            "repo",
            "--snapshot",
            "s",
            "--destination",
            "out",
            "--fetch",
            "--dry-run",
        ],
    ] {
        let error = Cli::try_parse_from(iter::once("gutch").chain(args.iter().copied()))
            .err()
            .unwrap_or_else(|| panic!("{args:?} must be rejected"));
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::ArgumentConflict,
            "{args:?}"
        );
    }
}

#[test]
fn reconstruction_requires_explicit_fetch_and_complete_metadata_sources() {
    let base = [
        "gutch",
        "installation",
        "reconstruct",
        "--repo",
        "repo",
        "--snapshot",
        "snapshot",
        "--destination",
        "install",
    ];
    Cli::try_parse_from(base).expect("local reconstruction needs no remote inputs");
    for options in [
        vec!["--fetch"],
        vec![
            "--fetch",
            "--snapshot-metadata-url",
            "https://example.invalid/meta",
        ],
        vec!["--fetch", "--blueprint-url", "https://example.invalid/tree"],
        vec![
            "--snapshot-metadata-url",
            "https://example.invalid/meta",
            "--blueprint-url",
            "https://example.invalid/tree",
        ],
        vec!["--artifact-url", "id=https://example.invalid/artifact"],
    ] {
        let error = Cli::try_parse_from(base.into_iter().chain(options))
            .err()
            .expect("incomplete or implicit fetch must fail during parsing");
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
    }
}

#[test]
fn rejects_retired_operation_names() {
    for command in [
        vec!["snapshot", "reconstruct"],
        vec!["snapshot", "fetch-reconstruct"],
        vec!["snapshot", "estimate"],
        vec!["snapshot", "retag"],
        vec!["installation", "plan"],
        vec!["repo", "rebuild-index"],
        vec!["path", "compare"],
        vec!["tree", "compare"],
        vec!["bundle", "inspect"],
        vec!["repo", "tags"],
    ] {
        let error = Cli::try_parse_from(iter::once("gutch").chain(command))
            .err()
            .expect("retired command names have no aliases");
        assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }
}

#[test]
fn parses_installation_destination_commands() {
    let destination = if cfg!(windows) {
        r"D:\Installs\product"
    } else {
        "/installs/product"
    };
    let repo = if cfg!(windows) {
        r"D:\Repos\product"
    } else {
        "/repos/product"
    };

    let cli = Cli::try_parse_from([
        "gutch",
        "installation",
        "verify",
        "--repo",
        repo,
        "--snapshot",
        "snapshot-1",
        "--destination",
        destination,
    ])
    .expect("installation verify args should parse");
    assert!(matches!(
        cli.command,
        Commands::Installation {
            command: InstallationCommands::Verify { strict: false, .. }
        }
    ));

    let cli = Cli::try_parse_from([
        "gutch",
        "installation",
        "repair",
        "--repo",
        repo,
        "--snapshot",
        "snapshot-1",
        "--destination",
        destination,
        "--remove-extra-files",
        "--yes",
    ])
    .expect("installation repair args should parse");
    assert!(matches!(
        cli.command,
        Commands::Installation {
            command: InstallationCommands::Repair {
                remove_extra_files: true,
                confirm: ConfirmArgs { yes: true, .. },
                ..
            }
        }
    ));

    let plan = if cfg!(windows) {
        r"D:\Plans\corrupt.json"
    } else {
        "/plans/corrupt.json"
    };
    let cli = Cli::try_parse_from([
        "gutch",
        "debug",
        "corrupt-installation",
        "--destination",
        destination,
        "--plan",
        plan,
        "--yes",
    ])
    .expect("installation corrupt args should parse");
    assert!(matches!(
        cli.command,
        Commands::Debug {
            command: DebugCommands::CorruptInstallation { yes: true, .. }
        }
    ));
}

#[test]
fn cli_resolves_relative_filesystem_paths() {
    let cli =
        Cli::try_parse_from(["gutch-cli", "repo", "status", "--repo", "relative/repo"]).unwrap();
    let Commands::Repo {
        command: super::super::RepoCommands::Status { repo, .. },
    } = cli.command
    else {
        panic!("expected status");
    };
    assert_eq!(repo, current_dir().unwrap().join("relative/repo"));
}

#[test]
fn cli_accepts_absolute_filesystem_paths() {
    let repo = if cfg!(windows) {
        r"D:\Repos\game"
    } else {
        "/repos/game"
    };
    Cli::try_parse_from(["gutch", "repo", "status", "--repo", repo]).expect("absolute repo path");
}

#[test]
fn cli_resolves_absolute_and_relative_profile_output() {
    let trace_path = if cfg!(windows) {
        r"D:\Artifacts\gutch.trace.json"
    } else {
        "/artifacts/gutch.trace.json"
    };
    let cli = Cli::try_parse_from([
        "gutch",
        "repo",
        "remote",
        "list",
        "--profile-dir",
        trace_path,
    ])
    .expect("absolute trace path");
    assert_eq!(
        cli.profile_out.as_deref().unwrap().to_string_lossy(),
        trace_path
    );

    let cli = Cli::try_parse_from([
        "gutch",
        "repo",
        "remote",
        "list",
        "--profile-dir",
        "relative.trace.json",
    ])
    .unwrap();
    assert_eq!(
        cli.profile_out.unwrap(),
        current_dir().unwrap().join("relative.trace.json")
    );
}

#[test]
fn reconstruct_fetch_sources_require_fetch() {
    for source in ["--chunk-index-url", "--session-id", "--artifact-url"] {
        let error = Cli::try_parse_from([
            "gutch",
            "installation",
            "reconstruct",
            "--repo",
            "repo",
            "--snapshot",
            "snapshot",
            "--destination",
            "out",
            source,
            "value",
        ])
        .err()
        .unwrap_or_else(|| panic!("{source} without --fetch should be rejected"));
        assert!(error.to_string().contains("--fetch"), "{source}: {error}");
    }
}

#[test]
fn every_command_and_option_explains_itself() {
    fn missing(command: &clap::Command, path: &str, found: &mut Vec<String>) {
        let path = format!("{path} {}", command.get_name());
        if command.get_about().is_none() {
            found.push(path.clone());
        }
        for arg in command.get_arguments() {
            if arg.get_help().is_none() && !matches!(arg.get_id().as_str(), "help" | "version") {
                found.push(format!("{path} --{}", arg.get_id()));
            }
        }
        for subcommand in command.get_subcommands() {
            missing(subcommand, &path, found);
        }
    }
    use clap::CommandFactory;
    let mut found = Vec::new();
    missing(&Cli::command(), "", &mut found);
    assert!(found.is_empty(), "missing help: {found:#?}");
}
