from __future__ import annotations

import hashlib
import io
import json
import shutil
import stat
import tarfile
import tempfile
import unittest
import unicodedata
import zipfile
from pathlib import Path
from unittest import mock

from tests.datasets.generate import (
    DEFAULT_SEED,
    GenerationRanges,
    IntRange,
    METADATA_FILE_NAME,
    TEST_MANIFEST_FILE_NAME,
    _blake3_file_hex,
    _timestamp_base_seconds,
    write_fixture,
)

from tests.datasets.prepare_blender import _extract_blender_archive_safely, _load_blender_assets, _select_blender_assets, prepare_blender_product


def _ranges() -> GenerationRanges:
    return GenerationRanges(
        product_size_bytes=IntRange(48 * 1024, 64 * 1024),
        file_count=IntRange(8, 12),
        path_depth=IntRange(0, 4),
        file_size_bytes=IntRange(256, 32 * 1024),
        changed_files=IntRange(2, 2),
        patches_per_file=IntRange(2, 2),
        changed_bytes_per_file=IntRange(10, 10),
        added_files=IntRange(1, 2),
        removed_files=IntRange(1, 1),
        renamed_files=IntRange(1, 1),
        moved_files=IntRange(1, 1),
        long_path_files=IntRange(1, 2),
        relative_path_length=IntRange(120, 150),
        timestamp_age_seconds=IntRange(10, 1_000),
        readonly_files=IntRange(1, 2),
        executable_files=IntRange(1, 2),
        unicode_files=IntRange(1, 2),
        mixed_case_files=IntRange(1, 2),
    )


def _entries(fixture_dir: Path, version: str) -> list[dict]:
    path = fixture_dir / ".mock-base" / f"{version}.entries.json"
    return json.loads(path.read_text(encoding="utf-8"))


def _tree_fingerprint(root: Path) -> dict[str, tuple[int, int, str]]:
    fingerprint: dict[str, tuple[int, int, str]] = {}
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        file_stat = path.stat()
        fingerprint[path.relative_to(root).as_posix()] = (
            stat.S_IMODE(file_stat.st_mode),
            file_stat.st_mtime_ns,
            hashlib.sha256(path.read_bytes()).hexdigest(),
        )
    return fingerprint


