"""Operations: Run every library example in examples/ into a fresh work folder, then rerun one
into the same folder.

Expected: Each example succeeds and prints its key outcome; a rerun into an existing folder exits
with code 2 and a message instead of touching it.
"""

import os

from tests.helpers.cli import REPOSITORY, execute

EXPECTED = {
    "capture_and_install": "0 missing, 0 corrupt, 0 extra",
    "update_installation": "chunks reused from 1.0",
    "repair_installation": "after repair: 0 missing, 0 corrupt, 0 extra",
    "cancel_reconstruction": "1.0 after the cancelled update: 0 missing, 0 corrupt, 0 extra",
    "folder_remote": "0 missing, 0 corrupt",
}


def run(case):
    examples = case.executable.parent / "examples"
    suffix = ".exe" if os.name == "nt" else ""
    source = case.payload / "source"
    (source / "docs").mkdir(parents=True)
    (source / "docs/readme.txt").write_text("hello\n" * 100, encoding="utf-8")
    (source / "app.bin").write_bytes(bytes(range(256)) * 512)
    assert sorted(EXPECTED) == sorted(
        path.stem for path in (REPOSITORY / "examples").glob("*.rs")
    ), "every example needs an expected outcome here"
    for name, expected in EXPECTED.items():
        work = case.payload / name
        arguments = [source, work] if name == "capture_and_install" else [work]
        log = case.log(name)
        execute([examples / f"{name}{suffix}", *arguments], REPOSITORY, {}, log)
        output = log.read_text(encoding="utf-8")
        assert expected in output, f"{name} did not report {expected!r}; see {log}"
    log = case.log("rerun")
    code = execute(
        [examples / f"update_installation{suffix}", case.payload / "update_installation"],
        REPOSITORY,
        {},
        log,
        success=False,
    )
    assert code == 2
    assert "already exists" in log.read_text(encoding="utf-8")
