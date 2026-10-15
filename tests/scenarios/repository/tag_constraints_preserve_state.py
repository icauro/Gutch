"""Operations: Required tags, formats, parity, atomic retagging and rule tightening.

Expected: Rejected operations preserve the repository; bulk changes validate final state.
"""

import json
from tests.helpers.filesystem import repository_inventory


def run(case):
    repo = case.init()
    source = case.payload / "source"
    source.mkdir()
    (source / "file").write_bytes(b"first")
    first = case.capture(repo, source, "--tag", "version=1.0.0.0")["snapshot_id"]
    (source / "file").write_bytes(b"second")
    second = case.capture(repo, source, "--tag", "version=2.0.0.0")["snapshot_id"]
    constraints = {
        "snapshot_tags": {
            "key_value": {"version": {"required": True, "format": "u32.u32.u32.u32"}}
        },
        "require_matching_tag_keys_across_snapshots": True,
    }
    rules_file = case.payload / "constraints.json"
    rules_file.write_text(json.dumps(constraints), encoding="utf-8")
    case.command(
        "repo", "constraints", "set", "--repo", repo, "--constraints-file", rules_file
    )
    before = repository_inventory(repo)
    for value in ["1.0.0", "4294967296.0.0.0", "01.0.0.0"]:
        case.command(
            "snapshot",
            "tags",
            "replace",
            "--repo",
            repo,
            "--snapshot",
            first,
            "--tag",
            f"version={value}",
            "--yes",
            success=False,
        )
        assert repository_inventory(repo) == before
    case.command(
        "snapshot",
        "capture",
        "--repo",
        repo,
        "--source",
        source,
        "--yes",
        success=False,
    )
    assert repository_inventory(repo) == before
    case.command(
        "snapshot",
        "tags",
        "replace",
        "--repo",
        repo,
        "--snapshot",
        first,
        "--tag",
        "version=1.0.0.0",
        "--tag",
        "released_at=2024_02_29",
        "--yes",
        success=False,
    )
    assert repository_inventory(repo) == before
    replacements = {
        first: {"key_value": {"version": "1.0.0.0", "released_at": "2024_02_29"}},
        second: {"key_value": {"version": "2.0.0.0", "released_at": "2025_01_01"}},
    }
    tags_file = case.payload / "tags.json"
    tags_file.write_text(json.dumps(replacements), encoding="utf-8")
    case.command(
        "snapshot",
        "tags",
        "replace",
        "--repo",
        repo,
        "--tags-file",
        tags_file,
        "--dry-run",
        "--yes",
    )
    assert repository_inventory(repo) == before
    case.command(
        "snapshot", "tags", "replace", "--repo", repo, "--tags-file", tags_file, "--yes"
    )
    for snapshot, tags in replacements.items():
        metadata = json.loads(
            (repo / "snapshots" / snapshot / "snapshot_metadata.json").read_text(
                encoding="utf-8"
            )
        )
        assert metadata["tags"] == tags
    constraints["snapshot_tags"]["key_value"]["released_at"] = {
        "required": True,
        "format": "yyyy_mm_dd",
    }
    rules_file.write_text(json.dumps(constraints), encoding="utf-8")
    case.command(
        "repo", "constraints", "set", "--repo", repo, "--constraints-file", rules_file
    )
    before = repository_inventory(repo)
    replacements[second]["key_value"]["released_at"] = "2025_02_29"
    tags_file.write_text(json.dumps(replacements), encoding="utf-8")
    case.command(
        "snapshot",
        "tags",
        "replace",
        "--repo",
        repo,
        "--tags-file",
        tags_file,
        "--yes",
        success=False,
    )
    assert repository_inventory(repo) == before
    constraints["snapshot_tags"]["key_value"]["channel"] = {"required": True}
    rules_file.write_text(json.dumps(constraints), encoding="utf-8")
    case.command(
        "repo",
        "constraints",
        "set",
        "--repo",
        repo,
        "--constraints-file",
        rules_file,
        success=False,
    )
    assert repository_inventory(repo) == before
    case.command("repo", "verify", "--repo", repo)
