# Gutch S3 app

A desktop app that keeps Gutch repositories in an S3-compatible bucket (AWS S3,
MinIO, Cloudflare R2 and others). With it you can:

- create a repository in the bucket, or open one that is already there
- capture a local folder as a tagged snapshot and push it
- install any snapshot to a local folder, or update an installed one in place
- verify an installation and repair missing or changed files
- list a snapshot's files and save a copy of one without installing anything

Each operation shows its progress and can be cancelled.

## Run

```sh
cd examples/s3_app
cargo run --release
```

The app has its own Cargo workspace, so its GUI dependencies do not slow down
Gutch's own builds and tests. Run its tests with `cargo test` in this folder.

## Connect

| Field | Example |
| --- | --- |
| Endpoint | `https://s3.eu-central-1.amazonaws.com`, or `http://localhost:9000` for MinIO |
| Region | `eu-central-1` (MinIO accepts `us-east-1`) |
| Bucket | An existing bucket; the app does not create buckets |
| Key prefix | A folder in the bucket for Gutch's objects, such as `gutch`; empty for the bucket root |
| Path-style URLs | On for MinIO and other servers without bucket subdomains |
| Access key ID, secret key | Credentials allowed to read, write, list and delete objects below the prefix |

Hover over a field or button to see what it does.

On the first run the app fills in the endpoint and region from
`AWS_ENDPOINT_URL` and `AWS_REGION`. When `AWS_ACCESS_KEY_ID` and
`AWS_SECRET_ACCESS_KEY` are set, it always uses them.

## Settings and cache

The app remembers the connection fields, the cache folder, the tags and the
last folders you picked. It never saves the secret key: set
`AWS_SECRET_ACCESS_KEY`, or type the key each time you start the app.

| | Settings (`app.ron`) | Default cache folder |
| --- | --- | --- |
| Windows | `%APPDATA%\Gutch S3\data` | `%LOCALAPPDATA%\Gutch S3` |
| macOS | `~/Library/Application Support/Gutch-S3` | `~/Library/Caches/Gutch S3` |
| Linux | `$XDG_DATA_HOME/gutchs3` or `~/.local/share/gutchs3` | `$XDG_CACHE_HOME/Gutch S3` or `~/.cache/Gutch S3` |

You can delete the cache folder at any time; the app downloads again what it
needs.

The bucket must support conditional writes (`If-None-Match: *`), which Gutch
uses to allow only one push to a repository at a time. AWS S3, MinIO and R2 do.

## How it works

[`src/store.rs`](src/store.rs) implements Gutch's `RemoteObjectStore` trait
with presigned S3 requests. `ObjectStoreRemote` turns it into a full remote, as
described in
[Storage remotes](../../_docs/08_embedding_gutch.md#storage-remotes).

The app keeps a copy of each repository it opens in the local cache folder
(`CACHE/SCOPE/NAME`). Installing, verifying, repairing, listing files and
saving a file download only the snapshot they use, with its metadata and
bundles; snapshots already in the copy are not downloaded again. Saving a
file reads it straight from those bundles with `read_snapshot_file`.

A capture first pulls every snapshot, goes into the copy and is then pushed.
It needs the full copy because a push makes the bucket match the copy, and it
would delete snapshots the copy lacks.

Bundles are compressed as a whole, so listing or saving one file still
downloads the snapshot's bundles, not only that file's bytes.
[`src/ops.rs`](src/ops.rs) has these operations without the window, and its
tests run them against an in-memory store.
