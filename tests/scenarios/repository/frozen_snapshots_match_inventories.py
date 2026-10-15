"""Operations: Run the shared append/delete/rebuild/repair lifecycle on the frozen repository.

Expected: Paths, content hashes and supported attributes match the recorded source inventories.
"""

import shutil
from tests.datasets.generate import case_names, load_case
from tests.helpers.cli import Case
from tests.helpers.corpus import repository_input
from tests.repository_suite import run_lifecycle


def run(case):
    for name in case_names():
        definition = load_case(name)
        if definition["source"]["kind"] == "blender":
            continue  # Explicit selection: large real-world corpus has a separate runtime budget.
        assert definition["repositories"], f"missing frozen repository: {name}"
        for version in definition["repositories"]:
            child = Case(case.root / "corpus" / name / version, case.executable)
            shutil.copy2(case.root / "replay.json", child.root / "replay.json")
            source, expected = repository_input(name, version)
            run_lifecycle(child, source=source, expected=expected, steps=0)
            case.checks.append(f"{name}/{version}")
