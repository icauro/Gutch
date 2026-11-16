"""Operations: Capture and reconstruct in every profiling mode.

Expected: Content matches; off retains feedback without diagnostic timings.
"""

import json
from tests.datasets.generate import Dataset


def run(case):
    data = Dataset(case.payload / "source")
    data.file("data.bin", bytes(range(256)) * 8192)
    for mode in ("off", "summary", "detailed"):
        repo = case.init("repo-" + mode)
        directory = case.root / ("profile-" + mode)
        options = ["--profile", mode]
        if mode != "off":
            options += ["--profile-dir", directory]
        capture = case.capture(repo, data.root, *options)
        assert capture["profiling_mode"] == mode
        destination = case.payload / ("out-" + mode)
        if mode != "off":
            options = [
                "--profile",
                mode,
                "--profile-dir",
                case.root / ("restore-" + mode),
            ]
        stats = case.reconstruct(repo, capture["snapshot_id"], destination, *options)
        case.compare(data, destination, mode)
        # Summary mode measures phase walls; off mode measures nothing.
        assert (stats["verify_timings"]["total_seconds"] is None) == (mode == "off")
        if mode != "off":
            report = json.loads((directory / "summary.json").read_text(encoding="utf-8"))
            assert report["operation_succeeded"]
            assert report["profile"]["mode"] == mode
        assert (directory / "trace.json").exists() == (mode == "detailed")
    logs = "\n".join(
        p.read_text(encoding="utf-8", errors="replace")
        for p in (case.root / "commands").glob("*.log")
    )
    assert "Output write:" in logs
    case.command("repo", "list", "--profile", "detailed", success=False)
    case.command(
        "repo",
        "list",
        "--profile",
        "off",
        "--profile-dir",
        case.root / "invalid-profile",
        success=False,
    )
