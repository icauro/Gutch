"""Operations: Replace blueprint directory paths with traversal, absolute and invalid-separator
paths.

Expected: Every reconstruction fails without modifying the run-owned filesystem.
"""

import os
from tests.helpers.binary_metadata import directory_blueprint
from tests.datasets.generate import basic
from tests.helpers.filesystem import inventory


def run(case):
    repo = case.init()
    data = basic(case.payload / "source")
    snapshot = case.capture(repo, data.root)["snapshot_id"]
    blueprint = repo / "snapshots" / snapshot / "snapshot_blueprint.gtcbin"
    original = blueprint.read_bytes()
    paths = [
        "../outside",
        "/outside",
        "C:/outside",
        "nested\\outside",
        "a/./b",
        "a" * (128 * 1024 + 1),
        "/".join(["a"] * 257),
        "a" * 256,
    ]
    if os.name == "nt":
        paths += ["CON", "nul.txt", "name.", "name ", "stream:ads", "wild?card"]
    for path in paths:
        blueprint.write_bytes(directory_blueprint(path))
        destination = case.payload / "installed"
        before = inventory(case.payload)
        case.command(
            "installation",
            "reconstruct",
            "--repo",
            repo,
            "--snapshot",
            snapshot,
            "--destination",
            destination,
            success=False,
        )
        assert inventory(case.payload) == before
    blueprint.write_bytes(original)

    if os.name != "nt":
        from tests.helpers.filesystem import repository_inventory

        for name in (os.fsdecode(b"invalid-\xff"), "literal\\backslash"):
            bad = data.root / name
            bad.write_bytes(b"must not be silently renamed")
            before = repository_inventory(repo)
            case.command(
                "snapshot",
                "capture",
                "--repo",
                repo,
                "--source",
                data.root,
                success=False,
            )
            assert repository_inventory(repo) == before
            bad.unlink()
