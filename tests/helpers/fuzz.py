"""Seed and run bounded native Rust fuzz targets through cargo-bolero."""

import argparse
import hashlib
import shutil
import time

from tests.datasets.generate import CORPUS_ROOT
from tests.helpers.cli import execute, REPOSITORY

TARGETS = ("binary_decoders", "bundle_roundtrip", "blueprint_roundtrip")
# Must match the input bound in src/tests/engine_fuzz.rs. It admits the
# corpus's smaller bundles so their decoders start from real framing.
MAX_INPUT_BYTES = 131072


def seed_inputs():
    """Yield frozen repository files small enough to seed the decoder target."""
    for pattern in ("*.gtcbin", "bundles/**/*"):
        for path in sorted(CORPUS_ROOT.glob(f"*/repository/format-v1/**/{pattern}")):
            if path.is_file() and path.stat().st_size <= MAX_INPUT_BYTES:
                yield path


def seed_corpus(target, corpus):
    corpus.mkdir(parents=True, exist_ok=True)
    if target == "binary_decoders":
        for path in seed_inputs():
            data = path.read_bytes()
            (corpus / hashlib.sha256(data).hexdigest()).write_bytes(data)


def fuzz_command(target, root, seconds, seed=None, rustc_bootstrap=False):
    arguments = [
        "cargo",
        "bolero",
        "test",
        f"engine_fuzz::fuzz_{target}",
        "-T",
        f"{seconds}s",
        "-l",
        str(MAX_INPUT_BYTES),
        "--target-dir",
        str(REPOSITORY / ".builds/compiler-cache/fuzz"),
        "--corpus-dir",
        str(root / "corpus"),
        "--crashes-dir",
        str(root / "crashes"),
    ]
    if seed is not None:
        arguments += ["-S", str(seed)]
    if rustc_bootstrap:
        arguments += ["--rustc-bootstrap"]
    return arguments


def main(arguments=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=(*TARGETS, "all"), default="all")
    parser.add_argument("--seconds", type=int, default=30)
    parser.add_argument("--seed", type=int)
    parser.add_argument(
        "--rustc-bootstrap",
        action="store_true",
        help="Enable compiler instrumentation on the installed stable toolchain",
    )
    args = parser.parse_args(arguments)
    if args.seconds < 1:
        parser.error("seconds must be positive")
    if shutil.which("cargo-bolero") is None:
        raise SystemExit(
            "cargo-bolero is not installed; run "
            "`cargo install cargo-bolero --locked --no-default-features` first"
        )
    for target in TARGETS if args.target == "all" else (args.target,):
        root = REPOSITORY / ".artifacts/tests/fuzz" / target
        seed_corpus(target, root / "corpus")
        log = root / f"run-{time.time_ns()}.log"
        print(
            f"Fuzzing {target} for {args.seconds}s after compilation; log: {log}",
            flush=True,
        )
        execute(
            fuzz_command(target, root, args.seconds, args.seed, args.rustc_bootstrap),
            REPOSITORY,
            {},
            log,
            timeout=900 + args.seconds,
        )
        print(f"Passed {target}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
