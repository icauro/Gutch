"""Portable source identity and lifecycle arguments for failure replay."""

import hashlib


def source_revision():
    from _tools.build_image import source_files

    digest = hashlib.sha256()
    for path, relative in sorted(source_files(), key=lambda item: item[1].as_posix()):
        digest.update(relative.as_posix().encode())
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return "sha256:" + digest.hexdigest()


LIFECYCLE_OPTIONS = ("seed", "steps", "storage", "compression", "mutations", "timeout", "sequence")


def lifecycle_arguments(options, *, exclude=()):
    """Return `gutch.py test` arguments that recreate a recorded lifecycle run.

    A corpus dataset prepares its own repository and expectations again, so its
    recorded prepared paths are not forwarded alongside it.
    """
    names = [*LIFECYCLE_OPTIONS]
    names += ["dataset", "repository_version"] if "dataset" in options else ["repo", "expected"]
    arguments = []
    for name in names:
        if name in options and name not in exclude:
            arguments += ["--" + name.replace("_", "-"), str(options[name])]
    return arguments
