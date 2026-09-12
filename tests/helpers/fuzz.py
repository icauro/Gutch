"""Seed and run bounded native Rust fuzz targets through cargo-bolero."""
import argparse
import hashlib
import time
from pathlib import Path

from tests.datasets.generate import CORPUS_ROOT
from tests.helpers.cli import execute, REPOSITORY


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--filesystem", choices=("binary_decoders", "bundle_roundtrip", "blueprint_roundtrip", "all"), default="all")
    parser.add_argument("--seconds", type=int, default=30)
    parser.add_argument("--seed", type=int)
    parser.add_argument("--rustc-bootstrap", action="store_true", help="Enable compiler instrumentation on the installed stable toolchain")
    args = parser.parse_args()
    if args.seconds < 1:
        parser.error("seconds must be positive")
    targets = ("binary_decoders", "bundle_roundtrip", "blueprint_roundtrip") if args.target == "all" else (args.target,)
    for target in targets:
        root = REPOSITORY / "tests/.artifacts/fuzz" / target
        corpus = root / "corpus"
        corpus.mkdir(parents=True, exist_ok=True)
        if target == "binary_decoders":
            for path in CORPUS_ROOT.glob("*/repository/format-v1/**/*.gtcbin"):
                if path.stat().st_size <= 65536:
                    data = path.read_bytes()
                    (corpus / hashlib.sha256(data).hexdigest()).write_bytes(data)
        arguments = ["cargo", "bolero", "test", f"engine_fuzz::fuzz_{target}",
                     "-T", f"{args.seconds}s", "-l", "65536",
                     "--target-dir", str(REPOSITORY / ".builds/compiler-cache/fuzz"),
                     "--corpus-dir", str(corpus), "--crashes-dir", str(root / "crashes")]
        if args.seed is not None:
            arguments += ["-S", str(args.seed)]
        if args.rustc_bootstrap:
            arguments += ["--rustc-bootstrap"]
        log = root / f"run-{time.time_ns()}.log"
        print(f"Fuzzing {target} for {args.seconds}s after compilation; log: {log}", flush=True)
        try:
            execute(arguments, REPOSITORY, {}, log, timeout=900 + args.seconds)
        except FileNotFoundError as error:
            raise RuntimeError("Install Rust and cargo-bolero before running native fuzzing") from error
        print(f"Passed {target}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
