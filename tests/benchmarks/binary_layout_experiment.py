"""Measure production Rust metadata codecs against the frozen corpus (warm reads)."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import statistics
import time
import psutil

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repetitions", type=int, default=7)
    args = parser.parse_args()
    if not 1 <= args.repetitions <= 100:
        parser.error("repetitions must be between 1 and 100")
    environment = {**os.environ, "GUTCH_LAYOUT_REPETITIONS": str(args.repetitions)}
    built = subprocess.run(
        [
            "cargo",
            "test",
            "--locked",
            "--release",
            "--lib",
            "--no-run",
            "--message-format=json",
        ],
        cwd=ROOT,
        env=environment,
        text=True,
        stdout=subprocess.PIPE,
        check=True,
    )
    artifacts = [
        json.loads(line) for line in built.stdout.splitlines() if line.startswith("{")
    ]
    executable = next(
        item["executable"]
        for item in artifacts
        if item.get("reason") == "compiler-artifact"
        and item.get("executable")
        and item.get("profile", {}).get("test")
    )
    output = ROOT / ".artifacts/tests/binary-layouts"
    output.mkdir(parents=True, exist_ok=True)
    with (output / "native.log").open("w", encoding="utf-8") as log:
        process = subprocess.Popen(
            [
                executable,
                "manifest::binary_layout_benchmark=native_corpus_layout_comparison",
                "--exact",
                "--nocapture",
            ],
            cwd=ROOT,
            env=environment,
            stdout=log,
            stderr=subprocess.STDOUT,
        )
        measured = psutil.Process(process.pid)
        peak = 0
        while process.poll() is None:
            try:
                peak = max(peak, measured.memory_info().rss)
            except psutil.NoSuchProcess:
                break
            time.sleep(0.01)
        if process.wait():
            raise RuntimeError(f"Native experiment failed: {output / 'native.log'}")
    summary = {
        "peak_process_rss_bytes": peak,
        "sampling_interval_ms": 10,
        "repetitions": args.repetitions,
        "scope": "release test process; warm corpus reads; RSS includes test harness and all materialized comparison values; compilation excluded",
    }
    (output / "memory.json").write_text(
        json.dumps(summary, indent=2) + "\n", encoding="utf-8"
    )
    io_results = []
    for path in sorted(
        (ROOT / "tests/datasets/corpus").glob("*/repository/format-v1/**/*.gtcbin")
    ):

        def read_sample(discard=False):
            with path.open("rb") as stream:
                if discard:
                    os.posix_fadvise(stream.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)
                start = time.perf_counter_ns()
                content = stream.read()
                elapsed = (time.perf_counter_ns() - start) / 1000
                assert len(content) == path.stat().st_size
                return elapsed

        read_sample()
        warm = statistics.median(read_sample() for _ in range(args.repetitions))
        discarded = None
        if hasattr(os, "posix_fadvise"):
            discarded = statistics.median(
                read_sample(True) for _ in range(args.repetitions)
            )
        io_results.append(
            {
                "path": path.relative_to(ROOT).as_posix(),
                "warm_read_median_us": warm,
                "eviction_requested_read_median_us": discarded,
            }
        )
    (output / "io.json").write_text(
        json.dumps(
            {
                "scope": "file reads only; Linux requests page-cache eviction with POSIX_FADV_DONTNEED; hardware caches are uncontrolled",
                "files": io_results,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    print(
        f"Native layout results: {output / 'native.json'}; sampled peak RSS: {peak / 1048576:.2f} MiB"
    )


if __name__ == "__main__":
    main()
