# Gutch test artifacts

| Directory | Contents |
|---|---|
| `runs/gutch/` | Correctness runs; start with `summary.md` and `run.json` |
| `runs/benchmarks/` | Benchmark results, traces, timings, and retained evidence |
| `datasets/products/` | Generated datasets; committed corpus cases stay in `tests/datasets/corpus/` |
| `cache/` | Disposable Python tooling caches |

Successful correctness runs retain only `run.json` and `summary.md`; the newest
20 unpinned successful runs are kept. Successful benchmarks retain aggregate
results and traces, removing lane payloads, logs, caches and owned S3 prefixes.

Failed runs and runs using `--keep-artifacts` retain detailed evidence, including
command logs, inventories and payloads. Use `python gutch.py replay <case-directory>`
with a retained correctness case to reproduce it in a fresh run. S3 services are
disposable and removed after the suite. Unmanaged datasets are not swept automatically.