class GutchMockProductCreateTests(unittest.TestCase):
    def test_generated_versions_stay_inside_size_count_and_depth_ranges(self) -> None:
        ranges = _ranges()
        with tempfile.TemporaryDirectory() as directory:
            result = write_fixture(
                root=Path(directory),
                name="bounded",
                total_versions=4,
                append_versions=0,
                force=False,
                ranges=ranges,
                seed=13579,
            )

            for version_index in range(1, 5):
                entries = _entries(result.fixture_dir, f"{version_index}.0.0")
                self.assertGreaterEqual(len(entries), ranges.file_count.minimum)
                self.assertLessEqual(len(entries), ranges.file_count.maximum)
                total_size = sum(int(entry["size_bytes"]) for entry in entries)
                self.assertGreaterEqual(total_size, ranges.product_size_bytes.minimum)
                self.assertLessEqual(total_size, ranges.product_size_bytes.maximum)
                for entry in entries:
                    self.assertGreaterEqual(int(entry["size_bytes"]), ranges.file_size_bytes.minimum)
                    self.assertLessEqual(int(entry["size_bytes"]), ranges.file_size_bytes.maximum)
                    depth = len(Path(str(entry["relative_path"])).parts) - 1
                    self.assertGreaterEqual(depth, ranges.path_depth.minimum)
                    self.assertLessEqual(depth, ranges.path_depth.maximum)
                paths = [str(entry["relative_path"]) for entry in entries]
                self.assertGreaterEqual(
                    sum(len(path) >= ranges.relative_path_length.minimum for path in paths),
                    ranges.long_path_files.minimum,
                )
                self.assertGreaterEqual(
                    sum(any(ord(character) > 127 for character in path) for path in paths),
                    ranges.unicode_files.minimum,
                )
                self.assertGreaterEqual(
                    sum(
                        any(character.islower() for character in path)
                        and any(character.isupper() for character in path)
                        for path in paths
                    ),
                    ranges.mixed_case_files.minimum,
                )
                self.assertGreaterEqual(
                    sum(bool(entry["readonly"]) for entry in entries),
                    ranges.readonly_files.minimum,
                )
                self.assertGreaterEqual(
                    sum(bool(entry["executable"]) for entry in entries),
                    ranges.executable_files.minimum,
                )

    def test_seed_reproduces_layout_and_content(self) -> None:
        ranges = _ranges()
        with tempfile.TemporaryDirectory() as first, tempfile.TemporaryDirectory() as second:
            first_result = write_fixture(
                root=Path(first),
                name="same",
                total_versions=3,
                append_versions=0,
                force=False,
                ranges=ranges,
                seed=24680,
            )
            second_result = write_fixture(
                root=Path(second),
                name="same",
                total_versions=3,
                append_versions=0,
                force=False,
                ranges=ranges,
                seed=24680,
            )

            self.assertEqual(
                _tree_fingerprint(first_result.fixture_dir),
                _tree_fingerprint(second_result.fixture_dir),
            )

    def test_default_seed_reproduces_complete_fixture(self) -> None:
        ranges = _ranges()
        with tempfile.TemporaryDirectory() as first, tempfile.TemporaryDirectory() as second:
            first_result = write_fixture(
                root=Path(first),
                name="default-seed",
                total_versions=2,
                append_versions=0,
                force=False,
                ranges=ranges,
            )
            second_result = write_fixture(
                root=Path(second),
                name="default-seed",
                total_versions=2,
                append_versions=0,
                force=False,
                ranges=ranges,
            )

            self.assertEqual(first_result.seed, DEFAULT_SEED)
            self.assertEqual(second_result.seed, DEFAULT_SEED)
            self.assertEqual(
                _tree_fingerprint(first_result.fixture_dir),
                _tree_fingerprint(second_result.fixture_dir),
            )

    @unittest.skipUnless(shutil.which("b3sum"), "b3sum is required for test manifests")
    def test_test_manifest_records_snapshot_paths_sizes_and_hashes(self) -> None:
        ranges = _ranges()
        with tempfile.TemporaryDirectory() as directory:
            result = write_fixture(
                root=Path(directory),
                name="manifest",
                total_versions=1,
                append_versions=0,
                force=False,
                ranges=ranges,
                seed=424242,
                emit_test_manifest=True,
            )

            manifest = json.loads(
                (result.fixture_dir / TEST_MANIFEST_FILE_NAME).read_text(encoding="utf-8")
            )
            self.assertEqual(manifest["schema_version"], 1)
            self.assertEqual(manifest["generator"]["seed"], 424242)
            self.assertEqual(manifest["generator"]["ranges"], ranges.to_json())
            self.assertEqual(
                {snapshot["path"] for snapshot in manifest["snapshots"]},
                {
                    "1.0.0-windows-x86-public",
                    "1.0.0-windows-x86-supporter",
                },
            )
            for snapshot in manifest["snapshots"]:
                snapshot_root = result.fixture_dir / snapshot["path"]
                for file_entry in snapshot["entries"]:
                    if file_entry["entry_type"] != "file":
                        continue
                    file_path = snapshot_root / file_entry["path"]
                    self.assertEqual(file_path.stat().st_size, file_entry["size_bytes"])
                    self.assertEqual(_blake3_file_hex(file_path), file_entry["blake3"])

    def test_blender_recipe_is_pinned_and_selects_requested_targets(self) -> None:
        recipe_path = Path("tests/datasets/corpus/blender/case.json")
        base_url, assets = _load_blender_assets(recipe_path)

        self.assertEqual(base_url, "https://download.blender.org/release")
        self.assertEqual(len(assets), 4)
        selected = _select_blender_assets(assets, ["5.2.0", "5.2.1"], ["windows-x64", "linux-x64"])
        self.assertEqual(
            [(asset.version, asset.platform) for asset in selected],
            [
                ("5.2.0", "linux-x64"),
                ("5.2.0", "windows-x64"),
                ("5.2.1", "linux-x64"),
                ("5.2.1", "windows-x64"),
            ],
        )
        self.assertTrue(all(len(asset.sha256) == 64 for asset in selected))

    def test_blender_product_reuses_pinned_archive_and_writes_source_config(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            recipe_path = root / "blender.json"
            archive_path = root / "cache" / "blender-5.0.0-windows-x64.zip"
            archive_path.parent.mkdir()
            with zipfile.ZipFile(archive_path, "w") as archive:
                archive.writestr("blender-5.0.0-windows-x64/blender.exe", b"blender")
            archive_digest = hashlib.sha256(archive_path.read_bytes()).hexdigest()
            recipe_path.write_text(
                json.dumps(
                    {
                        "schema_version": 1,
                        "base_url": "https://download.blender.org/release/",
                        "versions": [
                            {
                                "version": "5.0.0",
                                "assets": [
                                    {
                                        "platform": "windows-x64",
                                        "archive": archive_path.name,
                                        "sha256": archive_digest,
                                    }
                                ],
                            }
                        ],
                    }
                ),
                encoding="utf-8",
            )

            result = prepare_blender_product(
                recipe_path=recipe_path,
                product_dir=root / "Blender",
                cache_dir=archive_path.parent,
                platforms=["windows-x64"],
            )

            target = result.product_dir / "blender-5.0.0-windows-x64"
            self.assertEqual(result.target_dirs, (target,))
            self.assertEqual((target / "blender.exe").read_bytes(), b"blender")
            source_config = json.loads(
                (result.product_dir / ".gutch_source_config.json").read_text(encoding="utf-8")
            )
            self.assertEqual(
                source_config["explicit_ingestion_rules"]["snapshots"][0]["tags"],
                ["version::5.0.0", "os::windows", "arch::x86_64"],
            )

    def test_blender_archive_rejects_traversal(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive_path = root / "unsafe.zip"
            with zipfile.ZipFile(archive_path, "w") as archive:
                archive.writestr("../outside.txt", b"unsafe")
            with self.assertRaisesRegex(RuntimeError, "unsafe archive entry"):
                _extract_blender_archive_safely(archive_path, root / "target")

    def test_blender_tar_xz_extraction_preserves_the_official_folder(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive_path = root / "blender.tar.xz"
            payload = b"linux blender"
            with tarfile.open(archive_path, mode="w:xz") as archive:
                info = tarfile.TarInfo("blender-5.0.0-linux-x64/blender")
                info.size = len(payload)
                info.mode = 0o755
                archive.addfile(info, io.BytesIO(payload))
                link = tarfile.TarInfo("blender-5.0.0-linux-x64/blender-link")
                link.type = tarfile.SYMTYPE
                link.linkname = "blender"
                archive.addfile(link)

            _extract_blender_archive_safely(archive_path, root / "target")

            self.assertEqual(
                (root / "target" / "blender-5.0.0-linux-x64" / "blender").read_bytes(),
                payload,
            )
            self.assertEqual(
                (root / "target" / "blender-5.0.0-linux-x64" / "blender-link").read_bytes(),
                payload,
            )

    def test_byte_patch_count_and_length_are_randomized_within_ranges(self) -> None:
        ranges = GenerationRanges(
            product_size_bytes=IntRange(24 * 1024, 24 * 1024),
            file_count=IntRange(6, 6),
            path_depth=IntRange(1, 3),
            file_size_bytes=IntRange(512, 16 * 1024),
            changed_files=IntRange(2, 2),
            patches_per_file=IntRange(2, 2),
            changed_bytes_per_file=IntRange(10, 10),
            added_files=IntRange(0, 0),
            removed_files=IntRange(0, 0),
            renamed_files=IntRange(0, 0),
            moved_files=IntRange(0, 0),
            long_path_files=IntRange(0, 0),
            relative_path_length=IntRange(120, 150),
            timestamp_age_seconds=IntRange(0, 0),
            readonly_files=IntRange(0, 0),
            executable_files=IntRange(0, 0),
            unicode_files=IntRange(0, 0),
            mixed_case_files=IntRange(0, 0),
        )
        with tempfile.TemporaryDirectory() as directory:
            result = write_fixture(
                root=Path(directory),
                name="patches",
                total_versions=2,
                append_versions=0,
                force=False,
                ranges=ranges,
                seed=97531,
            )
            first_root = result.fixture_dir / ".mock-base" / "1.0.0"
            second_root = result.fixture_dir / ".mock-base" / "2.0.0"
            paths = [str(entry["relative_path"]) for entry in _entries(result.fixture_dir, "1.0.0")]
            changed_byte_counts = []
            for relative_path in paths:
                first_bytes = (first_root / relative_path).read_bytes()
                second_bytes = (second_root / relative_path).read_bytes()
                changed_count = sum(left != right for left, right in zip(first_bytes, second_bytes))
                if changed_count:
                    changed_byte_counts.append(changed_count)

            self.assertEqual(changed_byte_counts, [10, 10])

    def test_long_unicode_mixed_case_paths_timestamps_and_permissions(self) -> None:
        ranges = GenerationRanges(
            product_size_bytes=IntRange(64 * 1024, 64 * 1024),
            file_count=IntRange(10, 10),
            path_depth=IntRange(1, 4),
            file_size_bytes=IntRange(256, 32 * 1024),
            changed_files=IntRange(1, 1),
            patches_per_file=IntRange(1, 1),
            changed_bytes_per_file=IntRange(1, 10),
            added_files=IntRange(0, 0),
            removed_files=IntRange(0, 0),
            renamed_files=IntRange(0, 0),
            moved_files=IntRange(0, 0),
            long_path_files=IntRange(2, 2),
            relative_path_length=IntRange(140, 150),
            timestamp_age_seconds=IntRange(100, 200),
            readonly_files=IntRange(3, 3),
            executable_files=IntRange(2, 2),
            unicode_files=IntRange(3, 3),
            mixed_case_files=IntRange(4, 4),
        )
        seed = 556677
        with tempfile.TemporaryDirectory() as directory:
            result = write_fixture(
                root=Path(directory),
                name="filesystem-dimensions",
                total_versions=1,
                append_versions=0,
                force=False,
                ranges=ranges,
                seed=seed,
            )
            entries = _entries(result.fixture_dir, "1.0.0")
            paths = [str(entry["relative_path"]) for entry in entries]

            self.assertEqual(sum(len(path) >= 140 for path in paths), 2)
            self.assertEqual(sum(any(ord(character) > 127 for character in path) for path in paths), 3)
            self.assertEqual(
                sum(
                    any(character.islower() for character in path)
                    and any(character.isupper() for character in path)
                    for path in paths
                ),
                4,
            )
            self.assertEqual(sum(bool(entry["readonly"]) for entry in entries), 3)
            self.assertEqual(sum(bool(entry["executable"]) for entry in entries), 2)
            self.assertEqual(
                len({unicodedata.normalize("NFC", path).casefold() for path in paths}),
                len(paths),
            )

            timestamp_base_ns = _timestamp_base_seconds(seed) * 1_000_000_000
            for entry in entries:
                modified_at_ns = int(entry["modified_at_ns"])
                age_seconds = (timestamp_base_ns - modified_at_ns) / 1_000_000_000
                self.assertGreaterEqual(age_seconds, 99)
                self.assertLessEqual(age_seconds, 200)
                target_path = result.fixture_dir / ".mock-base" / "1.0.0" / str(entry["relative_path"])
                self.assertLessEqual(abs(target_path.stat().st_mtime_ns - modified_at_ns), 1_000_000_000)

            for snapshot_dir in result.captured_snapshot_dirs:
                for relative_path in paths:
                    self.assertLessEqual(len(str(snapshot_dir / relative_path)), 240)

    def test_appending_reuses_seed_and_ranges(self) -> None:
        ranges = _ranges()
        with tempfile.TemporaryDirectory() as appended, tempfile.TemporaryDirectory() as complete:
            appended_result = write_fixture(
                root=Path(appended),
                name="fixture",
                total_versions=1,
                append_versions=0,
                force=False,
                ranges=ranges,
                seed=112233,
            )
            write_fixture(
                root=Path(appended),
                name="fixture",
                total_versions=0,
                append_versions=2,
                force=False,
            )
            complete_result = write_fixture(
                root=Path(complete),
                name="fixture",
                total_versions=3,
                append_versions=0,
                force=False,
                ranges=ranges,
                seed=112233,
            )

            self.assertEqual(
                _tree_fingerprint(appended_result.fixture_dir),
                _tree_fingerprint(complete_result.fixture_dir),
            )


if __name__ == "__main__":
    unittest.main()
