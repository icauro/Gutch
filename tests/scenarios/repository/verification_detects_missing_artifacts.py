"""Operations: capture a repository, verify it, remove an artifact and verify again.

Expected: intact verification succeeds; missing artifacts produce a failed report and exit status.
"""

import json
from tests.datasets.generate import basic


def run(case):
    repo = case.init()
    case.capture(repo, basic(case.payload / "source").root)
    report = case.root / "verification.json"
    case.command(
        "repo", "verify", "--repo", repo, "--strict", "--json", "--output", report
    )
    assert json.loads(report.read_text(encoding="utf-8"))["ok"]
    next(path for path in (repo / "bundles").rglob("*") if path.is_file()).unlink()
    case.command(
        "repo",
        "verify",
        "--repo",
        repo,
        "--strict",
        "--json",
        "--output",
        report,
        success=False,
    )
    assert not json.loads(report.read_text(encoding="utf-8"))["ok"]
