"""Operations: Capture and reconstruct a tree containing a symbolic link.
Then recapture with only permissions or the link target changed and reconstruct again.

Expected: Links keep their intended targets and all entries match; unsupported hosts explicitly
skip.
"""
from tests.support.roundtrip import metadata_roundtrip


def run(case):
    metadata_roundtrip(case, links=True)
