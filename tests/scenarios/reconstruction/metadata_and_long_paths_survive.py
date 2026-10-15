"""Operations: Capture and reconstruct a tree with file attributes, directories and long paths.
Then recapture with only permissions or the link target changed and reconstruct again.

Expected: The independent inventory confirms content and supported filesystem attributes.
"""

from tests.helpers.roundtrip import metadata_roundtrip


def run(case):
    metadata_roundtrip(case, links=False)
