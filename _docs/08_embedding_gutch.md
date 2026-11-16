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

Library names follow the command words. A command `NOUN VERB` is the
function `VERB_NOUN`; its types put the words in command order, and a command
that shows its plan first has a `plan_` function:

| Command | Function | Types |
| --- | --- | --- |
| `repo init` | `init_repo` | `RepoInitOptions` |
| `snapshot capture` | `capture_snapshot`, `plan_capture_snapshot` | `SnapshotCaptureRequest`, `SnapshotCapturePlan`, `SnapshotCaptureResult` |
| `installation reconstruct` | `reconstruct_installation` | `InstallationRequest`, `InstallationReconstructResult` |
| `installation verify` | `verify_installation` | `InstallationVerifyRequest`, `InstallationVerifyResult` |
| `installation preview` | `preview_installation` | `InstallationPreviewEvent`, `InstallationPreviewSummary` |
| `installation repair` | `repair_installation`, `plan_repair_installation` | `InstallationRepairRequest`, `InstallationRepairResult` |
| `repo push` | `push_repo`, `plan_push_repo` | `RepoPushPlan`, `RepoPushResult` |

Commands named by a noun, such as `repo status` or `repo constraints show`,
are functions named by that noun: `repo_status`, `repo_constraints`. See the
crate documentation (`cargo doc -p gutch --open`) for the rest. Results are
plain structs, the same data the CLI prints with `--json`. Errors are `gutch::api::Error`;
`error.exit_code()` gives the status the CLI would exit with.

## Progress, cancellation and warnings

Long operations take an `OperationControl`. Attach what you need:

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
- `.with_warnings(...)` collects non-fatal conditions, and `.with_profiler(...)`
  records timings, as `--profile` does.

## Launch metadata

`set_snapshot_product_metadata` stores information an application launcher
needs with a snapshot, such as the program to start; `check_snapshot_product_metadata`
validates it. These have no CLI command.

## Remote providers

Gutch does not include a client for any particular server. The remote
commands (`repo remote ...`, `repo push`, `repo pull`) work through traits the
host application implements:

| Trait | Responsibility |
| --- | --- |
| `RemoteRepoReader` | Read a remote repository's files |
| `RemoteRepoWriter` | Write and delete them |
| `RemoteRepoProvider` | Both of the above; implemented automatically for any type that has them |
| `RemoteRepoService` | List, create and delete remote repositories, report tag references, and optionally check a push before it starts (`prepare_push`) or a new binding (`configure_binding`) |

The service owns sign-in and server configuration; Gutch only asks it for
files and repository operations.

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
