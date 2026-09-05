# Gutch and Storage Benchmark Suite

The benchmark packages under `gutch_repo/benchmarks` own the shared external-tool
binaries, command execution, typed fixture configuration, deterministic
artifact retention, and one verification measurement per reconstruct lane.

Run Gutch benchmarks interactively:

    python -m gutch_repo.benchmarks.gutch_fixture_runner

The interactive Gutch launcher uses `local.product_root` from the benchmark
YAML config as its product lookup root, then presents its product directories
as the next numbered selection.

The Gutch launcher can also be run directly by file path:

    python gutch_repo/benchmarks/gutch_fixture_runner/__main__.py

List Gutch fixtures:

    python -m gutch_repo.benchmarks.gutch_fixture_runner list

Run storage benchmarks interactively:

    python -m gutch_repo.benchmarks.s3_throughput

Benchmark-local server and path settings live in
`gutch_repo/benchmarks/config.yml`. Copy `config.yml.example` there and
fill in the SSH target, remote workspace, fixture root, local artifact paths,
and S3 credentials under the `environment` mapping. The benchmark does not
read the repository-root `config.yml`.

Storage actions are `prepare`, `download`, `delete`, and `abort-multipart`.
The destructive actions require `--yes` in explicit mode. `delete` defaults to
the scenario prefix; `abort-multipart` defaults to the whole bucket unless
`--prefix` is supplied.

Run the default local Gutch fixture:

    python -m gutch_repo.benchmarks.gutch_fixture_runner --product path/to/product

Run the cold S3 publication cycle (using the benchmark YAML config by default):

    python -m gutch_repo.benchmarks.gutch_fixture_runner s3-cycle --product path/to/product

Use `--config path/to/config.yml` to select another benchmark config. Keep the
filled config local and never commit S3 secrets or SSH private-key paths that
should remain private.

Run the fixed remote Linux benchmark without selecting a product:

    python -m gutch_repo.benchmarks.gutch_fixture_runner remote \
      --config gutch_repo/benchmarks/config.yml

The remote command compiles the standalone `gutch-cli` checkout locally through
the configured WSL Linux
builder, uploads that verified Linux binary, and performs no Rust compilation
on the server. It then generates the configured Blender versions, runs the
Gutch S3 cycle against `S3_MAIN`, downloads results and the combined Perfetto
trace, and removes the remote workspace and S3 prefix. Use
`--keep-artifacts` to retain benchmark artifacts. The generated Blender test
dataset is cached remotely by default; after results are shown, the launcher
asks whether to delete it (`N` is the default). For a low-concurrency diagnostic run, add
`--transfer-concurrency 1 --transfer-max-concurrency 1`.

Launch the remote benchmark menu, which loads the configured benchmark YAML
and discovers all remote Gutch fixtures automatically:

    python -m gutch_repo.benchmarks.remote

The menu includes the single-lane trace fixture, which runs the same default
capture/reconstruct/corrupt/repair flow as the local trace fixture against the
generated Blender product, without publishing to S3.

While the remote job runs, the local terminal receives preparation, lane, and
individual Gutch command-step updates prefixed with `[remote]`.

Shared Python utilities live under `gutch_repo/benchmarks/common` and
`gutch_repo/benchmarks/remote`; executable benchmark runners live beside
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

Fixtures may set `snapshot_limit` to use only the newest N target directories
from a generated product for individual snapshot lanes. The remote external
comparison uses `snapshot_limit: 1` to keep its five-tool comparison practical.

## Source tree

    gutch_repo/benchmarks/common/
      command.py       subprocess boundary, timing, and redaction
      environment.py   benchmark-owned YAML environment loading
      config.py        recursive YAML catalog
      artifacts.py     run/lane layout and retention
      filesystem.py    tree stats and content comparison
      traces.py        Perfetto composition with one process per benchmark lane
    gutch_repo/benchmarks/
      gutch_fixture_runner/
        __main__.py    interactive and explicit Gutch launcher
        config.py      validated Gutch fixture model
        report.py      compact comparison and detailed Gutch tables
        workflow.py    capture, reconstruct, optional corruption, repair
        external.py    Restic/Kopia/Borg/Plakar adapters
        external_tools.py pinned Linux x86_64 external-tool installers
        s3.py          S3 publication, URL manifests, and fetch commands
        s3_cycle.py    cold S3 publication and fetch-reconstruct cycle
        fixtures/      typed Gutch benchmark scenarios
        bin/           Gutch CLI and external comparison tools
      s3_throughput/
        __main__.py    interactive and explicit storage launcher
        config.py      typed object-storage workload config
        workflow.py    parallel upload/download phases
    gutch_repo/benchmarks/remote/
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
