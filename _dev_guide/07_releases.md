# Gutch Releases

Status: Current

Scope: Publishing a version to crates.io and GitHub Releases

A release publishes both crates, `gutch` and `gutch-cli`, to crates.io and
attaches the four `gutch-cli` executables to a GitHub release. Both use the
same version, and pushing a tag is the only step that starts a release.

## Making a release

1. Set the new version in the root `Cargo.toml`, in both
   `[workspace.package] version` and `[workspace.dependencies] gutch`. Both
   crates inherit it from there.
2. Run any Cargo command, such as `cargo check`, so `Cargo.lock` picks up the
   new version, and commit both files to `main`.
3. Tag that commit and push the tag:

   ```sh
   git tag v0.2.0
   git push origin v0.2.0
   ```

A version with a hyphen, such as `v0.2.0-rc.1`, becomes a GitHub prerelease.

## What the release workflow does

`.github/workflows/release.yml` runs these jobs in order:

| Job | Does |
| --- | --- |
| Check version | Fails unless the tag is on `main` and matches the version in `Cargo.toml` and `Cargo.lock` |
| Tests, Build | Runs the full correctness workflow and the [CLI build](06_cli_builds.md), which also checks that `gutch-cli --version` reports the tag's version |
| Draft GitHub release | Names each platform's executable `gutch-cli-VERSION-PLATFORM` (plus `.exe` on Windows), writes `SHA256SUMS`, and creates a draft release with the five files and notes generated from the commits. The executables embed their license notices, so nothing is archived |
| Publish to crates.io | Publishes `gutch`, then `gutch-cli` |
| Publish GitHub release | Makes the draft public |

A crates.io version can be yanked but never replaced, so nothing is published
until every check has passed, and the GitHub release stays a draft until
crates.io has accepted both crates. If a job fails, fix the cause and re-run
the failed jobs: the draft is updated in place, and crates that are already
on crates.io at that version are skipped. If the fix needs a new commit,
delete the draft and the tag, then tag the fixed commit.

## crates.io access

The workflow signs in with crates.io trusted publishing: crates.io trusts this
repository's `release.yml` in the `crates-io` environment and hands it a
short-lived token, so no crates.io token is stored on GitHub. Trusted
publishing can only be enabled on a crate that already exists, so the first
version is published by hand:

```sh
cargo login
cargo publish --workspace
```

Then, for each crate on crates.io, open **Settings → Trusted Publishing** and
add a GitHub publisher with owner `icauro`, repository `Gutch`, workflow
`release.yml` and environment `crates-io`. In the repository's GitHub
settings, the `crates-io` environment can additionally require an approval
before the crates.io job runs.
