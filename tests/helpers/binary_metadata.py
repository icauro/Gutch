"""Independent raw wire fixtures for testing validly framed hostile metadata."""

import struct
from blake3 import blake3


def envelope(magic, groups):
    directory = bytearray(magic + struct.pack("<HHI", 1, 0, len(groups)))
    for index, group in enumerate(groups, 1):
        directory += struct.pack("<HBBII", index, 1, 0, len(group), len(group))
        directory += blake3(group).digest()
    return bytes(directory) + blake3(directory).digest() + b"".join(groups)


def directory_blueprint(path):
    components = path.split("/")
    leaves = [part.encode("utf-8") for part in components]
    count = len(leaves)
    paths = bytearray(struct.pack("<II", count - 1, 1))
    paths += struct.pack("<" + "I" * count, *range(count))
    offsets = [0]
    for leaf in leaves:
        offsets.append(offsets[-1] + len(leaf))
    paths += struct.pack("<" + "I" * (count + 1), *offsets)
    paths += b"".join(leaves)
    return envelope(b"GUTCH_BLUEPRINT\0", [bytes(paths), b"\x01\0\0", bytes(4)])
