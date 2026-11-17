"""Operations: create an encrypted repository, capture, add a key file, then reconstruct with
each credential, a wrong one and none.

Expected: stored files hold no plaintext, names or tags; every added credential opens the
repository; a wrong or missing credential fails without output.
"""

from tests.datasets.generate import basic


def run(case):
    case.variables["GUTCH_REPO_PASSWORD"] = "first password"
    repo = case.payload / "repo"
    case.command("repo", "init", "--repo", repo, "--encrypt")
    dataset = basic(case.payload / "source")
    tag = "codename=bluebird"
    snapshot = case.capture(repo, dataset.root, "--tag", tag)["snapshot_id"]
    secret = next(path for path in dataset.root.rglob("*") if path.is_file())
    for path in (path for path in repo.rglob("*") if path.is_file()):
        data = path.read_bytes()
        assert secret.name not in str(path)
        assert b"bluebird" not in data and secret.name.encode() not in data

    key_file = case.payload / "repo.key"
    case.command(
        "repo", "credential", "add", "--repo", repo, "--new-key-file", key_file
    )
    case.reconstruct(repo, snapshot, case.payload / "with_password")
    case.compare(dataset, case.payload / "with_password", "with_password")

    del case.variables["GUTCH_REPO_PASSWORD"]
    case.reconstruct(repo, snapshot, case.payload / "locked", success=False)
    case.variables["GUTCH_REPO_PASSWORD"] = "wrong password"
    case.reconstruct(repo, snapshot, case.payload / "wrong", success=False)
    assert not (case.payload / "locked").exists()
    assert not (case.payload / "wrong").exists()

    del case.variables["GUTCH_REPO_PASSWORD"]
    case.variables["GUTCH_REPO_KEY_FILE"] = str(key_file)
    case.reconstruct(repo, snapshot, case.payload / "with_key_file")
    case.compare(dataset, case.payload / "with_key_file", "with_key_file")
