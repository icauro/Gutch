# Gutch

Original project code is [MIT licensed](LICENSE). Included third-party source
retains its own terms; see [third-party notices](THIRD_PARTY_NOTICES.md).

Gutch is a Rust artifact engine for software delivery. It packages source folders
into deduplicated, compressed snapshots and reconstructs verified installations
from them. Applications use the `gutch::api` library or the `gutch-cli` executable.

Each snapshot describes an installable filesystem tree. Gutch splits file content
into reusable chunks and stores them as artifacts. Files and snapshots in the
same repository share identical chunks, reducing duplicate storage and allowing
installations and updates to reuse available content instead of downloading it
again.

Gutch owns capture, repository synchronization, artifact transfer, reconstruction,
verification, and repair. Hosts provide remote connections and authorized artifact
sources, and manage application workflows. See the
[provider integration contract](_docs/integration/cli_and_app_integration.md)
for the supported host interfaces.

Its current purpose is software delivery. Although it shares techniques with
backup tools, it does not preserve all filesystem metadata or provide a complete
backup service. See [current limitations](_docs/overview/limitations.md)
for supported boundaries and their reasons.

## First successful run

To download the CLI, open **Actions > Build Gutch CLI** and select a successful
run. Artifacts cover Linux and Windows on x86-64 and ARM64, with checksums and
notices. Builds run on code pushes to `main` or manually. See
[CLI builds](_docs/development/cli_builds.md).

Install Python 3.12 or newer, uv, and Rust 1.100 or newer, the minimum the
crates declare; the standalone CI uses Rust 1.100.0. Cargo rejects unused
dependencies, and `.cargo/config.toml` makes the resolver skip crate versions
published less than seven days ago. On Windows, Rust also needs the
MSVC C++ build tools, and the test corpus needs long paths: clone with
`git clone -c core.longpaths=true` (or run `git config core.longpaths true`
before checking out). Run these commands from this directory:

```sh
python gutch.py setup
python gutch.py test
```

Setup creates the locked local Python environment. Later commands select that
environment automatically; no shell activation is needed. The default test runs
Python tooling, Rust tests and all local CLI scenarios. First-time compilation
can take several minutes. A successful run prints a report path and exits zero.

```sh
python gutch.py --help
python gutch.py test --list
python gutch.py test --suite repository
python gutch.py benchmark
```

Docker is required only for the S3 suite and container benchmarks. Run
`python gutch.py test --suite s3` to exercise disposable object storage.
`python gutch.py build-image` builds the standalone Linux image. Run its local
checks with `docker run --rm --init gutch-builder python gutch.py test --suite check`;
`--init` reaps descendants terminated by timeout tests.

Start with [your first change](CONTRIBUTING.md) and the [test map](tests/README.md).
`python gutch.py benchmark` lists Gutch's own diagnostic benchmarks; see the
[benchmark guide](tests/benchmarks/README.md). Comparisons with other tools live in
[Gutch-Benchmarks](https://github.com/icauro/Gutch-Benchmarks). Failed cases print an
absolute evidence path and a `python gutch.py replay` command. Build outputs
live in `.builds/`; generated test data and reports live in `.artifacts/tests/`.

The developer commands are separate from the product executable, `gutch-cli`.
All commands work when this directory is exported as a standalone project.

## Profiling and progress

Profiling defaults to `off`. Use `--profile summary` for coarse timings, or
`--profile detailed --profile-dir /absolute/report/directory` for a sampled
Perfetto timeline and summary. Live download and installation-write speeds work
with profiling off. See the [profiling contract](_docs/development/performance_and_benchmarks.md)
for measurement meanings and recording limits.

## Source navigation

Start with [the public API](src/11_api.rs). [Capture](src/08_capture) owns source
selection through packing; [installation](src/09_installation) owns inspection,
change planning, execution and publication. [Transfer](src/05_transfer) supplies
artifact readiness, with separate scheduling, adaptive-policy and feedback owners.
[Repository](src/07_repo) owns acquisition, guarded readers, persistence, index
recovery and synchronization. Installation coordinates combined fetch and
reconstruction; [artifact](src/04_artifact) owns the binary codecs. The
[module map](_docs/overview/operation_ownership.md#crate-and-module-map) records dependency boundaries.

Reconstruction and repair use the same executor. Reconstruction replaces a
verified staged tree; repair leaves healthy entries in place and publishes
changed entries individually. Preview changes without writing the destination:

```sh
gutch-cli installation preview --repo /path/to/repo --snapshot SNAPSHOT_ID --destination /path/to/install
```

Output is readable by default; use `--json-lines` to stream structured events. See [installation policies and preview](_docs/installation/verification_and_repair.md)
for verification levels, extra files, estimates, and failure behavior.

See [CLI usage](usage.md) and [engine contracts](_docs/README.md).
