# Folders and outputs

| Folder | Holds |
| --- | --- |
| `_dev_guide/` | This guide: how to work on Gutch, and the engine internals |
| `_docs/` | The user guide: how to use Gutch |
| `_tools/` | Maintained developer scripts |
| `.artifacts/` | Generated test, benchmark, tool and experiment output |
| `.builds/` | Compiler caches and build output |

Run every command from the Gutch root. No parent workspace or sibling
repository is needed. `.artifacts/` and `.builds/` are ignored and fully
disposable, including pinned or failed runs; copy evidence you need later
elsewhere before cleaning.

## Generated output

| Path | Contents |
| --- | --- |
| `.artifacts/tests/runs/gutch/` | Correctness runs; start with `summary.md` and `run.json` |
| `.artifacts/tests/runs/benchmarks/` | Benchmark results, traces, timings and retained evidence |
| `.artifacts/tests/datasets/products/` | Generated datasets; the committed corpus stays in `tests/datasets/corpus/` |
| `.artifacts/tests/cache/` | Disposable Python tooling caches |
| `.artifacts/tools/` | Tool output |
| `.artifacts/scratch/` | Experiments |
| `.builds/compiler-cache/cargo/` | Cargo library, CLI and test compilation output |
| `.builds/intermediate/` | Temporary source-only Linux build contexts, removed after each build |

Direct pytest calls take an explicit `--basetemp=.artifacts/tests/<unique-run>`,
so temporary data stays inside the project. The [test guide](../tests/README.md)
describes which evidence each run keeps. Successful benchmarks keep aggregate
results and traces and remove lane payloads, logs, caches and owned S3 prefixes.
The Python build backend may create ignored `build/` and `src/*.egg-info/`
metadata; cleanup leaves them alone.

## Cleanup

Run `python _tools/cleanup/cleanup_artifacts.py --dry-run` or
`python _tools/cleanup/cleanup_builds.py --dry-run` to inspect targets. Remove
`--dry-run` to delete them. Default behavior includes declared children;
`--current-only` stays at this folder. Cleanup never stops applications.
Pinning a run controls automatic retention only, not this explicit cleanup.
