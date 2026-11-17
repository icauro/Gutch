"""Operations: Capture two tagged trees, list them, compare readable/JSON output in both
directions, against itself and by shortened IDs, then compare with payload artifacts and the
index unavailable.

Expected: The list shows each ID with its tags. Sorted logical byte deltas preserve empty,
same-size, type and metadata changes; unchanged files are omitted, a unique ID prefix names
its snapshot, comparison needs only manifests and never changes the trees, and an unknown ID
is reported as missing.
"""

import json


def run(case):
    repo = case.init()
    before = case.payload / "before"
    after = case.payload / "after"
    before.mkdir()
    after.mkdir()
    for root in (before, after):
        (root / "unchanged").write_bytes(b"same")
        (root / "permissions").write_bytes(b"mode")
    (before / "removed").write_bytes(b"123")
    (before / "empty-removed").touch()
    (before / "same-size").write_bytes(b"abcd")
    (before / "type").write_bytes(b"12")
    (before / "grow").write_bytes(b"a")
    (after / "added-日本語").write_bytes(b"12345")
    (after / "empty-added").touch()
    (after / "same-size").write_bytes(b"wxyz")
    (after / "type").mkdir()
    (after / "grow").write_bytes(b"abcd")
    (after / "permissions").chmod(0o444)
    first = case.capture(repo, before, "--tag", "version=1.0")["snapshot_id"]
    second = case.capture(repo, after, "--tag", "version=1.1")["snapshot_id"]
    case.command("snapshot", "list", "--repo", repo)
    listed = (case.root / "commands" / f"{case.sequence:03d}-snapshot.log").read_text(
        encoding="utf-8"
    )
    assert f"{first}  version=1.0" in listed and f"{second}  version=1.1" in listed

    def diff(source, target, *, json_output=True, success=True):
        case.command(
            "snapshot",
            "diff",
            "--repo",
            repo,
            "--from",
            source,
            "--to",
            target,
            *(["--json"] if json_output else []),
            success=success,
        )
        output = (
            case.root / "commands" / f"{case.sequence:03d}-snapshot.log"
        ).read_text(encoding="utf-8")
        if not (json_output and success):
            return output
        result = json.loads(output)
        assert result.pop("ok") is True
        assert result.pop("output_version") == "1.0.0"
        return result

    expected = [
        ("added-日本語", "added", 5),
        ("empty-added", "added", 0),
        ("empty-removed", "removed", 0),
        ("grow", "modified", 3),
        ("permissions", "properties_changed", 0),
        ("removed", "removed", -3),
        ("same-size", "modified", 0),
        ("type", "type_changed", -2),
    ]
    result = diff(first, second)
    assert result == {
        "changes": [
            dict(path=path, change=change, delta_bytes=delta)
            for path, change, delta in expected
        ],
        "delta_bytes": 3,
    }
    readable = diff(first, second, json_output=False)
    for path, change, delta in expected:
        line = next(line for line in readable.splitlines() if f'"{path}"' in line)
        assert line.startswith(change) and f"{delta:+}" in line
    assert "8 changes; total +3 B" in readable
    reverse = diff(second, first)
    assert reverse["delta_bytes"] == -3
    inverse = {
        "added": "removed",
        "removed": "added",
        "modified": "modified",
        "properties_changed": "properties_changed",
        "type_changed": "type_changed",
    }
    assert reverse["changes"] == [
        dict(path=path, change=inverse[change], delta_bytes=-delta)
        for path, change, delta in expected
    ]
    assert diff(first, first) == {"changes": [], "delta_bytes": 0}
    shared = next(i for i in range(64) if first[i] != second[i])
    assert diff(first[: shared + 1].upper(), second[: shared + 4]) == result
    # Move only this case's generated artifacts aside; a tree diff must not read them.
    for name in ("bundles", "chunks", "chunk_map.gtcbin"):
        path = repo / name
        if path.exists():
            path.rename(case.payload / f"unavailable-{name}")
    assert diff(first, second) == result
    assert "no snapshot" in diff(first, "f" * 64, success=False)
    assert (before / "same-size").read_bytes() == b"abcd"
    assert (after / "same-size").read_bytes() == b"wxyz"
    case.checks.append("snapshot-diff-json-readable-and-manifest-only")
