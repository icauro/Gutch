# Changelog

Gutch is not yet at 1.0, so any release can break the library API, the
command line or the repository format.

## 0.2.1 (2026-11-22)

### New

- `fetch_snapshot_metadata` and `snapshot fetch --metadata-only` download a
  snapshot's metadata, blueprint and the repository's chunk index from URLs,
  without bundles. An installed copy of the snapshot can then be verified, and
  an update from it reuses its files, without downloading its content first.
  Before, only `repo pull` could fetch metadata alone, which needs a remote
  that lists folders.

## 0.2.0 (2026-11-18)

The first release on crates.io, as the crates `gutch` and `gutch-cli`.

### Breaking changes

- `SnapshotCaptureRequest` has a new field, `reuse_remote_bundles`. Set it to
  `false` to keep the old behavior.
- `CommandLineHost` has a new field, `licenses`. Set it to `None` to leave the
  `licenses` command out.
- Releases ship bare executables instead of archives:
  `gutch-cli-0.2.0-PLATFORM` (`.exe` on Windows) for `linux-x86_64`,
  `linux-arm64`, `windows-x86_64` and `windows-arm64`, plus one `SHA256SUMS`.

### New

- A copy of a repository pulled without bundles (metadata only) can now
  capture new snapshots and push them. With `capture --reuse-remote-bundles`,
  chunks are treated as stored when the chunk index lists their bundle, even
  though the bundle is not in this copy. Push then accepts a missing local
  bundle when the remote already holds it. A publisher can add a version
  without downloading the whole repository.
- Such a copy can also delete snapshots and push the removal. Before, delete
  refused to run when a remaining snapshot needed a bundle the copy did not
  have.
- `gutch-cli licenses` prints the license notices of Gutch and every
  dependency built into the executable. Builds from crates.io print Gutch's own
  license and a note instead.

### Examples

- The Gutch Launcher Sample (`examples/launcher_sample`): a publisher, run
  daily by a workflow, adds official Blender builds to a public bucket as a
  Gutch repository. A desktop launcher installs versions side by side and
  downloads only what changed. It shows network and disk speeds, and asks
  before a repair removes extra files.
- The S3 code moved into the `gutch-s3-store` crate, shared by the S3 app
  and the launcher sample.
- The example apps open without a console window on Windows.

### Fixed

- A Unix-only test did not build after the new capture field was added.

### Documentation

- The README was rewritten around one install, with a logo, a pipeline
  diagram, a profiled install and a table of where Gutch fits.
- The user guide compares bundled and loose-chunk storage, and says what
  encryption leaves visible.

## 0.1.0 (2026-11-17)

The first public release, on GitHub only.
