"""Stage a CLI binary and target-specific dependency notices for CI artifacts."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess


ROOT = Path(__file__).resolve().parents[1]
TARGETS = ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu",
           "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc")
SOURCE_NOTICES = (
    "LICENSE", "THIRD_PARTY_NOTICES.md", "src/artifact/fastcdc_v2020/LICENSE",
    "src/artifact/fastcdc_v2020/README.md",
    "src/artifact/jc.LICENSE", "src/artifact/ram/LICENSE", "src/artifact/ram/README.md",
)


def cli_dependencies(metadata):
    """Follow the CLI's enabled normal/build edges, excluding dev-only packages."""
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    cli = next(package["id"] for package in metadata["packages"]
               if package["name"] == "gutch-cli" and package["id"] in metadata["workspace_members"])
    visited = set()
    pending = [cli]
    while pending:
        identifier = pending.pop()
        if identifier in visited:
            continue
        visited.add(identifier)
        pending.extend(dependency["pkg"] for dependency in nodes[identifier]["deps"]
                       if any(kind["kind"] != "dev" for kind in dependency["dep_kinds"]))
    return [packages[identifier] for identifier in sorted(visited)
            if identifier not in metadata["workspace_members"]]


def stage_package(root, target, metadata, provenance, rust_notices):
    if target not in TARGETS:
        raise ValueError(f"Unsupported distribution target: {target}")
    binary_name = "gutch-cli.exe" if "windows" in target else "gutch-cli"
    binary = root / ".builds/compiler-cache/cargo" / target / "release" / binary_name
    destination = root / ".builds/releases" / target
    # Every package is rebuilt from scratch inside the canonical build directory.
    owner = (root / ".builds").resolve()
    if not destination.resolve().is_relative_to(owner):
        raise ValueError("Distribution directory escapes .builds")
    if destination.exists():
        shutil.rmtree(destination)
    destination.mkdir(parents=True)
    shutil.copy2(binary, destination / binary_name)
    for name in SOURCE_NOTICES:
        target_file = destination / name
        target_file.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(root / name, target_file)
    # The standard library is linked too; Cargo does not list its notices.
    rust_destination = destination / "licenses/rust"
    shutil.copytree(rust_notices / "licenses", rust_destination / "licenses")
    shutil.copyfile(rust_notices / "COPYRIGHT-library.html", rust_destination / "COPYRIGHT-library.html")
    dependencies = []
    for package in cli_dependencies(metadata):
        source = Path(package["manifest_path"]).parent
        licenses = [path for path in source.rglob("*") if path.is_file() and (
            path.name.lower().startswith(("license", "licence", "copying", "notice", "copyright", "unlicense"))
            or any(part.lower() == "licenses" for part in path.relative_to(source).parts[:-1])
        )]
        if package.get("license_file"):
            declared = source / package["license_file"]
            if not declared.resolve().is_relative_to(source.resolve()):
                raise ValueError(f"License path escapes package: {package['name']}")
            licenses.append(declared)
        if not licenses:
            raise ValueError(f"No license/notice files found for {package['name']} {package['version']}")
        relative = Path("licenses/dependencies") / f"{package['name']}-{package['version']}"
        for license_file in set(licenses):
            target_file = destination / relative / license_file.relative_to(source)
            target_file.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(license_file, target_file)
        dependencies.append({"name": package["name"], "version": package["version"],
                             "license": package.get("license"), "notices": relative.as_posix()})
    (destination / "dependencies.json").write_text(json.dumps(dependencies, indent=2) + "\n", encoding="utf-8")
    record = {**provenance, "target": target, "binary": binary_name,
              "sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}
    (destination / "build.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    (destination / "SHA256SUMS").write_text(f"{record['sha256']}  {binary_name}\n", encoding="utf-8")
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--toolchain", required=True)
    args = parser.parse_args()
    metadata = json.loads(subprocess.check_output(
        ["cargo", f"+{args.toolchain}", "metadata", "--locked", "--format-version", "1",
         "--filter-platform", args.target], cwd=ROOT, text=True,
    ))
    provenance = {
        "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "compiler": subprocess.check_output(["rustc", f"+{args.toolchain}", "--version"], text=True).strip(),
    }
    sysroot = Path(subprocess.check_output(
        ["rustc", f"+{args.toolchain}", "--print", "sysroot"], text=True,
    ).strip())
    print(stage_package(ROOT, args.target, metadata, provenance, sysroot / "share/doc/rust"))


if __name__ == "__main__":
    main()
