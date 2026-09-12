"""One repository lifecycle, exercised against generated, frozen or selected inputs."""
import copy
import json
import os
from pathlib import Path
import random
import shutil

from tests.datasets.generate import generated
from tests.helpers.filesystem import assert_tree, inventory, repository_inventory, write_json


def copy_repository(source, destination):
    source = source.resolve()
    if not (source / "repo_metadata.json").is_file():
        raise ValueError(f"Not a repository: {source}")
    if destination.resolve().is_relative_to(source):
        raise ValueError("Repository input must not contain the test output directory")
    def reject(error):
        raise error
    for directory, directories, files in os.walk(source, onerror=reject):
        for name in directories + files:
            path = Path(directory) / name
            if path.is_symlink() or path.is_junction():
                raise ValueError(f"Repository input contains a filesystem link: {path}")
    before = repository_inventory(source)
    shutil.copytree(source, destination)
    if repository_inventory(source) != before or repository_inventory(destination) != before:
        raise ValueError("Repository changed while being copied; select an idle repository")


def run_lifecycle(case, *, source=None, expected=None, seed=123, steps=4,
                  storage="bundled", compression="zstd", mutations=0):
    operations = []
    def record(operation, **details):
        operations.append({"operation": operation, **details})
        write_json(case.root / "operations.json", operations)

    expected = copy.deepcopy(expected) if expected is not None else None
    if expected is not None and os.name == "nt":
        for snapshot in expected.values():
            for entry in snapshot["entries"].values():
                if entry["kind"] == "file":
                    entry["executable"] = False
    repo = case.payload / "repo"
    if source is not None:
        retained = case.root / "inputs/repository"
        retained.parent.mkdir(parents=True, exist_ok=True)
        copy_repository(source, retained)
        shutil.copytree(retained, repo)
        replay_path = case.root / "replay.json"
        if replay_path.is_file():
            replay = json.loads(replay_path.read_text(encoding="utf-8"))
            replay.setdefault("options", {}).update(repo=str(retained), seed=seed, steps=steps,
                storage=storage, compression=compression, mutations=mutations)
            if expected is not None:
                expected_path = case.root / "inputs/expected.json"
                write_json(expected_path, expected)
                replay["options"]["expected"] = str(expected_path)
            write_json(replay_path, replay)
    else:
        repo = case.init()
        expected = {}
    trusted = expected is not None
    case.details = {"oracle": "independent-inventory" if trusted else "initial-reconstruction-consistency",
                    "seed": seed, "steps": steps, "storage": storage, "compression": compression,
                    "parser_mutations": mutations}
    model = expected if trusted else {}
    record("open", oracle=case.details["oracle"], seed=seed)

    def listed():
        path = case.root / "listed.json"
        case.command("snapshot", "list", "--repo", repo, "--json-out", path)
        return {item["snapshotId"] for item in json.loads(path.read_text(encoding="utf-8"))}

    def check_all(label):
        record("check", phase=label, snapshots=sorted(model))
        assert listed() == set(model), "snapshot list differs from the expected repository state"
        report = case.root / "verify.json"
        case.command("repo", "verify", "--repo", repo, "--strict", "--json-out", report)
        assert json.loads(report.read_text(encoding="utf-8"))["ok"]
        for snapshot, specification in model.items():
            destination = case.payload / "restored" / snapshot
            case.reconstruct(repo, snapshot, destination)
            assert_tree(specification["entries"], destination, case.root / "comparisons" / f"{label}-{snapshot}.json")
            assert case.verify(repo, snapshot, destination)["verification_passed"]
        case.checks.append(label)

    original_ids = listed()
    if trusted:
        assert original_ids == set(model), "expected inventories must cover exactly the input snapshots"
    else:
        for snapshot in sorted(original_ids):
            destination = case.payload / "baseline" / snapshot
            case.reconstruct(repo, snapshot, destination)
            model[snapshot] = {"entries": inventory(destination)}
        write_json(case.root / "observed-baseline.json", model)
    check_all("initial")

    version = 0
    added = []
    def capture():
        nonlocal version
        for _ in range(len(model) + 1):
            version += 1
            data = generated(case.payload / f"source-{version}", seed, version)
            record("capture", version=version, storage=storage, compression=compression)
            result = case.capture(repo, data.root, "--storage-mode", storage, "--compression-algorithm", compression)
            snapshot = result["snapshot_id"]
            if snapshot not in model:
                break
            assert model[snapshot]["entries"] == data.expected, "recapture identity aliases different content"
        else:
            raise AssertionError("distinct generated versions did not produce a new snapshot")
        if added:
            assert result["reused_chunk_count"] > 0, "generated versions did not share chunks"
        model[snapshot] = {"entries": data.expected}
        added.append(snapshot)
        write_json(case.root / "expected.json", model)
        return snapshot

    first = capture()
    second = capture()
    check_all("append")

    def rejected_mutation():
        record("reject-delete-with-broken-index")
        path = repo / "chunk_map.gtcbin"
        intact = path.read_bytes()
        path.write_bytes(b"invalid chunk map")
        before = repository_inventory(repo)
        case.command("snapshot", "delete", "--repo", repo, "--snapshot-id", second, "--yes", success=False)
        after = repository_inventory(repo)
        differences = {name: {"before": before.get(name), "after": after.get(name)}
                       for name in before.keys() | after.keys() if before.get(name) != after.get(name)}
        write_json(case.root / "rejected-mutation.json", differences)
        assert not differences, f"rejected deletion changed repository data: {differences}"
        path.write_bytes(intact)

    def rebuild():
        record("rebuild-index")
        (repo / "chunk_map.gtcbin").unlink()
        report = case.root / "rebuild.json"
        case.command("repo", "regenerate-chunk-map", "--repo", repo, "--publish", "--json-out", report)
        assert json.loads(report.read_text(encoding="utf-8"))["published"]

    def repair():
        snapshot = second
        record("repair", snapshot=snapshot)
        destination = case.payload / "repair"
        case.reconstruct(repo, snapshot, destination)
        assert_tree(model[snapshot]["entries"], destination, case.root / "repair-before.json")
        filename = next(name for name, entry in model[snapshot]["entries"].items() if entry["kind"] == "file")
        (destination / filename).write_bytes(b"damaged installation")
        assert not case.verify(repo, snapshot, destination, success=False)["verification_passed"]
        case.command("reconstructed", "plan", "--repo", repo, "--snapshot-id", snapshot, "--destination", destination)
        case.repair(repo, snapshot, destination)
        assert_tree(model[snapshot]["entries"], destination, case.root / "repair-after.json")

    rejected_mutation()
    check_all("rejected-mutation")
    record("delete", snapshot=first)
    case.command("snapshot", "delete", "--repo", repo, "--snapshot-id", first, "--yes")
    del model[first]
    check_all("delete")
    rebuild()
    check_all("rebuild")
    repair()
    generator = random.Random(seed)
    def delete_added():
        candidates = [snapshot for snapshot in added if snapshot in model and snapshot != second]
        if not candidates:
            capture()
            candidates = [snapshot for snapshot in added if snapshot in model and snapshot != second]
        snapshot = generator.choice(candidates)
        record("delete", snapshot=snapshot)
        case.command("snapshot", "delete", "--repo", repo, "--snapshot-id", snapshot, "--yes")
        del model[snapshot]
    for step in range(steps):
        action = generator.choice((capture, rebuild, repair, rejected_mutation, delete_added))
        action()
        check_all(f"sequence-{step}")
    if mutations:
        exercise_malformed_inputs(case, repo, seed, mutations, record)
        check_all("after-parser-mutations")
    write_json(case.root / "lifecycle.json", {"seed": seed, "steps": steps,
               "oracle": "independent-inventory" if trusted else "initial-reconstruction-consistency",
               "operations": operations, "snapshots": sorted(model)})


def exercise_malformed_inputs(case, repo, seed, count, record):
    """Seeded parser mutation testing, not coverage-guided fuzzing."""
    candidates = [repo / "chunk_map.gtcbin", *sorted(repo.glob("snapshots/*/snapshot_blueprint.gtcbin")),
                  *sorted(repo.glob("bundles/*/*")), *sorted(repo.glob("chunks/*/*"))]
    generator = random.Random(seed)
    for index in range(count):
        path = generator.choice(candidates)
        original = path.read_bytes()
        mutated = bytearray(original)
        if generator.randrange(2) and mutated:
            mutated = mutated[:generator.randrange(len(mutated))]
        elif mutated:
            mutated[generator.randrange(len(mutated))] ^= 1 << generator.randrange(8)
        record("parser-mutation", index=index, path=path.relative_to(repo).as_posix())
        path.write_bytes(mutated)
        # Retain the exact bytes before invoking a potentially failing decoder.
        (case.root / "mutated-input.bin").write_bytes(mutated)
        code = case.command("repo", "verify", "--repo", repo, "--strict", success=None)
        assert code in (0, 1), f"decoder crashed or aborted with exit code {code}"
        path.write_bytes(original)
    (case.root / "mutated-input.bin").unlink(missing_ok=True)
