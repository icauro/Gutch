"""Operations: Capture the same tree twice, delete the source, reconstruct it and verify the result.

Expected: Identity stays stable, no new bundles are generated, and every entry matches.
"""

from tests.helpers.roundtrip import capture_roundtrip


def run(case):
    capture_roundtrip(case)
