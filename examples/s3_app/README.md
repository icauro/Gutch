# Gutch S3 app

A desktop app that keeps Gutch repositories in an S3-compatible bucket (AWS S3,
MinIO, Cloudflare R2 and others). With it you can:

- create a repository in the bucket, or open one that is already there
- capture a local folder as a tagged snapshot and push it
- install any snapshot to a local folder, or update an installed one in place
- verify an installation and repair missing or changed files

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

The app fills in the credentials from `AWS_ACCESS_KEY_ID` and
`AWS_SECRET_ACCESS_KEY` when they are set, and the endpoint and region from
`AWS_ENDPOINT_URL` and `AWS_REGION`. It never saves the secret; it is kept in
memory only while the app runs.

The bucket must support conditional writes (`If-None-Match: *`), which Gutch
uses to allow only one push to a repository at a time. AWS S3, MinIO and R2 do.

## How it works

[`src/store.rs`](src/store.rs) implements Gutch's `RemoteObjectStore` trait
with presigned S3 requests. `ObjectStoreRemote` turns it into a full remote, as
described in
[Storage remotes](../../_docs/08_embedding_gutch.md#storage-remotes).

The app keeps a full copy of each repository it opens in the local cache folder
(`CACHE/SCOPE/NAME`; the default is a folder in the system temp directory).
Before each operation it pulls new snapshots from the bucket into that copy.
A capture goes into the copy and is then pushed; installs, verifies and repairs
read from the copy. [`src/ops.rs`](src/ops.rs) has these operations without the
window, and its tests run them against an in-memory store.
