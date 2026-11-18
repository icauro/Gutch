"""Write the license notices a CLI release embeds, and stage the built executable.

`notices` runs before the release build: it writes one text file with Gutch's
own licenses, the notices of every dependency linked into the CLI for the
target, and the Rust standard library's. The build embeds that file (see
gutch-cli/build.rs) and `gutch-cli licenses` prints it. `package` runs after
the build and stages the executable with its provenance and checksum.
"""

import argparse
from html.parser import HTMLParser
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess


ROOT = Path(__file__).resolve().parents[1]
TARGETS = ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu",
           "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc")
SOURCE_NOTICES = (
    "LICENSE", "THIRD_PARTY_NOTICES.md", "src/04_artifact/fastcdc_v2020/LICENSE",
    "src/04_artifact/fastcdc_v2020/README.md",
    "src/04_artifact/jc.LICENSE", "src/04_artifact/ram/LICENSE", "src/04_artifact/ram/README.md",
)
RULE = "=" * 78


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


def license_files(package):
    """A dependency's license, notice and copyright files, including vendored native ones."""
    source = Path(package["manifest_path"]).parent
    found = {path for path in source.rglob("*") if path.is_file() and (
        path.name.lower().startswith(("license", "licence", "copying", "notice", "copyright", "unlicense"))
        or any(part.lower() == "licenses" for part in path.relative_to(source).parts[:-1])
    )}
    if package.get("license_file"):
        declared = source / package["license_file"]
        if not declared.resolve().is_relative_to(source.resolve()):
            raise ValueError(f"License path escapes package: {package['name']}")
        found.add(declared)
    if not found:
        raise ValueError(f"No license/notice files found for {package['name']} {package['version']}")
    return sorted((path.relative_to(source).as_posix(), path) for path in found)


class _Text(HTMLParser):
    def __init__(self):
        super().__init__()
        self.parts = []

    def handle_data(self, data):
        self.parts.append(data)


def html_text(markup):
    parser = _Text()
    parser.feed(markup)
    return re.sub(r"\n\s*\n+", "\n\n", "".join(parser.parts))


def section(title, body):
    return f"{RULE}\n{title}\n{RULE}\n\n{body.strip()}\n"


def notices_text(root, target, metadata, rust_notices):
    """All license notices for the CLI built for `target`, with identical dependency texts printed once."""
    if target not in TARGETS:
        raise ValueError(f"Unsupported distribution target: {target}")
    sections = [section(f"Gutch: {name}", (root / name).read_text(encoding="utf-8")) for name in SOURCE_NOTICES]
    # The standard library is linked too; Cargo does not list its notices.
    for path in sorted((rust_notices / "licenses").iterdir()):
        sections.append(section(f"Rust standard library: {path.name}", path.read_text(encoding="utf-8")))
    sections.append(section("Rust standard library: copyright notices",
                            html_text((rust_notices / "COPYRIGHT-library.html").read_text(encoding="utf-8"))))
    users = {}
    for package in cli_dependencies(metadata):
        for relative, path in license_files(package):
            text = path.read_text(encoding="utf-8", errors="replace").strip()
            users.setdefault(text, []).append(
                f"{package['name']} {package['version']} ({package.get('license') or 'see text'}): {relative}")
    for text, names in users.items():
        sections.append(section("Dependency notice used by:\n  " + "\n  ".join(names), text))
    header = (f"Licenses and notices for gutch-cli ({target})\n\n"
              "gutch-cli is MIT licensed. It includes adapted third-party source and links the\n"
              "libraries below, each under its own terms.\n\n")
    return header + "\n".join(sections)


def stage_package(root, target, metadata, provenance):
    """Copy the built executable with its provenance and SHA-256 checksum into `.builds/releases/<target>`."""
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
    dependencies = [{"name": package["name"], "version": package["version"], "license": package.get("license")}
                    for package in cli_dependencies(metadata)]
    record = {**provenance, "target": target, "binary": binary_name,
              "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "dependencies": dependencies}
    (destination / "build.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    (destination / "SHA256SUMS").write_text(f"{record['sha256']}  {binary_name}\n", encoding="utf-8")
    return destination


def cargo_metadata(toolchain, target):
    return json.loads(subprocess.check_output(
        ["cargo", f"+{toolchain}", "metadata", "--locked", "--format-version", "1", "--filter-platform", target],
        cwd=ROOT, text=True, encoding="utf-8",
    ))


def rustc(toolchain, *args):
    return subprocess.check_output(["rustc", f"+{toolchain}", *args], text=True, encoding="utf-8").strip()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    for name, text in (("notices", "write the notices file the build embeds"),
                       ("package", "stage the built executable")):
        command = commands.add_parser(name, help=text)
        command.add_argument("--target", choices=TARGETS, required=True)
        command.add_argument("--toolchain", required=True)
        if name == "notices":
            command.add_argument("--out", type=Path, help="default: .builds/notices/<target>.txt")
    args = parser.parse_args(argv)
    metadata = cargo_metadata(args.toolchain, args.target)
    if args.command == "notices":
        out = args.out or ROOT / ".builds/notices" / f"{args.target}.txt"
        rust_notices = Path(rustc(args.toolchain, "--print", "sysroot")) / "share/doc/rust"
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(notices_text(ROOT, args.target, metadata, rust_notices), encoding="utf-8", newline="\n")
        print(out.resolve())
    else:
        provenance = {
            "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True,
                                              encoding="utf-8").strip(),
            "compiler": rustc(args.toolchain, "--version"),
        }
        print(stage_package(ROOT, args.target, metadata, provenance))


if __name__ == "__main__":
    main()
