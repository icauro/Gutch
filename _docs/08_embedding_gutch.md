# Embedding Gutch

Everything `gutch-cli` does is available from the `gutch` Rust crate, through
the `gutch::api` module. An application can capture, install and repair
snapshots itself, show its own progress, and connect the remote commands to
its own server.

## A first program

[`examples/capture_and_install.rs`](../examples/capture_and_install.rs)
creates a repository, captures a folder, installs it elsewhere and verifies
the result:

```sh
cargo run --example capture_and_install -- SOURCE_DIR WORK_DIR
```

More examples in [`examples/`](../examples) each create their own data, so they
need only a work folder that does not exist yet:

| Example | Shows |
| --- | --- |
| [`update_installation`](../examples/update_installation.rs) | Listing what changed between two versions, previewing an update and updating in place while reusing installed content |
| [`repair_installation`](../examples/repair_installation.rs) | Finding damage, planning and running a repair with a progress sink |
| [`cancel_reconstruction`](../examples/cancel_reconstruction.rs) | Cancelling an update partway and keeping the old installation intact |
| [`folder_remote`](../examples/folder_remote.rs) | A remote stored in a local folder through `RemoteObjectStore`, used to push and pull |

```sh
cargo run --example update_installation -- WORK_DIR
```

The library uses the same words as the command line for the same things: a
snapshot, an installation, a repository, a remote. The command line is a layer
on top of the library and may combine several calls into one command, so not
every command is a single function.

Operations share one shape. An operation takes a request and returns a result;
an operation that can show what it would do first has a `plan_` function that
takes the same request and returns a plan:

```rust
let request = SnapshotCaptureRequest { /* ... */ };
let plan = plan_capture_snapshot(&request)?;   // SnapshotCapturePlan
let result = capture_snapshot(&request)?;      // SnapshotCaptureResult
```

Requests own their inputs and include a `control` field for progress and
cancellation, so most can be written with `..Default::default()`. Snapshot
tags are written as `SnapshotTagSet::from_pairs([("version", "1.2.0")])?`,
which checks each tag as the CLI's `--tag` does. Anything the
host supplies at run time, such as a remote provider or a callback that
receives preview events, is passed beside the request. Simple lookups, such as
`repo_status` or `repo_constraints`, take only the repository path.

| Area | Functions | Types |
| --- | --- | --- |
| Repository | `init_repo`, `plan_init_repo`, `verify_repo`, `rename_repo`, `rebuild_repo_index` | `RepoInitRequest`, `RepoInitPlan`, `RepoInitResult`, `RepoVerifyRequest`, ... |
| Snapshot | `capture_snapshot`, `plan_capture_snapshot`, `measure_snapshot`, `diff_snapshots`, `delete_snapshot` | `SnapshotCaptureRequest`, `SnapshotCapturePlan`, `SnapshotCaptureResult`, ... |
| Installation | `reconstruct_installation`, `verify_installation`, `repair_installation`, `plan_repair_installation`, `preview_installation` | `InstallationReconstructRequest`, `InstallationVerifyRequest`, `InstallationRepairRequest`, `InstallationRepairPlan`, `InstallationPreviewRequest`, ... |
| Remote | `push_repo`, `plan_push_repo`, `pull_repo`, `plan_pull_repo`, `bind_remote_repo`, `create_remote_repo`, `unbind_remote_repo` | `RepoPushRequest`, `RepoPushPlan`, `RepoPushResult`, `RepoPullRequest`, ... |

See the crate documentation (`cargo doc -p gutch --open`) for the rest. Results
are plain structs, the same data the CLI prints with `--json`: for example
`reconstruct_installation` returns `reused_chunk_count` directly, and
`verify_installation` returns `issues`, whose `matches_snapshot()` is true when
nothing is missing or corrupt. A preview defaults to what a repair would do;
set `publication: InstallationPublication::ReplaceDirectory` to preview a
reconstruction. Errors are
`gutch::api::Error`; `error.exit_code()` gives the status the CLI would exit
with.

## Progress, cancellation and warnings

The `control` field of a request is an `OperationControl`. Attach what you need:

```rust
use std::sync::Arc;
use gutch::api::OperationControl;

let control = OperationControl::default()
    .with_progress(Arc::new(MyProgress))       // impl ProgressSink
    .with_cancellation(Arc::new(MyCancelFlag)); // impl CancellationHook
```

