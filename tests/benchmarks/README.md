# Gutch and Storage Benchmark Suite

Run these commands from the Gutch project root.

The suite separates three responsibilities:

- `scheduling/dataset.py` freezes selected input inventories and numeric version order.
- `scheduling/runner.py` schedules local and S3 fixture cases, records outcomes and
  repeated samples, and removes payloads only after success.
- `tests/benchmarks/support/` owns command execution, Gutch operations, filesystem
  validation, S3 transport, traces, and evidence layout for benchmarks only.
  Correctness tests own separate helpers and datasets under `tests/`.

Fixture lanes declare `tool` and `workload` separately. Only Gutch lanes accept
Gutch profiles. `profile_mode: "off"` is the default; `summary` adds coarse
profiling and `detailed` adds sampled traces and external process-memory sampling. `repetitions` defaults to one and rotates
case order across samples. Warmup lanes execute once and are excluded from
measured results. OS caches are uncontrolled; these are fresh-repository runs,
not guaranteed cold-cache measurements.

During execution, each run records `run.json`, incremental `results.json`,
`summary.json`, command logs, raw stats and validation results. Successful runs
retain root summaries and the combined trace, then remove detailed lanes, temporary
repositories and tool caches. Result paths describe the original measurements;
`detailed_evidence_retained` in `run.json` states whether those files still exist.
`--keep-artifacts` or an explicit recipe with `delete_artifacts: false` retains
details. Failed and cancelled runs keep evidence. Automatic retention keeps the
latest 20 finalized unpinned successful runs per artifact root.

Successful S3 publication runs also delete their newly allocated UUID object
prefixes unless artifacts are explicitly retained. Failed publication prefixes
remain recorded for investigation; shared buckets and source datasets are never
deleted. Every benchmark command has a 900-second default deadline, overridable
with `GUTCH_BENCHMARK_TIMEOUT_SECONDS`. A timeout kills the process tree and records
failure evidence. Unreadable source directories fail validation.
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
| Repository size and ratio | Logical repository-file bytes relative to matched input bytes | "Stored size (final)" sums file lengths, not allocated disk space, and excludes caches outside the repository. All local repository lanes sample immediately after capture, before restore or recovery. This is total retained repository size, not incremental snapshot growth. |
| Filesystem copy | Same selected snapshots as individual lanes | Capture copies each snapshot into a separate stored directory; restore copies those directories into fresh output, newest first. Size is the retained copies, without compression or deduplication. Independent validation runs outside timing; copy has no embedded content verification. Recovery recopies from storage. |
| S3 cycles | Comparing Gutch layouts under the same storage conditions | These exercise publication, fetching, and reconstruction; they do not establish a cross-tool network-performance comparison. |

All bundled comparison fixtures select `profile_mode: detailed` and
`open_perfetto: true`. Set `profile_mode: "off"` and `open_perfetto: false`
explicitly when collecting timings without profiling overhead. Quote `"off"` in YAML to avoid boolean interpretation. `open_perfetto: false` only
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

    python gutch.py benchmark interactive

The interactive Gutch launcher uses `local.product_root` from the benchmark
YAML config as its product lookup root, then presents its product directories
as the next numbered selection.

List Gutch fixtures:

    python gutch.py benchmark list

Run storage benchmarks interactively:

    python gutch.py benchmark storage

Benchmark-local server and path settings live in
`tests/benchmarks/config.yml`. Copy `config.yml.example` there and
set local artifact paths and any external S3 credentials used by standalone
storage benchmarks. Docker benchmarks provision their own S3 service. The benchmark does not
read the repository-root `config.yml`.

`tests/benchmarks/config.yml` is the only default local benchmark configuration.
Paths inside it are relative to that file unless absolute. Keep generated
repositories and datasets under `tests/.artifacts/`; obsolete top-level tooling
directories are not configuration or data locations.

Storage actions are `prepare`, `download`, `delete`, and `abort-multipart`.
The destructive actions require `--yes` in explicit mode. `delete` defaults to
the scenario prefix; `abort-multipart` defaults to the whole bucket unless
`--prefix` is supplied.

Run the default local Gutch fixture:

    python gutch.py benchmark --product path/to/product

Run the cold S3 publication cycle (using the benchmark YAML config by default):

    python gutch.py benchmark s3-cycle --product path/to/product

Use `--config path/to/config.yml` to select another benchmark config. Keep the
filled config local and never commit S3 secrets.

Run the fixed Linux benchmark in Docker:

    python gutch.py benchmark docker

Gutch's source-inventory builder and Dockerfile compile `gutch-cli`; a run-owned Compose project runs
it alongside SeaweedFS with synthetic credentials. Results and traces are
exported before teardown. Containers, volumes, and the network are removed on
success or failure; exported evidence remains. `--keep-artifacts` affects
benchmark payload retention, not service lifetime. The builder is owned by this project.

Launch the Docker fixture menu:

    python -m tests.benchmarks.docker

