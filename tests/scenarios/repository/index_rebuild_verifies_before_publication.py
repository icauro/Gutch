"""Operations: Regenerate missing/corrupt maps from bundles and loose chunks.

Expected: Complete recovery reconstructs exact content; corruption or missing artifacts cannot publish.
"""

import json
from tests.datasets.generate import basic


def run(case):
    for storage in ("bundled", "loose_chunks"):
        repo = case.init(storage)
        data = basic(case.payload / (storage + "-source"))
        captured = case.capture(
            repo, data.root, "--storage-mode", storage.replace("_", "-")
        )
        index = repo / "chunk_map.gtcbin"
        index.unlink()
        report = case.root / (storage + "-recovery.json")
        case.command(
            "repo", "index", "rebuild", "--repo", repo, "--json", "--output", report
        )
        assert json.loads(report.read_text())["complete"]
        assert not index.exists()
        for damaged in (None, b"malformed index"):
            if damaged is not None:
                index.write_bytes(damaged)
            case.command(
                "repo",
                "index",
                "rebuild",
                "--repo",
                repo,
                "--apply",
                "--json",
                "--output",
                report,
            )
            assert json.loads(report.read_text())["published"]
            destination = case.payload / (
                storage + ("-missing" if damaged is None else "-corrupt")
            )
            case.reconstruct(repo, captured["snapshot_id"], destination)
            case.compare(data, destination, destination.name)
        good = index.read_bytes()
        directory = repo / ("bundles" if storage == "bundled" else "chunks")
        artifact = next(path for path in directory.rglob("*") if path.is_file())
        intact = artifact.read_bytes()
        artifact.write_bytes(b"corrupt artifact")
        case.command(
            "repo",
            "index",
            "rebuild",
            "--repo",
            repo,
            "--apply",
            "--json",
            "--output",
            report,
            success=False,
        )
        assert json.loads(report.read_text())["corruptArtifacts"]
        assert index.read_bytes() == good
        artifact.write_bytes(intact)
        artifact.unlink()
        case.command(
            "repo",
            "index",
            "rebuild",
            "--repo",
            repo,
            "--apply",
            "--json",
            "--output",
            report,
            success=False,
        )
        result = json.loads(report.read_text())
        assert not result["complete"] and not result["published"]
        assert result["missingChunks"] and result["affectedSnapshots"]
        assert index.read_bytes() == good
