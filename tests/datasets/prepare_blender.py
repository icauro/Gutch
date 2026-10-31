"""Download and extract pinned Blender inputs; never rebuild frozen repositories."""

from __future__ import annotations
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import tarfile
import urllib.request
import zipfile
from dataclasses import dataclass
from .generate import _write_json, _remove_tree

BLENDER_PRODUCT_NAME = "Blender"
BLENDER_MANIFEST_FILE_NAME = ".blender-test-manifest.json"
BLENDER_SOURCE_CONFIG_FILE_NAME = ".gutch_source_config.json"
BLENDER_DEFAULT_PLATFORMS = ("linux-x64", "windows-x64")
BLENDER_SUPPORTED_PLATFORMS = ("linux-x64", "windows-x64")
BLENDER_ARCHIVE_SUFFIXES = {"linux-x64": ".tar.xz", "windows-x64": ".zip"}
BLENDER_VERSION_RE = re.compile(r"^\d+\.\d+\.\d+$")


@dataclass(frozen=True)
class BlenderAsset:
    version: str
    platform: str
    archive_name: str
    sha256: str

    @property
    def folder_name(self) -> str:
        for suffix in (".tar.xz", ".zip"):
            if self.archive_name.endswith(suffix):
                return self.archive_name[: -len(suffix)]
        raise ValueError(f"unsupported Blender archive name: {self.archive_name}")


@dataclass(frozen=True)
class BlenderPreparationResult:
    product_dir: Path
    target_dirs: tuple[Path, ...]


def _blender_archive_parts(name: str) -> tuple[str, ...]:
    normalized = name.replace("\\", "/")
    if (
        not normalized
        or normalized.startswith("/")
        or normalized.startswith("\\")
        or re.match(r"^[A-Za-z]:", normalized)
        or "\x00" in normalized
    ):
        raise RuntimeError(f"refusing unsafe archive entry: {name!r}")
    parts = tuple(part for part in normalized.split("/") if part)
    if not parts or any(part in {".", ".."} for part in parts):
        raise RuntimeError(f"refusing unsafe archive entry: {name!r}")
    return parts


def _extract_blender_archive_safely(archive_path: Path, target_root: Path) -> None:
    """Extract one official Blender archive into a new, contained directory."""
    target_root = target_root.resolve()
    target_root.mkdir(parents=True, exist_ok=True)
    if archive_path.name.endswith(".zip"):
        with zipfile.ZipFile(archive_path) as archive:
            for info in archive.infolist():
                parts = _blender_archive_parts(info.filename)
                destination = target_root.joinpath(*parts)
                if not destination.resolve().is_relative_to(target_root):
                    raise RuntimeError(
                        f"refusing archive entry escaping root: {info.filename!r}"
                    )
                if info.is_dir():
                    destination.mkdir(parents=True, exist_ok=True)
                    continue
                mode = (info.external_attr >> 16) & 0o170000
                if mode == 0o120000:
                    raise RuntimeError(
                        f"refusing symlink archive entry: {info.filename!r}"
                    )
                destination.parent.mkdir(parents=True, exist_ok=True)
                with archive.open(info) as source, destination.open("wb") as output:
                    shutil.copyfileobj(source, output)
                permissions = (info.external_attr >> 16) & 0o777
                if permissions:
                    destination.chmod(permissions)
        return

    if not archive_path.name.endswith(".tar.xz"):
        raise ValueError(f"unsupported Blender archive: {archive_path.name}")
    symlinks: list[tuple[Path, str, Path]] = []
    symlink_targets: dict[Path, Path] = {}
    with tarfile.open(archive_path, mode="r:xz") as archive:
        for member in archive.getmembers():
            parts = _blender_archive_parts(member.name)
            destination = target_root.joinpath(*parts)
            if not destination.resolve().is_relative_to(target_root):
                raise RuntimeError(
                    f"refusing archive entry escaping root: {member.name!r}"
                )
            if member.isdir():
                destination.mkdir(parents=True, exist_ok=True)
                continue
            if member.issym():
                link_name = member.linkname.replace("\\", "/")
                if (
                    not link_name
                    or link_name.startswith("/")
                    or re.match(r"^[A-Za-z]:", link_name)
                    or "\x00" in link_name
                ):
                    raise RuntimeError(
                        f"refusing unsafe archive symlink: {member.name!r}"
                    )
                link_target = (destination.parent / link_name).resolve(strict=False)
                if not link_target.is_relative_to(target_root):
                    raise RuntimeError(
                        f"refusing archive symlink escaping root: {member.name!r}"
                    )
                symlinks.append((destination, link_name, link_target))
                symlink_targets[destination] = link_target
                continue
            if not member.isfile():
                raise RuntimeError(f"refusing non-file archive entry: {member.name!r}")
            source = archive.extractfile(member)
            if source is None:
                raise RuntimeError(f"archive member is not readable: {member.name!r}")
            destination.parent.mkdir(parents=True, exist_ok=True)
            with source, destination.open("wb") as output:
                shutil.copyfileobj(source, output)
            destination.chmod(member.mode & 0o777)

    def resolve_materialized_target(path: Path) -> Path:
        seen: set[Path] = set()
        current = path
        while current in symlink_targets:
            if current in seen:
                raise RuntimeError(
                    f"cannot materialize cyclic Blender symlink: {current!s}"
                )
            seen.add(current)
            current = symlink_targets[current]
        return current

    for destination, link_name, link_target in symlinks:
        if os.path.lexists(destination):
            raise RuntimeError(f"duplicate archive entry: {destination.name!r}")
        try:
            destination.symlink_to(link_name)
        except (NotImplementedError, OSError):
            materialized_target = resolve_materialized_target(link_target)
            if not materialized_target.is_file():
                raise RuntimeError(
                    f"cannot materialize Blender symlink target: {link_name!r}"
                )
            shutil.copy2(materialized_target, destination)


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def _download_blender_asset(
    asset: BlenderAsset, destination: Path, base_url: str
) -> None:
    major, minor, _ = asset.version.split(".")
    series_url = f"{base_url.rstrip('/')}/Blender{major}.{minor}/{asset.archive_name}"
    temporary_path = destination.with_name(f".{destination.name}.{os.getpid()}.part")
    temporary_path.unlink(missing_ok=True)
    request = urllib.request.Request(
        series_url,
        headers={"User-Agent": "gutch-test-data/1"},
    )
    try:
        with (
            urllib.request.urlopen(request, timeout=120) as response,
            temporary_path.open("wb") as output,
        ):
            while chunk := response.read(1024 * 1024):
                output.write(chunk)
        if _sha256_file(temporary_path) != asset.sha256:
            raise RuntimeError(
                f"SHA-256 mismatch for {asset.archive_name}; expected {asset.sha256}"
            )
        temporary_path.replace(destination)
    finally:
        temporary_path.unlink(missing_ok=True)


