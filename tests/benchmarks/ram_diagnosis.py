"""Compare pinned C++ RAM/VRAM kernels with Gutch; requires an AVX2 CPU, g++, and Cargo."""

import argparse
import json
import os
import platform
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--product", type=Path, required=True)
    args = parser.parse_args()
    if platform.machine().lower() not in {"x86_64", "amd64"}:
        # The C++ reference uses AVX2 intrinsics directly.
        parser.error("RAM diagnosis needs an x86-64 machine with AVX2")
    product = args.product.resolve()
    output = ROOT / ".artifacts/tests/vram-diagnosis"
    output.mkdir(parents=True, exist_ok=True)
    paths = sorted(
        str(f)
        for build in product.iterdir()
        if build.is_dir()
        for f in build.rglob("*")
        if f.is_file()
    )
    if not paths:
        parser.error("Product must contain populated build directories")
    (output / "files.json").write_text(json.dumps(paths), encoding="utf-8")
    (output / "files.txt").write_text("\n".join(paths), encoding="utf-8")
    executable = output / ("reference.exe" if os.name == "nt" else "reference")
    flags = ["-O3", "-mavx2", "-std=c++17"]
    if os.name == "nt":
        flags += ["-static-libgcc", "-static-libstdc++"]
    with (output / "cpp-build.log").open("w", encoding="utf-8") as log:
        subprocess.run(
            [
                "g++",
                *flags,
                str(Path(__file__).with_name("ram_reference.cpp")),
                "-o",
                str(executable),
            ],
            check=True,
            stdout=log,
            stderr=log,
        )
    with (output / "rust-build.log").open("w", encoding="utf-8") as log:
        build = subprocess.check_output(
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
            stderr=log,
            text=True, encoding="utf-8",
        )
    items = [json.loads(line) for line in build.splitlines() if line.startswith("{")]
    rust_executable = next(
        item["executable"]
        for item in items
        if item.get("executable") and item.get("profile", {}).get("test")
    )
    (output / "metadata.json").write_text(
        json.dumps(
            {
                "product": str(product),
                "files": len(paths),
                "warmups": 1,
                "repetitions": 7,
                "upstream": "8e2697cbf6332ac5da6dc615bfab82a720e820e4",
                "compiler_flags": flags,
                "g++": subprocess.check_output(
                    ["g++", "--version"], text=True, encoding="utf-8"
                ).splitlines()[0],
                "rustc": subprocess.check_output(
                    ["rustc", "--version"], text=True, encoding="utf-8"
                ).strip(),
                "platform": platform.platform(),
                "scope": "single worker; input loaded before timing",
            },
            indent=2,
        ),
        encoding="utf-8",
    )
    with (output / "cpp-run.log").open("w", encoding="utf-8") as log:
        subprocess.run(
            [str(executable), str(output / "files.txt"), str(output / "cpp.json")],
            check=True,
            stdout=log,
            stderr=log,
        )
    print("C++ reference complete", flush=True)
    env = {
        **os.environ,
        "GUTCH_RAM_INPUTS": str(output / "files.json"),
        "GUTCH_RAM_OUTPUT": str(output / "rust.json"),
    }
    with (output / "rust-run.log").open("w", encoding="utf-8") as log:
        subprocess.run(
            [
                rust_executable,
                "artifact::ram_diagnosis::compare_ram_kernels",
                "--exact",
                "--ignored",
                "--nocapture",
            ],
            cwd=ROOT,
            env=env,
            check=True,
            stdout=log,
            stderr=log,
        )
    cpp = json.loads((output / "cpp.json").read_text(encoding="utf-8"))
    rust = json.loads((output / "rust.json").read_text(encoding="utf-8"))
    for window in (8192, 131072):
        selected = [
            r for r in cpp if r["window"] == window and r["variant"] != "reference-avx2"
        ]
        selected += [
            r
            for r in rust
            if r["window"] == window
            and r["mode"] == "boundary"
            and r["variant"] != "fastcdc"
        ]
        assert len({(r["chunks"], r["boundary_fingerprint"]) for r in selected}) == 1
    print(f"Results and boundary cross-check passed: {output}")


if __name__ == "__main__":
    main()
