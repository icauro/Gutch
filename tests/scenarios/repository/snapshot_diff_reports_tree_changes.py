"""Operations: Capture two trees, compare readable/JSON output in both directions and against
itself, then compare with payload artifacts and the index unavailable.

Expected: Sorted logical byte deltas preserve empty, same-size, type and metadata changes;
unchanged files are omitted, comparison needs only manifests and never changes the trees.
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
    first = case.capture(repo, before)["snapshot_id"]
    second = case.capture(repo, after)["snapshot_id"]

    def diff(source, target, *, json_output=True, success=True):
        case.command("snapshot", "diff", "--repo", repo, "--from", source, "--to", target,
                     *(["--json"] if json_output else []), success=success)
        output = (case.root / "commands" / f"{case.sequence:03d}-snapshot.log").read_text(encoding="utf-8")
        return json.loads(output) if json_output and success else output

    expected = [("added-日本語", "added", 5), ("empty-added", "added", 0),
                ("empty-removed", "removed", 0), ("grow", "modified", 3),
                ("permissions", "metadata", 0), ("removed", "removed", -3),
                ("same-size", "modified", 0), ("type", "modified", -2)]
    result = diff(first, second)
    assert result == {"changes": [dict(path=path, change=change, delta_bytes=delta)
                                  for path, change, delta in expected], "delta_bytes": 3}
    readable = diff(first, second, json_output=False)
    for path, change, delta in expected:
        line = next(line for line in readable.splitlines() if f'"{path}"' in line)
        assert line.startswith(change.title()) and f"{delta:+}" in line
    assert "8 changes; total +3 B" in readable
    reverse = diff(second, first)
    assert reverse["delta_bytes"] == -3
    inverse = {"added": "removed", "removed": "added", "modified": "modified", "metadata": "metadata"}
    assert reverse["changes"] == [dict(path=path, change=inverse[change], delta_bytes=-delta)
                                   for path, change, delta in expected]
    assert diff(first, first) == {"changes": [], "delta_bytes": 0}
    # Move only this case's generated artifacts aside; a tree diff must not read them.
    for name in ("bundles", "chunks", "chunk_map.gtcbin"):
        path = repo / name
        if path.exists():
            path.rename(case.payload / f"unavailable-{name}")
    assert diff(first, second) == result
    diff(first, "f" * 64, success=False)
    assert (before / "same-size").read_bytes() == b"abcd"
    assert (after / "same-size").read_bytes() == b"wxyz"
    case.checks.append("snapshot-diff-json-readable-and-manifest-only")
