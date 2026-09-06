"""Operations: Capture and reconstruct a tree with file attributes, directories and long paths.

Expected: The independent inventory confirms content and supported filesystem attributes.
"""
from ..support.roundtrip import metadata_roundtrip


def run(case):
    metadata_roundtrip(case, links=False)