Progress includes preparation, lanes, and individual Gutch commands. No SSH
credentials, remote workspace, or WSL distribution is required.

The Gutch launcher shows an ASCII spinner for each command, including its lane
and current step, then prints each measured lane as soon as it completes,
followed by the machine-readable results path. A fixture with `open_perfetto: true`
combines the retained lane traces and opens that trace in the Perfetto UI after
the run. The local trace server follows the same `127.0.0.1:9001` workflow as
the original benchmark. The completed run also prints the compact ASCII
per-snapshot comparison tables followed by a summary of total and average
capture/restore times, throughput, and final stored size. Averages use measured
snapshots within each lane/sample; throughput divides total input bytes by total
operation time. Batch capture has no per-snapshot capture timing or average;
whole-product lanes have one measurement for the product root. Repository file
counts remain in JSON results. Verification and enabled recovery measurements
appear separately. When Gutch traces exist, a detailed Gutch profile table follows.

Fixture roots may set `lane_wait_seconds` and `step_wait_seconds` to control
cooldown between measured lanes and between commands within a lane. Both
default to 4 seconds; set either to `0` to disable that wait.

External lanes may set an `arguments` list for capture-time tool options. The
adapter routes these arguments to the tool's repository setup and capture
command as required: Restic init/backup, Kopia global policy, Borg create, or
Plakar backup. Gutch lanes use their typed `profile` options instead.

Fixtures may set `snapshot_limit` to use only the newest N target directories
from a generated product for individual snapshot lanes and filesystem copy. The Docker external
comparison uses `snapshot_limit: 1` to keep its five-tool comparison practical.

## Source tree

    tests/benchmarks/scheduling/
      environment.py   benchmark-owned YAML environment loading
      config.py        recursive YAML catalog
      dataset.py       frozen input inventories and snapshot selection
      runner.py        shared case scheduling, samples, and retention
    tests/benchmarks/support/
      command.py       subprocess boundary, timing, and redaction
      artifacts.py     run/lane layout and retention
      filesystem.py    tree stats and content comparison
      gutch.py         shared Gutch operations
      s3.py            S3 transport and publication helpers
      traces.py        Perfetto composition
    tests/benchmarks/
      comparison/
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
    tests/benchmarks/docker/
      launcher.py     Docker execution and artifact-copy boundary

Each measured lane gets an isolated artifact directory. Reconstruct owns its single
verification measurement through the Gutch stats payload; corruption and
repair are one optional fixture-level phase that never issues another verify
command. Set corrupt_repair: true at the fixture root to run it for every
lane; omit it or set it to false to skip it. Scan, recount/inventory,
comparison, and each external tool's native verification are always run.
Snapshots are captured chronologically and reconstructed newest-first by
default in every multi-snapshot lane.
Generated repositories, payloads, and traces live below tests/.artifacts/ and are
never part of the source tree.

## Process memory profiling

Use fixture `gutch_default_memory_with_corrupt_repair` for the default workload.
`profile_mode: detailed` activates both trace and memory collection. The old
`memory_profile` and `measurement_mode` keys are rejected. Each command retains a
`commands/NNNN.memory.jsonl` timeline and a `memory` summary in its command JSON.
Each operation report directory contains `summary.json`, `trace.json`, and
`memory.json`. The sampler requests a 2 ms interval and reports the observed
mean interval; actual cadence depends on OS scheduling.
It observes the CLI process: sampled RSS, Windows private
bytes and observed Windows peak working set. Unsupported or unobserved values
are null. Sampling can miss short peaks and excludes child processes; RSS is not
an allocation profile or a repository-size-independent memory guarantee.

Timelines use seconds since sampler start, near command launch. Correlate these
with command traces to investigate phases. Sampling and tracing add overhead;
disable both explicitly in the selected fixture for timing comparisons.

Local S3 benchmarks require explicit storage settings in the benchmark config
(`GUTCH_S3_*`) or command-line options. They do not assume an existing application
storage service. Docker benchmark runs provision their own disposable storage.

## Shared corpus inputs

Use `python gutch.py dataset prepare CASE`, then `python gutch.py benchmark --dataset CASE`.
Local comparisons and S3 cycle/reconstruction commands accept `--dataset` instead
of `--product`. Sources are validated against the committed case inventory;
benchmarks build their own repositories and leave the frozen major-version
repositories unchanged. Blender downloads require the explicit preparation command.

## Binary layout experiment

Run `python tests/benchmarks/binary_layout_experiment.py --repetitions 7` from
this project root using Python 3.14 (standard-library `compression.zstd`). This
standalone experiment compares metadata representations using the frozen corpus
and synthetic edge cases, checking complete encoding round trips. It reads the
frozen repositories without modifying them and requires no services or builds.

Results, input hashes, script identity and warm Python timings are written to
`tests/.artifacts/binary-layouts/results.json`. Timings are prototype measurements,
not native Gutch performance claims. The script handles trusted experiment inputs;
it is not a production decoder or a substitute for malformed-input fuzzing.
