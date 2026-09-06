# Gutch and Storage Benchmark Suite

The suite separates three responsibilities:

- `common/dataset.py` freezes selected input inventories and numeric version order.
- `common/runner.py` schedules local and S3 fixture cases, records outcomes and
  repeated samples, and removes payloads only after success.
- `gutch_repo/test_support/` owns command execution, Gutch operations, filesystem
  validation, S3 transport, traces, and evidence layout. Regression tests use
  these primitives directly without importing benchmark workflows.

Fixture lanes declare `tool` and `workload` separately. Only Gutch lanes accept
Gutch profiles. `measurement_mode: performance` disables Gutch traces;
`profile` (the default) retains them. `repetitions` defaults to one and rotates
case order across samples. Warmup lanes execute once and are excluded from
measured results. OS caches are uncontrolled; these are fresh-repository runs,
not guaranteed cold-cache measurements.

Each run retains `run.json`, incremental `results.json`, `summary.json`
(sample count, median, minimum, maximum), command logs, raw stats, validation
results, and available traces. Artifact links in serialized fixture results
are relative to the run directory. Failed or cancelled runs retain payloads.
Docker exports an evidence archive into a unique host run directory. Generated
run and lane directory names are bounded and hash-suffixed when shortened;
full fixture and case labels remain in result metadata. This limits Windows
path growth; use a short artifact root when source trees are deeply nested.

The Borg adapter reads the executable's major version: the pinned Linux Borg 1
uses repository/archive syntax, while Windows Borg 2 uses explicit repository
options and `repo-create`. The detected version is retained in lane metadata.
Kopia disables credential persistence without platform-specific keyring flags.

Process timings exclude cooldown and independent correctness checks. Embedded
Gutch verification, external repository checks, incremental Gutch repair, and
external full-restore recovery are separate metrics. Throughput uses selected
input bytes. Dataset fingerprints identify the exact input inventory: whole
product and selected-snapshot workloads can differ and must not be treated as
identical merely because they share a product directory.

## Reading Gutch versus external-tool comparisons

These are configured end-to-end workflow measurements. They can compare the
cost of delivering the same selected files on the same machine; they do not
isolate the efficiency of a compression, chunking, or deduplication algorithm.
A successful Blender run establishes correctness for that dataset, not a
performance ranking or equivalence of backup features.

| Measurement | When comparable | Current limitations |
|---|---|---|
| Individual capture and restore | Same selected target directories, history, order, host, and performance mode | Each tool uses its own repository format, chunking, compression, protection, and concurrency settings. Initialization is outside capture timing. |
| Bulk capture and restore | Same whole-product inventory | All five tools capture the entire product root, including root files and hidden directories. Input counts, fingerprints, and validation use that same whole tree. Restic, Borg, and Plakar capture `.` from the product directory; Kopia and Gutch receive the product path. |
| Restore time | Time until each configured restore command completes | Gutch includes embedded output verification; Restic restores use `--verify` to check restored contents. Other adapters have no equivalent output-reread switch established by their installed restore help. External repository checks run separately; they are not equivalent to verifying restored files. Embedded verification is already part of Gutch restore time and must not be added again. |
| Recovery | Operational cost of the stated recovery strategy | Gutch repairs an existing damaged output incrementally. External tools restore the output from scratch; deleting the old output is outside that timer. This compares strategies, not equivalent repair implementations. Corruption targets output files, not stored repository data. |
| Repository size and ratio | Logical repository-file bytes relative to matched input bytes | "Repo logical size" sums file lengths, not allocated disk space, and excludes caches outside the repository. All local repository lanes sample immediately after capture, before restore or recovery. This is total retained repository size, not incremental snapshot growth. |
| Filesystem copy | A local copy baseline | The separate "Copy time" column measures one local copy. The baseline provides no versioned repository and has no capture or restore timing. |
| S3 cycles | Comparing Gutch layouts under the same storage conditions | These exercise publication, fetching, and reconstruction; they do not establish a cross-tool network-performance comparison. |

The external comparison fixtures explicitly use `measurement_mode: performance`
to disable Gutch tracing. Other fixtures still default to `profile`; set
performance mode when adding timing comparisons. `open_perfetto: false` only
disables opening the viewer. Profiling instruments Gutch alone.

Compression remains tool-specific: Gutch uses Zstd level 1, Restic requests
`fastest`, Kopia requests `zstd-fastest`, and Borg requests `zstd,1`; Plakar
keeps its native configuration. Matching preset names would not match chunk
boundaries, codecs, or CPU work. Borg explicitly disables encryption and
Plakar uses plaintext; Restic and Kopia use password-protected repositories.
These configured protection and format differences remain part of the timing.
Installed restore help exposes Restic overwrite/delete controls, Kopia
overwrite/skip-existing/delete-extra controls, and Borg interrupted-extraction
continuation. These are not a common content-verified incremental repair
contract; the suite retains full restore for external recovery. Plakar restore
help exposes destination/selection options, without an equivalent repair mode.
Repository verification and recovery metrics stay separate.