def _load_blender_assets(recipe_path: Path) -> tuple[str, list[BlenderAsset]]:
    try:
        recipe = json.loads(recipe_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        raise ValueError(f"invalid Blender recipe {recipe_path}: {error}") from error
    if not isinstance(recipe, dict) or recipe.get("schema_version") != 1:
        raise ValueError(f"unsupported Blender recipe schema: {recipe_path}")
    if "source" in recipe:
        recipe = recipe["source"]
    base_url = recipe.get("base_url")
    if not isinstance(base_url, str) or not base_url.strip():
        raise ValueError(
            f"Blender recipe base_url must be a non-empty string: {recipe_path}"
        )
    versions = recipe.get("versions")
    if not isinstance(versions, list) or not versions:
        raise ValueError(f"Blender recipe must define versions: {recipe_path}")

    assets: list[BlenderAsset] = []
    for version_entry in versions:
        if not isinstance(version_entry, dict):
            raise ValueError(
                f"Blender recipe version entries must be objects: {recipe_path}"
            )
        version = version_entry.get("version")
        raw_assets = version_entry.get("assets")
        if not isinstance(version, str) or not BLENDER_VERSION_RE.fullmatch(version):
            raise ValueError(f"invalid Blender version in recipe: {version!r}")
        if not isinstance(raw_assets, list) or not raw_assets:
            raise ValueError(f"Blender version {version} has no assets")
        for raw_asset in raw_assets:
            if not isinstance(raw_asset, dict):
                raise ValueError(
                    f"Blender asset entries must be objects: {recipe_path}"
                )
            platform = raw_asset.get("platform")
            archive_name = raw_asset.get("archive")
            sha256 = raw_asset.get("sha256")
            expected_suffix = BLENDER_ARCHIVE_SUFFIXES.get(str(platform))
            expected_name = f"blender-{version}-{platform}{expected_suffix or ''}"
            if platform not in BLENDER_SUPPORTED_PLATFORMS:
                raise ValueError(
                    f"unsupported Blender platform in recipe: {platform!r}"
                )
            if archive_name != expected_name:
                raise ValueError(
                    f"Blender archive must be {expected_name!r}, got {archive_name!r}"
                )
            if not isinstance(sha256, str) or not re.fullmatch(
                r"[0-9a-fA-F]{64}", sha256
            ):
                raise ValueError(
                    f"Blender asset {archive_name} must have a SHA-256 digest"
                )
            assets.append(BlenderAsset(version, platform, archive_name, sha256.lower()))
    return base_url.rstrip("/"), assets


def _select_blender_assets(
    assets: list[BlenderAsset],
    versions: list[str] | None,
    platforms: list[str] | None,
) -> list[BlenderAsset]:
    available_versions = {asset.version for asset in assets}
    selected_versions = versions or sorted(
        available_versions,
        key=lambda value: tuple(int(part) for part in value.split(".")),
    )
    unknown_versions = sorted(set(selected_versions) - available_versions)
    if unknown_versions:
        raise ValueError(
            f"Blender recipe has no pinned versions: {', '.join(unknown_versions)}"
        )
    selected_platforms = platforms or list(BLENDER_DEFAULT_PLATFORMS)
    unknown_platforms = sorted(
        set(selected_platforms) - set(BLENDER_SUPPORTED_PLATFORMS)
    )
    if unknown_platforms:
        raise ValueError(
            f"unsupported Blender platforms: {', '.join(unknown_platforms)}"
        )
    selected = [
        asset
        for asset in assets
        if asset.version in selected_versions and asset.platform in selected_platforms
    ]
    missing = [
        f"{version}/{platform}"
        for version in selected_versions
        for platform in selected_platforms
        if not any(
            asset.version == version and asset.platform == platform
            for asset in selected
        )
    ]
    if missing:
        raise ValueError(f"Blender recipe has no pinned assets: {', '.join(missing)}")
    return selected


def _blender_folder_tags(folder_name: str) -> dict[str, str] | None:
    match = re.fullmatch(
        r"blender-(\d+\.\d+\.\d+)-(linux-x64|windows-x64)",
        folder_name,
    )
    if match is None:
        return None
    version, platform = match.groups()
    operating_system = "linux" if platform == "linux-x64" else "windows"
    return {"version": version, "os": operating_system, "arch": "x86_64"}


def _write_blender_source_config(
    product_dir: Path, target_dirs: tuple[Path, ...]
) -> None:
    snapshots = []
    for target_dir in target_dirs:
        tags = _blender_folder_tags(target_dir.name)
        if tags is None:
            raise ValueError(f"invalid prepared Blender folder: {target_dir.name}")
        snapshots.append(
            {
                "path": target_dir.name,
                "tags": [f"{key}={value}" for key, value in tags.items()],
            }
        )
    _write_json(
        product_dir / BLENDER_SOURCE_CONFIG_FILE_NAME,
        {
            "ingestion_mode": "explicit",
            "auto_ingestion_rules": {"ignore_paths": []},
            "explicit_ingestion_rules": {"snapshots": snapshots},
        },
    )


def prepare_blender_product(
    *,
    recipe_path: Path,
    product_dir: Path,
    versions: list[str] | None = None,
    platforms: list[str] | None = None,
    cache_dir: Path | None = None,
    force: bool = False,
) -> BlenderPreparationResult:
    base_url, recipe_assets = _load_blender_assets(recipe_path)
    selected_assets = _select_blender_assets(recipe_assets, versions, platforms)
    product_dir = product_dir.resolve()
    cache_dir = (cache_dir or product_dir.parent / ".blender-archives").resolve()
    manifest_path = product_dir / BLENDER_MANIFEST_FILE_NAME
    expected_manifest = {
        "schema_version": 1,
        "recipe": recipe_path.name,
        "assets": [
            {
                "version": asset.version,
                "platform": asset.platform,
                "archive": asset.archive_name,
                "sha256": asset.sha256,
            }
            for asset in selected_assets
        ],
    }
    if product_dir.exists() and not force:
        if not manifest_path.is_file():
            if any(product_dir.iterdir()):
                raise RuntimeError(
                    f"refusing to use non-empty Blender product without {BLENDER_MANIFEST_FILE_NAME}: {product_dir}"
                )
        else:
            existing_manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            if existing_manifest != expected_manifest:
                raise RuntimeError(
                    f"prepared Blender selection differs at {product_dir}; use --force to recreate it"
                )
    if force and product_dir.exists():
        _remove_tree(product_dir)
    product_dir.mkdir(parents=True, exist_ok=True)
    cache_dir.mkdir(parents=True, exist_ok=True)

    target_dirs: list[Path] = []
    for asset in selected_assets:
        archive_path = cache_dir / asset.archive_name
        if not archive_path.is_file() or _sha256_file(archive_path) != asset.sha256:
            print(
                f"Downloading Blender {asset.version} {asset.platform}: {asset.archive_name}",
                flush=True,
            )
            try:
                _download_blender_asset(asset, archive_path, base_url)
            except urllib.error.HTTPError:
                # Mirror bytes still have to match the independently pinned digest.
                _download_blender_asset(
                    asset,
                    archive_path,
                    "https://ftp.nluug.nl/pub/graphics/blender/release",
                )
        target_dir = product_dir / asset.folder_name
        if not target_dir.is_dir():
            staging_dir = product_dir / f".{asset.folder_name}.{os.getpid()}.part"
            _remove_tree(staging_dir)
            try:
                _extract_blender_archive_safely(archive_path, staging_dir)
                extracted_dir = staging_dir / asset.folder_name
                if not extracted_dir.is_dir():
                    raise RuntimeError(
                        f"Blender archive {asset.archive_name} did not contain {asset.folder_name}/"
                    )
                try:
                    extracted_dir.replace(target_dir)
                except PermissionError:
                    # Windows security software can briefly hold a freshly
                    # extracted directory open; copy it as a safe fallback.
                    shutil.copytree(extracted_dir, target_dir, symlinks=True)
            finally:
                _remove_tree(staging_dir)
        target_dirs.append(target_dir)

    ordered_targets = tuple(sorted(target_dirs, key=lambda path: path.name))
    _write_json(product_dir / BLENDER_MANIFEST_FILE_NAME, expected_manifest)
    _write_blender_source_config(product_dir, ordered_targets)
    return BlenderPreparationResult(product_dir, ordered_targets)
