# Gutch

Gutch captures files into deduplicated snapshots, stores compressed artifacts,
and fetches, reconstructs, verifies, and repairs their contents. Applications
consume the Rust `gutch::api` library or the `gutch-cli` executable.

## Build and test

Run from this directory with Rust, Python 3.12 or newer, and uv installed:

```sh
cargo build --locked --workspace
uv sync --locked
uv run python tests/run_all.py --suite fast
uv run python tests/run_all.py --suite local
uv run python tests/run_all.py --suite all
```

`fast` runs the Python tooling tests, Rust tests, and one CLI roundtrip. `local`
runs all local CLI scenarios. `all` also starts a disposable Docker Compose S3
fixture with synthetic credentials and removes its containers and volumes on
completion. Docker must be available for S3 and Linux benchmarks.

Build data lives in `.builds/`; test evidence, generated fixtures, and benchmark
results live in `tests/.artifacts/`. Failed correctness cases retain their local
payloads. Successful cases keep reports and command logs; `--keep-artifacts`
also retains their payloads. S3 fixture data is disposable even on failure.

```sh
uv run python test_benchmarks/gutch_fixture_runner/__main__.py list
uv run python test_benchmarks/gutch_fixture_runner/__main__.py docker --fixture gutch_container_trace_single_lane
```

The Docker benchmark builds only Gutch's explicit source inventory.
`python test_services/build_image.py` builds that Linux image directly. Optional local benchmark
settings belong in the ignored `test_benchmarks/config.yml`; start with its
example file. External-tool comparisons may download the requested tools.

Callers may invoke the same scripts from another project. The correctness runner
accepts `--artifacts-root`, `--event-file`, `--run-id`, and `--suite-id` for output
integration; it does not read a host application's configuration.

See [CLI usage](usage.md) and [engine contracts](docs/current_state/README.md).
This source tree is not yet licensed for public distribution; choosing a license
and reviewing the intended public export are required before publication.
