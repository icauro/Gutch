use super::{CommandLineHost, run_command_line};

fn run_from_args<const N: usize>(args: [&str; N]) -> crate::Result<()> {
    run_command_line(args, &CommandLineHost::default())
}

#[test]
fn every_structured_output_carries_the_output_version() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let path = |name: &str| temp.path().join(name).to_str().unwrap().to_owned();
    let read = |name: &str| std::fs::read_to_string(temp.path().join(name)).unwrap();
    let version = |line: &str| {
        serde_json::from_str::<serde_json::Value>(line).unwrap()["output_version"].clone()
    };
    let repo_arg = repo.to_str().unwrap();
    run_from_args([
        "gutch",
        "repo",
        "init",
        "--repo",
        repo_arg,
        "--json",
        "--output",
        &path("init.json"),
    ])
    .unwrap();
    assert_eq!(version(&read("init.json")), "1.0.0");

    run_from_args([
        "gutch",
        "snapshot",
        "list",
        "--repo",
        repo_arg,
        "--json-lines",
        "--output",
        &path("list.jsonl"),
    ])
    .unwrap();
    let lines = read("list.jsonl");
    assert!(!lines.trim().is_empty());
    for line in lines.lines() {
        assert_eq!(version(line), "1.0.0", "{line}");
    }

    let missing = temp.path().join("missing");
    assert!(
        run_from_args([
            "gutch",
            "snapshot",
            "list",
            "--repo",
            missing.to_str().unwrap(),
            "--json",
            "--output",
            &path("failed.json"),
            "--error-output",
            &path("error.json"),
        ])
        .is_err()
    );
    let failed: serde_json::Value = serde_json::from_str(&read("failed.json")).unwrap();
    assert_eq!(failed["output_version"], "1.0.0");
    assert_eq!(failed["ok"], false);
    assert_eq!(version(&read("error.json")), "1.0.0");
}

#[test]
fn the_remote_provider_is_created_only_for_remote_commands() {
    use std::cell::Cell;
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let repo = repo.to_str().unwrap();
    let created = Cell::new(0);
    let factory = || -> crate::Result<Box<dyn crate::sync::remote::RemoteRepoService>> {
        created.set(created.get() + 1);
        Err(crate::Error::invalid_input("no session"))
    };
    let host = CommandLineHost {
        bin_name: "host",
        remote_provider: Some(&factory),
    };
    run_command_line(["host", "repo", "init", "--repo", repo], &host).unwrap();
    run_command_line(["host", "snapshot", "list", "--repo", repo], &host).unwrap();
    assert_eq!(created.get(), 0);
    let error = run_command_line(["host", "repo", "push", "--repo", repo], &host).unwrap_err();
    assert!(error.to_string().contains("no session"), "{error}");
    assert_eq!(created.get(), 1);

    let error = run_from_args(["gutch", "repo", "remote", "list"]).unwrap_err();
    assert!(error.to_string().contains("remote provider"), "{error}");
}
