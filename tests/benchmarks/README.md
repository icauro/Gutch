# Gutch Benchmark Diagnostics

Run these commands from the Gutch project root.

`python gutch.py benchmark` lists internal Gutch fixtures. Use
`python gutch.py benchmark interactive` to select one, or pass `--fixture` with
`--product` / `--dataset` for a scripted run. These compare Gutch profiles,
capture modes, reconstruction paths, storage layouts, and a filesystem baseline.

Comparisons against other tools belong exclusively to the separate
[comparison suite](../../../comparison_benchmark_suite/README.md). Run its
`python run.py`; Gutch no longer downloads or uploads competitor binaries.

This directory retains advanced diagnostic fixtures and optional Docker/S3
workflows. They import the same command, validation, scheduling, and comparison
runtime from `comparison_benchmark_suite/`; there is no second implementation.
Correctness tests retain their independent helpers and input expectations.

Fixture lanes declare `tool` and `workload` separately. Only Gutch lanes accept
Gutch profiles. `profile_mode: "off"` is the default; `summary` adds coarse
profiling and `detailed` adds worker-job traces and external process-memory sampling. `repetitions` defaults to one and rotates
case order across samples. Warmup lanes execute once and are excluded from
measured results. OS caches are uncontrolled; these are fresh-repository runs,
not guaranteed cold-cache measurements.

During execution, each run records `run.json`, incremental `results.json`,
`summary.json`, command logs, raw stats and validation results. Successful runs
retain root summaries and the measured `trace.json`, then remove detailed lanes,
temporary repositories and tool caches. Result paths describe the original measurements;
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

Process timings exclude cooldown and independent correctness checks. Embedded
verification is already included in reconstruction time; repair and filesystem
full recovery are reported separately. Input inventories and fingerprints
identify each workload. OS cache state is uncontrolled, and detailed profiling
adds overhead. Use repeated samples on the same host and dataset.

Run Gutch benchmarks interactively:

    python gutch.py benchmark interactive

The interactive Gutch launcher uses `local.product_root` from the benchmark
YAML config as its product lookup root, then presents its product directories
as the next numbered selection.

List Gutch fixtures:

    python gutch.py benchmark list

Fixture filenames retain their numeric prefixes; removed external comparisons
leave gaps in the numbering.
Similar fixtures appear together, ordered from basic local measurements to
more involved workflows.

Use the complete numbered name with `--fixture`, for example
`--fixture 03_gutch_batch_vs_individual_capture --dataset empty_and_tiny`.

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
combines the retained lane traces into `trace.json` and opens it in Perfetto.
Sequential commands reuse neutral `Worker #N` rows across capture, reconstruction,
and verification, within each lane. Pools with the same worker number also share rows inside a command when their
recorded activity does not overlap; overlapping pools retain separate role names. Unnamed or duplicate-named threads retain separate
rows. Commands are concatenated; their internal timing and gaps are preserved.
Main-thread phases provide the overview; worker tracks retain measured jobs and
nested operations with separate thread identities for each command. Pool threads
use one-based names such as `Capture worker #1` and `Reconstruction worker #1`;
Perfetto appends a separate numeric trace thread ID. Gaps remain
unrecorded time, not inferred sleeping or CPU activity. Reconstruction jobs include
`queue_wait_us`, measured from submission to worker entry. `runtime.memory.wait`
measures capture admission waits on the caller; coordinator waits measure
receiving worker results. Reconstruction admits jobs before occupying workers.
The automatic working-memory budget is 25% of host RAM, bounded by a detected
Linux cgroup limit (512 MiB fallback if detection fails). CLI `--memory-budget-mib`
overrides it; `runtime.memory.budget` records the selected bytes. Memory reservations
remain estimates, not a hard process memory limit.
`installation.worker.resolve_output_paths` measures startup path validation in
batches of up to 64 files, with a `path_count` argument and manifest order preserved.
A bundle-build span encloses input loading and encoding so its children can be
inspected by zooming. No global phases are projected onto worker tracks.
Automatic opening uses a content-versioned local URL so changed traces do not
reuse an earlier browser import at the same path. The local trace server follows the same `127.0.0.1:9001` workflow as
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

Gutch lanes use typed `profile` options. Fixtures may set `snapshot_limit` to
select the newest N targets for individual capture and filesystem copy lanes.

## Source tree

    tests/benchmarks/
      comparison/       internal fixture launcher, binary selection, S3 cycles
        fixtures/       Gutch profiles, capture modes, storage and recovery cases
      scheduling/       benchmark-owned environment configuration
      support/          S3 transport and reconstruction diagnostics
      s3_throughput/    object-storage workload measurements
      docker/           Linux execution and evidence export

The installed `comparison_benchmark_suite` package supplies shared process,
validation, scheduling, Gutch operation, and reporting helpers. It owns the
external-tool adapters and comparison fixtures.

Each measured lane gets an isolated artifact directory. Reconstruct owns its single
verification measurement through the Gutch stats payload; corruption and
repair are one optional fixture-level phase that never issues another verify
command. Set corrupt_repair: true at the fixture root to run it for every
lane; omit it or set it to false to skip it. Scan, recount/inventory,
and independent output validation are always run.
Snapshots are captured chronologically and reconstructed newest-first by
default in every multi-snapshot lane.
Generated repositories, payloads, and traces live below tests/.artifacts/ and are
never part of the source tree.

## Process memory profiling

Use fixture `08_gutch_default_memory_with_corrupt_repair` for the default workload.
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

The comparison suite is a separate root project. Before using Gutch benchmark
commands, install its checkout into the same Python environment:
`uv pip install --python .venv -e <suite-checkout>`. Gutch does not search sibling folders.
Its build image includes the installed suite when available; build and engine
correctness commands do not require it.
