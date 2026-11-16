"""Released formats stay frozen: schemas/v*/ and frozen corpus repositories.

Once a release tag exists, nothing inside a folder that the tag already holds
may change, be added or be removed. A new format adds a new folder instead.
Before the first release, or outside a Git checkout, there is nothing to check.
"""

import re
import subprocess

import pytest

from _tools import runner

RELEASE_TAG = re.compile(r"v\d+\.\d+\.\d+")
FROZEN_FOLDER = re.compile(
    r"schemas/v\d+/|tests/datasets/corpus/[^/]+/repository/format-v\d+/"
)
FROZEN_PATHS = [
    ":(glob)schemas/v*/**",
    ":(glob)tests/datasets/corpus/*/repository/format-v*/**",
]


def git(*arguments):
    return subprocess.run(
        ["git", "-c", "core.quotepath=false", *arguments],
        cwd=runner.ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )


def latest_release_tag():
    tags = git("tag", "--list", "v*", "--sort=-v:refname")
    if tags.returncode != 0:
        return None
    return next((tag for tag in tags.stdout.split() if RELEASE_TAG.fullmatch(tag)), None)


def folders(paths):
    return {match.group() for path in paths if (match := FROZEN_FOLDER.match(path))}


def test_released_schemas_and_frozen_repositories_are_unchanged():
    tag = latest_release_tag()
    if tag is None:
        pytest.skip("no release tag yet; released formats are not frozen")
    # ls-tree ignores glob pathspecs, so filter the released tree here.
    released = folders(git("ls-tree", "-r", "--name-only", tag).stdout.splitlines())
    touched = git("diff", "--name-only", tag, "--", *FROZEN_PATHS).stdout.splitlines()
    touched += git(
        "ls-files", "--others", "--exclude-standard", "--", *FROZEN_PATHS
    ).stdout.splitlines()
    changed = [path for path in touched if folders([path]) & released]
    assert not changed, (
        f"released formats changed since {tag}; add a new format folder instead: "
        f"{changed[:10]}"
    )
