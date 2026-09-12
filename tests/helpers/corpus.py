"""Explicitly freeze source datasets; normal regression runs only read this corpus."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import tomllib
from uuid import uuid4

from tests.datasets.generate import CORPUS_ROOT, TESTS_ROOT, load_case, prepare_case_source
from tests.helpers.cli import Case, REPOSITORY
from tests.helpers.filesystem import assert_tree, inventory, repository_inventory, write_json


def repository_input(name, version="v1"):
    definition = load_case(name)
    if version not in definition["repositories"] or not re.fullmatch(r"v[1-9][0-9]*", version):
        raise ValueError(f"no frozen {version} repository for {name}")
    root = CORPUS_ROOT / name
    record = definition["repositories"][version]
    expected = json.loads((root / "expected.json").read_text(encoding="utf-8"))
    return root / "repository" / f"format-{version}", {
        snapshot: expected[source] for snapshot, source in record["snapshots"].items()}


def freeze(name, version, executable, source=None):
    if not re.fullmatch(r"v[1-9][0-9]*", version):
        raise ValueError("repository version must be v1, v2, ...")
    definition = load_case(name)
    destination = CORPUS_ROOT / name / "repository" / f"format-{version}"
    if destination.exists() or version in definition["repositories"]:
        raise ValueError(f"frozen repository already exists: {destination}")
    source = source.resolve() if source else prepare_case_source(name)
    settings = definition["source"]
    names = ([f"v{index}" for index in range(1, settings["versions"] + 1)]
             if settings["kind"] == "generated" else [
                 f"blender-{release}-{platform}" for release in settings["selected_versions"]
                 for platform in settings["selected_platforms"]])
    work = TESTS_ROOT / ".artifacts/tasks" / ("freeze-" + uuid4().hex[:10])
    case = Case(work, executable.resolve())
    repo = case.init()
    expected = {}
    snapshots = {}
    for index, name_part in enumerate(names, 1):
        tree = source / name_part
        if not tree.is_dir():
            raise ValueError(f"missing source version: {tree}")
        # This inventory is derived from original source bytes, before Gutch runs.
        expected[name_part] = {"entries": inventory(tree)}
        captured = case.capture(repo, tree, "--compression-algorithm", "zstd",
                                "--tag", f"version::{index}.0.0")
        snapshot = captured["snapshot_id"]
        snapshots[snapshot] = name_part
        restored = case.payload / "restored" / name_part
        case.reconstruct(repo, snapshot, restored)
        assert_tree(expected[name_part]["entries"], restored, work / f"{index}-comparison.json")
        assert inventory(tree) == expected[name_part]["entries"], "source changed during capture"
    expected_path = destination.parents[1] / "expected.json"
    if expected_path.exists():
        assert json.loads(expected_path.read_text(encoding="utf-8")) == expected, "source inventory changed across major versions"
    try:
        engine_commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPOSITORY,
                                               text=True, stderr=subprocess.DEVNULL).strip()
    except subprocess.CalledProcessError:
        engine_commit = None
    engine_source = hashlib.sha256()
    for path in sorted([REPOSITORY / "Cargo.toml", REPOSITORY / "Cargo.lock", *REPOSITORY.glob("src/**/*.rs")]):
        engine_source.update(path.relative_to(REPOSITORY).as_posix().encode())
        engine_source.update(path.read_bytes())
    definition["repositories"][version] = {
        "creator": {"crate_version": tomllib.loads((REPOSITORY / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"],
                    "engine_commit": engine_commit,
                    "engine_source_sha256": engine_source.hexdigest(),
                    "binary_sha256": hashlib.sha256(executable.read_bytes()).hexdigest()},
        "snapshots": snapshots,
    }
    limit = definition.get("max_case_bytes")
    # Include source, repository, inventories and definition in the committed-case cap.
    source_bytes = sum(item["size"] for tree in expected.values() for item in tree["entries"].values() if item["kind"] == "file")
    repo_bytes = sum(item["size"] for item in repository_inventory(repo).values() if item["kind"] == "file")
    case_root = destination.parents[1]
    existing_bytes = sum(path.stat().st_size for path in case_root.rglob("*")
                         if path.is_file() and path not in {expected_path, case_root / "case.json"})
    total = existing_bytes + repo_bytes + (0 if (case_root / "source").exists() else source_bytes)
    total += len(json.dumps(expected, indent=2).encode()) + len(json.dumps(definition, indent=2).encode()) + 1024
    if limit and total > limit:
        raise ValueError(f"case exceeds {limit} bytes: {total}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(repo, destination, ignore=shutil.ignore_patterns(".gutch-lock", ".gutch-transactions"))
    if repository_inventory(repo) != repository_inventory(destination):
        raise AssertionError("frozen repository copy differs")
    write_json(expected_path, expected)
    write_json(destination.parents[1] / "case.json", definition)
    if settings["kind"] == "generated":
        committed_source = destination.parents[1] / "source"
        if not committed_source.exists():
            shutil.copytree(source, committed_source)
    print(f"Frozen {name}/{version}: repository {repo_bytes / 1048576:.2f} MiB", flush=True)
    from tools.artifacts import remove_owned
    remove_owned(work, work.parent)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("name")
    parser.add_argument("--repository-version", default="v1")
    parser.add_argument("--gutch-exe", type=Path, required=True)
    parser.add_argument("--source", type=Path)
    args = parser.parse_args()
    freeze(args.name, args.repository_version, args.gutch_exe, args.source)


if __name__ == "__main__":
    main()
