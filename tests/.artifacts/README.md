# Gutch test artifacts

| Directory | Contents |
|---|---|
| `runs/gutch/` | Correctness runs; start with `summary.md` and `run.json` |
| `runs/benchmarks/` | Benchmark results, traces, timings, and retained evidence |
| `fixtures/products/` | Generated datasets; checked-in reference fixtures stay in `tests/datasets/` |
| `cache/` | Disposable Python tooling caches |

Each correctness case contains numbered command logs, expected inventories,
comparison results, and timing metadata. Failed cases retain their local
payloads. S3 services are disposable and removed after the suite.
