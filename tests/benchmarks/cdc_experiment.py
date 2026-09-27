"""Compare test-only JC variants with FastCDC; requires two local input builds."""
import argparse
import json
import hashlib
import os
import platform
import re
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", type=Path, required=True)
    parser.add_argument("--updated", type=Path, required=True)
    parser.add_argument("--name", default="input-builds", help="Dataset label and evidence subdirectory")
    parser.add_argument("--repetitions", type=int, default=5)
    args = parser.parse_args()
    if args.repetitions < 3 or not args.base.is_dir() or not args.updated.is_dir():
        parser.error("Provide two input directories and at least three repetitions")
    if not re.fullmatch(r"[A-Za-z0-9_-]+", args.name) or args.name in {"random-edits", "repeated"}:
        parser.error("Use a dataset name with letters, digits, underscores or hyphens; synthetic names are reserved")
    output = ROOT / "tests/.artifacts/jc-experiment" / args.name
    output.mkdir(parents=True, exist_ok=True)
    with (output / "build.log").open("w", encoding="utf-8") as log:
        built = subprocess.run(["cargo", "test", "--locked", "--release", "--lib", "--no-run", "--message-format=json"],
                               cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=log, check=True)
    artifacts = [json.loads(line) for line in built.stdout.splitlines() if line.startswith("{")]
    executable = next(item["executable"] for item in artifacts if item.get("executable") and item.get("profile", {}).get("test"))
    environment = {**os.environ, "GUTCH_CDC_LABEL": args.name, "GUTCH_CDC_BASE": str(args.base.resolve()),
                   "GUTCH_CDC_UPDATED": str(args.updated.resolve()), "GUTCH_CDC_REPETITIONS": str(args.repetitions),
                   "GUTCH_CDC_OUTPUT": str(output / "results.json")}
    source_files = list((ROOT / "src/artifact").rglob("*.rs")) + [Path(__file__).with_suffix(".rs"), Path(__file__).with_name("cdc_baseline.rs")]
    source_hash = hashlib.sha256()
    for path in sorted(source_files):
        source_hash.update(path.relative_to(ROOT).as_posix().encode())
        source_hash.update(path.read_bytes())
    metadata = {"dataset": args.name, "source_sha256": source_hash.hexdigest(), "plakar_commit": "b6c4abce19e9691184a0aceefc4e9f53fcfe79c8","base": str(args.base.resolve()), "updated": str(args.updated.resolve()),
                "repetitions": args.repetitions, "warmups": 1, "platform": platform.platform(),
                "processor": platform.processor(), "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
                "scope": "single-thread release; base resident during timings; updated files read individually in untimed reuse analysis; medians of rotating variant order"}
    (output / "metadata.json").write_text(json.dumps(metadata, indent=2), encoding="utf-8")
    with (output / "benchmark.log").open("w", encoding="utf-8") as log:
        subprocess.run([executable, "artifact::cdc_experiment::compare_chunkers", "--exact", "--ignored", "--nocapture"],
                       cwd=ROOT, env=environment, stdout=log, stderr=subprocess.STDOUT, check=True)
    print(f"Results: {output / 'results.json'}")


if __name__ == "__main__":
    main()
