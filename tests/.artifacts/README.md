# Gutch test artifacts

| Directory | Contents |
|---|---|
| `runs/gutch/` | Correctness runs; start with `summary.md` and `run.json` |
| `runs/benchmarks/` | Benchmark results, traces, timings, and retained evidence |
| `datasets/products/` | Generated datasets; checked-in reference fixtures stay in `tests/fixtures/repositories/` |
| `cache/` | Disposable Python tooling caches |

Each correctness case includes `replay.json`; run `python gutch.py replay <case-directory>` to reproduce it in a fresh run. Each correctness case contains numbered command logs, expected inventories,
comparison results, and timing metadata. Failed cases retain their local
payloads. S3 services are disposable and removed after the suite.