Use repeated samples and inspect `summary.json`, keeping the host, dataset,
tool binaries, and competing workload fixed. One repetition is the default;
case-order rotation only spans all starting positions after a full case-count
cycle. Warmups use separate repositories, and inventory hashing reads source
files before measurement. Neither cooldown nor warmup guarantees cold OS caches.
Do not treat results from different Windows/Linux Blender distributions or
Borg major versions as measurements of the same workload.

Check the selected paths and inventories as well as byte counts. Fingerprints
include inventory grouping, so whole-product and per-target fingerprints can
differ even when file contents match. Throughput is logical input bytes divided
by elapsed command time, not measured disk or network bandwidth. Correctness
checks cover paths, contents, symlink targets, read-only state, and executable
state where supported; they do not certify timestamps, ownership, ACLs,
extended attributes, hardlink identity, or sparse allocation.

Run Gutch benchmarks interactively:

    python -m gutch_repo.test_benchmarks.gutch_fixture_runner

The interactive Gutch launcher uses `local.product_root` from the benchmark
YAML config as its product lookup root, then presents its product directories
as the next numbered selection.

The Gutch launcher can also be run directly by file path:

    python gutch_repo/test_benchmarks/gutch_fixture_runner/__main__.py

List Gutch fixtures:

    python -m gutch_repo.test_benchmarks.gutch_fixture_runner list

Run storage benchmarks interactively:

    python -m gutch_repo.test_benchmarks.s3_throughput

Benchmark-local server and path settings live in
`gutch_repo/test_benchmarks/config.yml`. Copy `config.yml.example` there and
set local artifact paths and any external S3 credentials used by standalone
storage benchmarks. Docker benchmarks provision their own S3 service. The benchmark does not
read the repository-root `config.yml`.

Storage actions are `prepare`, `download`, `delete`, and `abort-multipart`.
The destructive actions require `--yes` in explicit mode. `delete` defaults to
the scenario prefix; `abort-multipart` defaults to the whole bucket unless
`--prefix` is supplied.

Run the default local Gutch fixture:

    python -m gutch_repo.test_benchmarks.gutch_fixture_runner --product path/to/product

Run the cold S3 publication cycle (using the benchmark YAML config by default):

    python -m gutch_repo.test_benchmarks.gutch_fixture_runner s3-cycle --product path/to/product

Use `--config path/to/config.yml` to select another benchmark config. Keep the
filled config local and never commit S3 secrets.

Run the fixed Linux benchmark in Docker:

    python -m gutch_repo.test_benchmarks.gutch_fixture_runner docker

The Ubuntu builder compiles `gutch-cli`; an isolated integration Compose project
runs it alongside its own SeaweedFS. Blender datasets are generated inside the
run's volume. Results and Perfetto traces are copied to the configured artifact
root before successful runs remove their containers, volumes, and network.
Failures retain the environment for inspection. `--keep-artifacts` retains a
successful environment too. Shared compiler caches persist separately.

Launch the Docker fixture menu:

    python -m gutch_repo.test_benchmarks.docker

Progress includes preparation, lanes, and individual Gutch commands. No SSH
credentials, remote workspace, or WSL distribution is required.

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
from a generated product for individual snapshot lanes. The Docker external
comparison uses `snapshot_limit: 1` to keep its five-tool comparison practical.

## Source tree

    gutch_repo/test_benchmarks/common/
      environment.py   benchmark-owned YAML environment loading
      config.py        recursive YAML catalog
      dataset.py       frozen input inventories and snapshot selection
      runner.py        shared case scheduling, samples, and retention
    gutch_repo/test_support/
      command.py       subprocess boundary, timing, and redaction
      artifacts.py     run/lane layout and retention
      filesystem.py    tree stats and content comparison
      gutch.py         shared Gutch operations
      s3.py            S3 transport and publication helpers
      traces.py        Perfetto composition
    gutch_repo/test_benchmarks/
      gutch_fixture_runner/
        __main__.py    interactive and explicit Gutch launcher
        config.py      validated Gutch fixture model
        report.py      compact comparison and detailed Gutch tables
        workflow.py    capture, reconstruct, optional corruption, repair
        external.py    Restic/Kopia/Borg/Plakar adapters
        external_tools.py pinned Linux x86_64 external-tool installers
        s3_cycle.py    cold S3 publication and fetch-reconstruct cycle
        fixtures/      typed Gutch benchmark scenarios
        bin/           Gutch CLI and external comparison tools
      s3_throughput/
        __main__.py    interactive and explicit storage launcher
        config.py      typed object-storage workload config
        workflow.py    parallel upload/download phases
    gutch_repo/test_benchmarks/docker/
      launcher.py     Docker execution and artifact-copy boundary

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