- `ProgressSink::emit` receives each progress event; it is called from worker
  threads, so keep it quick.
- `CancellationHook::is_cancelled` is polled during the work. Once it returns
  `true`, the operation stops at the next safe point and returns an error.
  A cancelled reconstruction leaves the destination as it was.
- When updating an installation, `reuse_snapshot_id` names the snapshot that
  is already installed, and `reuse_from` points to another folder holding it
  if that is not the destination. Content already there is reused instead of
  read from the repository.
- `.with_warnings(...)` collects non-fatal conditions, and `.with_profiler(...)`
  records timings, as `--profile` does.

## Launch metadata

`set_snapshot_product_metadata` stores information an application launcher
needs with a snapshot, such as the program to start; `check_snapshot_product_metadata`
validates it. These have no CLI command.

## Remote providers

Gutch does not include a client for any particular server. The remote
commands (`repo remote ...`, `repo push`, `repo pull`) work through a provider
the host application supplies.

### Storage remotes

When the remote is plain storage, such as a folder, an S3 bucket or a blob
container, implement `RemoteObjectStore` and wrap it in `ObjectStoreRemote`:

| Method | Does |
| --- | --- |
| `get(key)` | Returns an object's bytes, or `None` |
| `put(key, bytes)` | Creates or replaces an object in one step |
| `put_if_absent(key, bytes)` | Creates an object only if the key is free, atomically; returns whether it did |
| `delete(key)` | Deletes an object; a missing one is fine |
| `list(prefix)` | Returns the keys directly below a prefix ending in `/` |
| `exists(key)`, `download_url(key)` | Optional: a cheaper existence check, and a URL (such as a presigned link) Gutch downloads from instead of calling `get` |

`ObjectStoreRemote` implements every provider trait over those methods. Gutch
chooses the keys, records the hashes it compares, and deletes bundles no
snapshot needs. Remote repository IDs are `SCOPE--NAME`.

Only one push changes a remote repository at a time. A push creates the
object `repos/REPO_ID/push.lock` with `put_if_absent` and deletes it when it
ends, whether or not it succeeded; a second push fails straight away and says
how long the first has been running. If a process dies during a push, the lock
stays; delete that object once no push is running. Snapshots a push uploads
are visible to pulls only after that push completes. Pulls take no lock, so a
pull that overlaps a push may fail and can be retried.

[`examples/folder_remote.rs`](../examples/folder_remote.rs) is a complete
store backed by a local folder.

### Servers with their own API

A server that keeps its own records implements the provider traits directly:

| Trait | Responsibility |
| --- | --- |
| `RemoteRepoReader` | Read a remote repository's files |
| `RemoteRepoWriter` | Write and delete them |
| `RemoteRepoProvider` | Both of the above; implemented automatically for any type that has them |
| `RemoteRepoService` | List, create and delete remote repositories, report tag references, and optionally check a push before it starts (`prepare_push`) or a new binding (`configure_binding`) |

The service owns sign-in and server configuration; Gutch only asks it for
files and repository operations. The `*_hash_b3` fields a provider reports for
stored files are computed with `remote_artifact_hash`. A source can be bytes,
a URL, or `RemoteArtifactSource::Fetch`, which Gutch calls only when it needs
that file. To keep pushes from overlapping, take a lock in
`RemoteRepoWriter::begin_push` and release it in `finish_push`.

### The command line

To offer the full command line inside your application, with remote commands
connected to your service, pass a factory to `run_command_line`:

```rust
use gutch::api::{run_command_line, CommandLineHost, RemoteProviderFactory};

let factory: &RemoteProviderFactory = &|| Ok(Box::new(MyService::connect()?));
let host = CommandLineHost {
    bin_name: "myapp",
    remote_provider: Some(factory),
};
if let Err(error) = run_command_line(std::env::args_os(), &host) {
    eprintln!("error: {error}");
    std::process::exit(error.exit_code());
}
```

`bin_name` is the program name shown in help and messages. The factory is
called only when a command needs the remote, so local commands work without
a connection.

## Further reading

The [integration contract](../_dev_guide/internals/05_integration/01_cli_and_app_integration.md)
describes the guarantees in depth, for those building on Gutch's internals.
