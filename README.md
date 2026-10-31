# Gutch

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
[provider integration contract](docs/current_state/10_cli_and_app_integration.md)
for the supported host interfaces.

Its current purpose is software delivery. Although it shares techniques with
backup tools, it does not preserve all filesystem metadata or provide a complete
backup service. See [current limitations](docs/current_state/14_limitations.md)
for supported boundaries and their reasons.

## First successful run

Install Python 3.12 or newer, uv, and a Rust toolchain supporting edition 2024.
The standalone CI currently uses Rust 1.97.1. On Windows, Rust also needs the
MSVC C++ build tools. Run these commands from this directory:

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

Start with [your first change](CONTRIBUTING.md), the [test map](tests/README.md),
and the separately installed `comparison-benchmark-suite` package.
`python gutch.py benchmark` lists internal engine diagnostics. Cross-tool
comparisons run only through the separate suite. See the [benchmark guide](tests/benchmarks/README.md). Failed cases print an
absolute evidence path and a `python gutch.py replay` command. Build outputs
live in `.builds/`; generated test data and reports live in `tests/.artifacts/`.

The developer commands are separate from the product executable, `gutch-cli`.
All commands work when this directory is exported as a standalone project.

## Profiling and progress

Profiling defaults to `off`. Use `--profile summary` for coarse timings, or
`--profile detailed --profile-dir /absolute/report/directory` for a sampled
Perfetto timeline and summary. Live download and installation-write speeds work
with profiling off. See the [profiling contract](docs/current_state/12_performance_and_development.md)
for measurement meanings and recording limits.

## Source navigation

Start with [the public API](src/api.rs). [Capture](src/capture/) owns source
selection through packing; [installation](src/installation/) owns inspection,
change planning, execution and publication. [Transfer](src/transfer/) supplies
artifact readiness, with separate scheduling, adaptive-policy and feedback owners.
[Repository](src/repo/) owns acquisition, guarded readers, persistence, index
recovery and synchronization. Installation coordinates combined fetch and
reconstruction; [artifact](src/artifact/) owns the binary codecs. The
[module map](docs/current_state/01_operation_ownership.md#crate-and-module-map) records dependency boundaries.

Reconstruction and repair use the same executor. Reconstruction replaces a
verified staged tree; repair leaves healthy entries in place and publishes
changed entries individually. Preview changes without writing the destination:

```sh
gutch-cli installation preview --repo /path/to/repo --snapshot SNAPSHOT_ID --destination /path/to/install
```

Output is readable by default; use `--json-lines` to stream structured events. See [installation policies and preview](docs/current_state/08_installation_verification_and_repair.md)
for verification levels, extra files, estimates, and failure behavior.

See [CLI usage](usage.md) and [engine contracts](docs/current_state/README.md).
This source tree is not yet licensed for public distribution; choosing a license
and reviewing the intended public export are required before publication.

The comparison suite is a separate root project. Before using Gutch benchmark
commands, install its checkout into the same Python environment:
`uv pip install --python .venv -e <suite-checkout>`. Gutch does not search sibling folders.
Its build image includes the installed suite when available; build and engine
correctness commands do not require it.
