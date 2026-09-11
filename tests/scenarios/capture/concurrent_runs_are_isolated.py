"""Operations: run two capture/reconstruct workflows concurrently in separate case roots.

Expected: both round trips succeed and repositories and configuration remain isolated.
"""
from concurrent.futures import ThreadPoolExecutor
from tests.support.cli import Case
from tests.support.roundtrip import capture_roundtrip


def run(case):
    children = [Case(case.payload / name, case.executable) for name in ("first", "second")]
    with ThreadPoolExecutor(max_workers=2) as pool:
        list(pool.map(capture_roundtrip, children))
    assert children[0].environment != children[1].environment
    assert all((child.payload / "repo").is_dir() for child in children)
