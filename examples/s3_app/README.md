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

Pick the provider; the app fills in the endpoint, region and URL style.

| Provider | You fill in |
| --- | --- |
| AWS S3 | Region, such as `eu-central-1` |
| Cloudflare R2 | An API token; see below |
| OVHcloud | Region, such as `gra` or `sbg` |
| MinIO | Endpoint, such as `http://localhost:9000` |
| Other S3-compatible | Endpoint, region and whether to use path-style URLs |

Every provider also needs the bucket, a key prefix (a folder in the bucket for
Gutch's objects, such as `gutch`; empty for the bucket root) and, except R2, an
access key allowed to read, write, list and delete objects below the prefix.

**Test connection** checks the keys, the bucket and the region, and that the
bucket supports conditional writes. It writes one small test object and
deletes it. When something fails, the status line names the likely cause.

Hover over a field or button to see what it does.

On the first run the app fills in the endpoint and region from
`AWS_ENDPOINT_URL` and `AWS_REGION`. When `AWS_ACCESS_KEY_ID` and
`AWS_SECRET_ACCESS_KEY` are set, it always uses them.

### Cloudflare R2

R2 needs only an API token:

1. Click **Create token**. In the Cloudflare dashboard, under R2, choose
   **Manage API tokens** and create a token with **Admin Read & Write**.
2. Paste the token value, or set `CLOUDFLARE_API_TOKEN` before starting the app.
3. Click **Sign in**. The app finds the account, lists its buckets and derives
   the S3 keys from the token, as
   [Cloudflare documents](https://developers.cloudflare.com/r2/api/tokens/#get-s3-api-credentials-from-an-api-token):
   the access key ID is the token's ID and the secret key is the token's
   SHA-256.
4. Pick a bucket, or type a name under **New bucket** and click **Create**.

If the token may not list accounts, paste the account ID from the R2 overview
page. Buckets in the EU or FedRAMP jurisdiction need the matching jurisdiction.

## Settings and cache

The app remembers the connection fields, the cache folder, the tags and the
last folders you picked. It never saves the secret key or the Cloudflare
token: set `AWS_SECRET_ACCESS_KEY` or `CLOUDFLARE_API_TOKEN`, or type them each
time you start the app.

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
(`CACHE/SCOPE/NAME`). Installing, verifying and repairing download only the
snapshot they use, with its metadata and bundles; snapshots already in the
copy are not downloaded again. Listing files downloads the snapshot's metadata
and no bundles (`PullBundles::None`). Saving a file downloads only the bundles
holding it (`PullBundles::Files`) and reads it from them with
`read_snapshot_file`.

A capture first pulls every snapshot, goes into the copy and is then pushed.
It needs the full copy because a push makes the bucket match the copy, and it
would delete snapshots the copy lacks.

Bundles are compressed as a whole, so saving a file downloads each bundle it
touches in full; small files that share a bundle come along.
[`src/ops.rs`](src/ops.rs) has these operations without the window, and its
tests run them against an in-memory store.
