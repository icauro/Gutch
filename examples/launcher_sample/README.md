# Gutch Launcher Sample

A working example of an app that ships large, frequent releases with Gutch. It
publishes the official Blender builds, then installs several versions side by
side. Each new version downloads only the content that changed since a version
you already have.

> Not affiliated with or endorsed by the Blender Foundation. Blender is a
> trademark of the Blender Foundation. See [BLENDER_NOTICE.md](BLENDER_NOTICE.md).

It has three parts, each in its own crate:

| Crate | What it does |
| --- | --- |
| [`publisher`](publisher) | Adds new Blender releases to a public bucket as a Gutch repository, retires old ones and writes `catalog.json` |
| [`launcher`](launcher) | A desktop app that installs, launches, verifies, repairs, moves and uninstalls versions |
| [`catalog`](catalog) | The `catalog.json` format and the bucket's public URLs, shared by both |

The S3 code comes from [`../s3_store`](../s3_store), which the
[S3 app](../s3_app) uses too.

## Run the launcher

```sh
cd examples/launcher_sample
cargo run --release -p gutch-launcher-sample
```

The launcher lists the builds published for your system (Windows x64 or Linux
x64). With **Install**, a version goes to `DATA/versions/blender-VERSION`.
With **Install to…**, in the menu beside **Install**, you pick the parent
folder. An install first checks that the disk has room for the version, and
it shows its progress in the version's row: the amount downloaded and
written, the time left, and a graph of the network and disk speeds. Gutch
downloads and writes at the same time, so both can be busy at once.
Installed versions have **Launch**; the rest is under **More…**. A newer
patch of an installed series is marked as an update. The launcher keeps its
files in each system's folders for the current user:

| | `DATA`: cache and versions | The list of installs (`state.json`) |
| --- | --- | --- |
| Windows | `%LOCALAPPDATA%\Gutch Launcher Sample` | In `DATA` |
| Linux | `$XDG_DATA_HOME/gutch-launcher-sample`, or `~/.local/share/gutch-launcher-sample` | `$XDG_STATE_HOME/gutch-launcher-sample`, or `~/.local/state/gutch-launcher-sample` |

Set `LAUNCHER_BUCKET_URL` to read builds from another bucket that the
publisher fills.

### How the launcher uses Gutch

- **Install** calls `fetch_and_reconstruct_installation`. It downloads the
  snapshot's metadata and the bundles it needs into a local repository, the
  cache (`DATA/cache`), then builds the folder. When another version for the
  same system is installed, the newest one is passed as the reuse source.
  Files it shares with the new version are copied from disk and are not
  downloaded.
- Every download is a plain HTTP GET of a public URL from
  [`PublicLayout`](catalog/src/lib.rs). The launcher holds no keys.
- The speeds come from the `feedback` field of Gutch's progress events,
  which reports the download and the writes to the folder separately.
- **Verify** calls `verify_installation` against the cached snapshot.
- **Repair** downloads the bundles the snapshot needs, while the version is
  still published. It then calls `repair_installation`, which restores
  missing and changed files and keeps files you added.
- **Move…** moves the folder, and **Uninstall** deletes it. Uninstall then
  deletes the snapshot from the cache, which keeps any bundles that other
  installed versions still need.

The cache keeps the bundles of every installed version, even after the
publisher retires that version. An installed version that is no longer
published can still be verified, and repaired from the cache.

## The publisher

[`.github/workflows/launcher-publish.yml`](../../.github/workflows/launcher-publish.yml)
runs the publisher every day, and on demand from the Actions tab. Each run:

1. Reads the list of stable releases. It keeps the newest patch release of
   the 8 newest series, from 5.0 on, for Windows x64 and Linux x64.
   `download.blender.org` refuses GitHub's runners, so the workflow reads an
   official mirror of it (`--releases-url`). A build is used only when two
   mirrors run by other organizations publish the same SHA-256 for it
   (`--confirm-url`). A build they have not copied yet waits for a later
   run.
2. Pulls the repository's metadata from the bucket, without bundles, and
   compares the published versions with the wanted ones.
3. Deletes retired versions' snapshots and pushes. The push deletes the
   bundles no remaining version needs.
4. Downloads each new build and checks it against the release's published
   SHA-256. It then extracts the build, captures it as a snapshot tagged
   `version=` and `os=`, and pushes. Bundles already in the bucket are reused
   (`reuse_remote_bundles`), so a new version uploads only what changed.
5. Writes `catalog.json`. The bucket does not allow listing, so the launcher
   finds the versions here.

A run with an empty release list changes nothing, so an outage never
retires the published versions. One run adds at most 4 builds (`--max-new`). It stops adding when the bucket
would grow past 18 GiB (`--budget-gib`). Builds left over wait for a later
run. `--dry-run` reports what would change and changes nothing.

### Run it yourself

The bucket must allow public-read objects and conditional writes. Set the
connection, check it, then try a dry run:

```sh
export S3_ENDPOINT=https://s3.REGION.example.com S3_REGION=REGION S3_BUCKET=BUCKET
export AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=...
cargo run --release -p launcher-publisher -- check
cargo run --release -p launcher-publisher -- publish --dry-run
```

`S3_PREFIX` puts everything under a folder in the bucket. `publish --help`
lists the other options.

### Set up the workflow

1. Create a GitHub environment named `launcher-publish`.
2. Add the bucket keys to it as the secrets `OVH_S3_ACCESS_KEY_ID` and
   `OVH_S3_SECRET_ACCESS_KEY`.
3. Change `S3_ENDPOINT`, `S3_REGION` and `S3_BUCKET` in the workflow if you
   use another bucket.
4. Run the workflow from the Actions tab with **dry-run** first.

GitHub turns off scheduled workflows in a public repository after 60 days
with no activity. Turn the workflow back on from the Actions tab.

## Tests

```sh
cargo test
```

The tests use an in-memory bucket and need no network or keys. The launcher
tests serve that bucket over local HTTP.

## License

The sample's code is under the [MIT license](LICENSE). The Blender builds it
downloads are not part of this repository. They keep their own licenses, as
[BLENDER_NOTICE.md](BLENDER_NOTICE.md) describes.
