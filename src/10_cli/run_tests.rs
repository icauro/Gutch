use super::requires_remote_provider;
use super::run_from_args;

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
            "--error-json-out",
            &path("error.json"),
        ])
        .is_err()
    );
    let failed: serde_json::Value = serde_json::from_str(&read("failed.json")).unwrap();
    assert_eq!(failed["output_version"], "1.0.0");
    assert_eq!(failed["ok"], false);
    assert_eq!(version(&read("error.json")), "1.0.0");
}
