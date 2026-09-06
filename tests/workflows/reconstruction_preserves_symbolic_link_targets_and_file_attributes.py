"""Operations: Capture and reconstruct a tree containing a symbolic link.

Expected: Links keep their intended targets and all entries match; unsupported hosts explicitly
skip.
"""
from ..support.roundtrip import metadata_roundtrip


def run(case):
    metadata_roundtrip(case, links=True)
