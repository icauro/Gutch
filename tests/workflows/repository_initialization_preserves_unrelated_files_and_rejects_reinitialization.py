"""Operations: Initialize a nonempty folder normally and with force, then attempt reinitialization.

Expected: Unsafe initialization fails, forced initialization preserves unrelated files, and
reinitialization is inert.
"""
from ..support.filesystem import inventory


def run(case):
    repo = case.payload / "nonempty"
    repo.mkdir()
    (repo / "keep.txt").write_bytes(b"unrelated")
    case.command("repo", "init", "--path", repo, success=False)
    assert (repo / "keep.txt").read_bytes() == b"unrelated"
    case.command("repo", "init", "--path", repo, "--force")
    assert (repo / "keep.txt").read_bytes() == b"unrelated"
    before = inventory(repo)
    case.command("repo", "init", "--path", repo, success=False)
    assert inventory(repo) == before
