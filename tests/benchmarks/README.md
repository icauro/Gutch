# Gutch and Storage Benchmark Suite

The benchmark packages under `gutch_repo/tests/benchmarks` own the shared external-tool
binaries, command execution, typed fixture configuration, deterministic
artifact retention, and one verification measurement per reconstruct lane.

Run Gutch benchmarks interactively:

    python -m gutch_repo.tests.benchmarks.gutch_fixture_runner

The interactive Gutch launcher uses
`C:/datasets/products` as the default product lookup
root, then presents its product directories as the next numbered selection.

The Gutch launcher can also be run directly by file path:

    python gutch_repo/tests/benchmarks/gutch_fixture_runner/__main__.py

List Gutch fixtures:

    python -m gutch_repo.tests.benchmarks.gutch_fixture_runner list

Run storage benchmarks interactively:

    python -m gutch_repo.tests.benchmarks.s3_throughput

Storage commands also load `gutch_repo/tests/benchmarks/.env` by default and accept
`--env-file` for an explicit benchmark environment.

Benchmark-local server and path settings live in
`gutch_repo/tests/benchmarks/config.yml`. Copy `config.yml.example` there and
fill in the SSH target, remote workspace, fixture root, and local artifact
paths. The benchmark does not read the repository-root `config.yml`.

Storage actions are `prepare`, `download`, `delete`, and `abort-multipart`.
The destructive actions require `--yes` in explicit mode. `delete` defaults to
the scenario prefix; `abort-multipart` defaults to the whole bucket unless
`--prefix` is supplied.

Run the default local Gutch fixture:

    python -m gutch_repo.tests.benchmarks.gutch_fixture_runner --product path/to/product

Run the cold S3 publication cycle (using `gutch_repo/tests/benchmarks/.env` by default):

    python -m gutch_repo.tests.benchmarks.gutch_fixture_runner s3-cycle --product path/to/product

Copy `.env.example` to `.env`, or select another file with `--env-file`.
The benchmark suite owns this environment-file convention and does not resolve
credentials through the repository root `config.yml`.

Shared Python utilities live under `gutch_repo/tests/benchmarks/common` and
`gutch_repo/tests/benchmarks/remote`; executable benchmark runners live beside
the Gutch crate tests.

The Gutch launcher shows an ASCII spinner for each command, including its lane
and current step, then prints each measured lane as soon as it completes,
followed by the machine-readable results path. A fixture with `open_perfetto: true`
combines the retained lane traces and opens that trace in the Perfetto UI after
the run. The local trace server follows the same `127.0.0.1:9001` workflow as
the original benchmark. The completed run also prints the compact ASCII
comparison table and, when Gutch traces exist, the detailed Gutch profile
table.

Fixture roots may set `lane_wait_seconds` and `step_wait_seconds` to control
cooldown between measured lanes and between commands within a lane. Both
default to 4 seconds; set either to `0` to disable that wait.

External lanes may set an `arguments` list for capture-time tool options. The
adapter routes these arguments to the tool's repository setup and capture
command as required: Restic init/backup, Kopia global policy, Borg create, or
Plakar backup. Gutch lanes use their typed `profile` options instead.

## Source tree

    gutch_repo/tests/benchmarks/common/
      command.py       subprocess boundary, timing, and redaction
      environment.py   benchmark-owned env-file loading
      config.py        recursive YAML catalog
      artifacts.py     run/lane layout and retention
      filesystem.py    tree stats and content comparison
      traces.py        Perfetto composition with one process per benchmark lane
    gutch_repo/tests/benchmarks/
      gutch_fixture_runner/
        __main__.py    interactive and explicit Gutch launcher
        config.py      validated Gutch fixture model
        report.py      compact comparison and detailed Gutch tables
        workflow.py    capture, reconstruct, optional corruption, repair
        external.py    Restic/Kopia/Borg/Plakar adapters
        s3.py          S3 publication, URL manifests, and fetch commands
        s3_cycle.py    cold S3 publication and fetch-reconstruct cycle
        fixtures/      typed Gutch benchmark scenarios
        bin/           Gutch CLI and external comparison tools
      s3_throughput/
        __main__.py    interactive and explicit storage launcher
        config.py      typed object-storage workload config
        workflow.py    parallel upload/download phases
    gutch_repo/tests/benchmarks/remote/
      launcher.py     shared SSH/SCP execution boundary

Each measured lane gets an isolated artifact directory. Reconstruct owns its single
verification measurement through the Gutch stats payload; corruption and
repair are one optional fixture-level phase that never issues another verify
command. Set corrupt_repair: true at the fixture root to run it for every
lane; omit it or set it to false to skip it. Scan, recount/inventory,
comparison, and each external tool's native verification are always run.
Snapshots are captured chronologically and reconstructed newest-first by
default in every multi-snapshot lane.
Generated repositories, payloads, and traces live below .artifacts/ and are
never part of the source tree.
