"""Portable source identity for failure replay, including uncommitted changes."""
import hashlib


def source_revision():
    from tools.build_image import source_files
    digest = hashlib.sha256()
    for path, relative in sorted(source_files(), key=lambda item: item[1].as_posix()):
        digest.update(relative.as_posix().encode())
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return "sha256:" + digest.hexdigest()
