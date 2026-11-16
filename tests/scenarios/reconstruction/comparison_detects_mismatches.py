"""Operations: compare equal files, change one file and compare again with a JSON report.

Expected: equality succeeds; differing bytes produce a mismatch report and a nonzero exit code.
"""


def run(case):
    expected = case.payload / "expected.txt"
    actual = case.payload / "actual.txt"
    expected.write_bytes(b"original")
    actual.write_bytes(b"original")
    assert case.stats("debug", "compare-trees", "--expected", expected, "--actual", actual)[
        "compare_passed"
    ]
    actual.write_bytes(b"modified")
    result = case.stats(
        "debug", "compare-trees", "--expected", expected, "--actual", actual, success=False
    )
    assert not result["compare_passed"]
    assert result["mismatched_file_count"] == 1
