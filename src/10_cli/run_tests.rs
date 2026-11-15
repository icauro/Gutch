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
