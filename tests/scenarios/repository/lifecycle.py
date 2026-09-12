"""Operations: Run the shared lifecycle against generated or selected repository input.

Expected: Every mutation preserves the model, and rejected mutations preserve repository bytes.
"""
import json
from pathlib import Path
from tests.repository_suite import run_lifecycle


def run(case):
    path = case.root / "options.json"
    options = json.loads(path.read_text(encoding="utf-8")) if path.is_file() else {}
    source = Path(options["repo"]) if options.get("repo") else None
    expected = json.loads(Path(options["expected"]).read_text(encoding="utf-8")) if options.get("expected") else None
    if options.get("dataset"):
        from tests.helpers.corpus import repository_input
        source, expected = repository_input(options["dataset"], options["repository_version"])
    run_lifecycle(case, source=source, expected=expected,
                  seed=options.get("seed", 123), steps=options.get("steps", 4),
                  storage=options.get("storage", "bundled"), compression=options.get("compression", "zstd"),
                  mutations=options.get("mutations", 0),
                  sequence=options["sequence"].split(",") if options.get("sequence") else ([] if "sequence" in options else None))
