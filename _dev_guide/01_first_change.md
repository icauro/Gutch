# Make your first change

All commands below run from the Gutch root; no containing project is required.

## Setup

Install Python 3.12 or newer, uv, and Rust 1.100 or newer, the minimum the
crates declare; the standalone CI uses Rust 1.100.0. Cargo rejects unused
dependencies, and `.cargo/config.toml` makes the resolver skip crate versions
published less than seven days ago. On Windows, Rust also needs the
MSVC C++ build tools, and the test corpus needs long paths: clone with
`git clone -c core.longpaths=true` (or run `git config core.longpaths true`
before checking out).

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

Failed cases print an absolute evidence path and a `python gutch.py replay`
command. Build outputs live in `.builds/`; generated test data and reports live
in `.artifacts/tests/`. The developer commands are separate from the product
executable, `gutch-cli`. All commands work when this directory is exported as a
standalone project.

## Steps

1. Run `python gutch.py test` to establish a baseline. It runs Python tooling,
   Rust tests and local CLI scenarios. S3 is an explicit additional suite.
2. Find the owner in the [module map](internals/01_overview/01_operation_ownership.md#crate-and-module-map).
   Public contracts live in `src/11_api.rs`; algorithm tests stay beside the
   Rust implementation. [Code and test layout](02_code_and_test_layout.md) says
   where other work belongs.
3. Find a relevant scenario with `python gutch.py test --list`. A case such as
   `repository/deletion_preserves_shared_artifacts` maps directly to
   `tests/scenarios/repository/deletion_preserves_shared_artifacts.py`.
4. Make the change and run that case with `python gutch.py test --case <case>`.
   A new CLI behavior gets its own scenario file; see
   [scenario files](02_code_and_test_layout.md#scenario-files).
5. Run the default suite again. Run `python gutch.py test --suite s3` when
   changing remote delivery or service setup. Docker must be running.
6. Update the owning page in the [engine internals](internals/README.md), and the
   [user guide](../_docs/README.md) when user-visible behavior changes.
   Include validation and any skipped or unavailable checks in the change
   description.

The [test guide](../tests/README.md) lists every suite and option.

## Investigate a failure

The runner prints the case identifier, failure reason, absolute evidence path,
and replay command. Start with `failure.txt`, `scenario.log`, and the comparison
JSON in that directory. Numbered command logs show which operation failed.

Run `python gutch.py replay <case-directory>` to rerun the same scenario in a
fresh workspace. The command reports source changes since the original run.
It does not overwrite the original failure or automatically check out old code.
Repository lifecycle failures also retain the selected original input, independent
expectations when provided, and the seed and operation sequence for generated data.
Timing-dependent failures may need more than one replay; this is scenario replay,
not a recorded operating-system schedule.
