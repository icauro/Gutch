"""Measure experimental metadata encodings; never writes production repositories.

Requires Python 3.14 (stdlib compression.zstd). Run from the Gutch root:
  python tests/benchmarks/binary_layout_experiment.py
Writes reproducible measurements under tests/.artifacts/binary-layouts/.
These are Python encoding/decoding experiments, not Rust performance forecasts.
"""
from __future__ import annotations

import argparse
from collections import Counter
from compression import zstd
from dataclasses import dataclass
import hashlib
import gc
import json
from pathlib import Path
import platform
import statistics
import struct
import time
import tracemalloc

ROOT = Path(__file__).resolve().parents[2]
EMPTY_HASH = bytes.fromhex("af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262")

@dataclass(eq=True)
class Entry:
    kind: int
    path: bytes
    flags: int = 0
    size: int = 0
    digest: bytes = EMPTY_HASH
    chunks: tuple = ()
    target: bytes = b""

class Reader:
    def __init__(self, data):
        self.data, self.position = data, 0
    def take(self, count):
        if count < 0 or self.position + count > len(self.data):
            raise ValueError("truncated record")
        result = self.data[self.position:self.position + count]
        self.position += count
        return result
    def number(self, code):
        return struct.unpack("<" + code, self.take(struct.calcsize("<" + code)))[0]
    def string(self):
        return self.take(self.number("I"))
    def finish(self):
        if self.position != len(self.data):
            raise ValueError("trailing bytes")


def integer(value, code="I"):
    return struct.pack("<" + code, value)


def string(value):
    return integer(len(value)) + value


def read_current(data):
    reader = Reader(data)
    if reader.take(16) != b"GUTCH_BLUEPRINT\0" or reader.number("H") != 1 or reader.number("H"):
        raise ValueError("unexpected current format")
    count, references = reader.number("I"), reader.number("Q")
    entries = []
    for _ in range(count):
        kind, path = reader.number("B"), reader.string()
        entry = Entry(kind, path)
        if kind == 0:
            entry.size = reader.number("Q")
            executable, readonly = reader.number("B"), reader.number("B")
            entry.flags = readonly | (executable << 1)
            entry.digest = reader.take(32)
            entry.chunks = tuple((reader.take(32), reader.number("I")) for _ in range(reader.number("I")))
            assert sum(size for _, size in entry.chunks) == entry.size
        elif kind == 1:
            entry.flags = reader.number("B")
        elif kind == 2:
            entry.target = reader.string()
        else:
            raise ValueError("entry kind")
        entries.append(entry)
    assert references == sum(len(entry.chunks) for entry in entries)
    reader.finish()
    return entries


def write_current(entries):
    output = bytearray(b"GUTCH_BLUEPRINT\0" + integer(1, "H") + integer(0, "H")
                       + integer(len(entries)) + integer(sum(len(e.chunks) for e in entries), "Q"))
    for entry in entries:
        output += integer(entry.kind, "B") + string(entry.path)
        if entry.kind == 0:
            output += integer(entry.size, "Q") + bytes([(entry.flags >> 1) & 1, entry.flags & 1])
            output += entry.digest + integer(len(entry.chunks))
            for digest, size in entry.chunks:
                output += digest + integer(size)
        elif entry.kind == 1:
            output += bytes([entry.flags])
        else:
            output += string(entry.target)
    return bytes(output)


def variable_integer(value):
    output = bytearray()
    while value >= 128:
        output.append((value & 127) | 128)
        value >>= 7
    output.append(value)
    return output


def read_variable(reader):
    value = 0
    for shift in range(0, 35, 7):
        part = reader.number("B")
        value |= (part & 127) << shift
        if not part & 128:
            return value
    raise ValueError("oversized integer")


def encode_paths(entries, encoding):
    output = bytearray()
    previous = b""
    directory_ids = {b"": 0}
    for index, entry in enumerate(entries):
        path = entry.path
        if encoding == "full":
            output += string(path)
        elif encoding == "prefix":
            shared = 0
            for left, right in zip(previous, path):
                if left != right:
                    break
                shared += 1
            suffix = path[shared:]
            output += variable_integer(shared) + variable_integer(len(suffix)) + suffix
        else:
            parent, _, name = path.rpartition(b"/")
            output += integer(directory_ids[parent]) + string(name)
            if entry.kind == 1:
                directory_ids[path] = index + 1
        previous = path
    return bytes(output)


def decode_paths(data, kinds, encoding):
    reader, paths, previous = Reader(data), [], b""
    for index, kind in enumerate(kinds):
        if encoding == "full":
            path = reader.string()
        elif encoding == "prefix":
            shared, length = read_variable(reader), read_variable(reader)
            if shared > len(previous):
                raise ValueError("bad prefix")
            path = previous[:shared] + reader.take(length)
        else:
            parent, name = reader.number("I"), reader.string()
            if parent > index or (parent and kinds[parent - 1] != 1):
                raise ValueError("bad directory reference")
            path = (paths[parent - 1] + b"/" if parent else b"") + name
        paths.append(path)
        previous = path
    reader.finish()
    return paths


def sections(entries, path_encoding, dictionary):
    files = [entry for entry in entries if entry.kind == 0]
    modes = [0 if not e.size else 1 if len(e.chunks) == 1 and e.chunks[0] == (e.digest, e.size) else 2
             for e in files]
    core = bytearray(integer(len(entries)) + bytes(e.kind for e in entries))
    core += b"".join(integer(e.flags, "H") for e in entries)
    core += b"".join(integer(e.size, "Q") for e in files)
    core += b"".join(e.digest for e in files if e.size)
    core += bytes(modes)
    core += b"".join(string(e.target) for e in entries if e.kind == 2)
    references, offsets = [], [0]
    for entry, mode in zip(files, modes):
        if mode == 2:
            references.extend(entry.chunks)
            offsets.append(len(references))
    chunk_data = bytearray(b"".join(integer(offset) for offset in offsets))
    if dictionary:
        unique = dict.fromkeys(references)
        rows = {value: index for index, value in enumerate(unique)}
        chunk_data += integer(len(rows))
        chunk_data += b"".join(digest + integer(size) for digest, size in rows)
        chunk_data += b"".join(integer(rows[reference]) for reference in references)
    else:
        chunk_data += b"".join(digest + integer(size) for digest, size in references)
    return [encode_paths(entries, path_encoding), bytes(core), bytes(chunk_data)]


def restore_sections(parts, path_encoding, dictionary, *, core_only=False):
    paths_data, core_data = parts[:2]
    reader = Reader(core_data)
    count = reader.number("I")
    kinds = list(reader.take(count))
    flags = [reader.number("H") for _ in kinds]
    file_count = kinds.count(0)
    sizes = [reader.number("Q") for _ in range(file_count)]
    digests = [reader.take(32) if size else EMPTY_HASH for size in sizes]
    modes = list(reader.take(file_count))
    targets = [reader.string() for kind in kinds if kind == 2]
    reader.finish()
    paths = decode_paths(paths_data, kinds, path_encoding)
    files = iter(zip(sizes, digests, modes))
    targets = iter(targets)
    entries = []
    multiple = []
    for kind, path, flag in zip(kinds, paths, flags):
        entry = Entry(kind, path, flag)
        if kind == 0:
            entry.size, entry.digest, mode = next(files)
            if mode == 1:
                entry.chunks = ((entry.digest, entry.size),)
            elif mode == 2:
                multiple.append(entry)
            elif mode != 0:
                raise ValueError("invalid file mode")
        elif kind == 2:
            entry.target = next(targets)
        entries.append(entry)
    if core_only:
        return entries
    reader = Reader(parts[2])
    offsets = [reader.number("I") for _ in range(len(multiple) + 1)]
    if dictionary:
        rows = [(reader.take(32), reader.number("I")) for _ in range(reader.number("I"))]
        references = [rows[reader.number("I")] for _ in range(offsets[-1])]
    else:
        references = [(reader.take(32), reader.number("I")) for _ in range(offsets[-1])]
    for index, entry in enumerate(multiple):
        entry.chunks = tuple(references[offsets[index]:offsets[index + 1]])
    reader.finish()
    for entry in entries:
        if entry.kind == 0:
            assert sum(size for _, size in entry.chunks) == entry.size
    return entries


def pack(parts):
    # Complete experimental envelope: 12-byte header and 12 bytes per group.
    encoded = []
    for part in parts:
        compressed = zstd.compress(part, level=3)
        encoded.append((1, compressed) if len(compressed) < len(part) else (0, part))
    output = bytearray(b"GTEXPER1" + integer(len(parts), "H") + integer(0, "H"))
    for index, ((codec, body), part) in enumerate(zip(encoded, parts)):
        output += struct.pack("<HBBII", index, codec, 0, len(body), len(part))
    for codec, body in encoded:
        output += body
    return bytes(output)


def unpack(data, selected=None):
    reader = Reader(data)
    assert reader.take(8) == b"GTEXPER1"
    count = reader.number("H")
    assert reader.number("H") == 0
    descriptors = [struct.unpack("<HBBII", reader.take(12)) for _ in range(count)]
    result = []
    for index, codec, reserved, length, raw_length in descriptors:
        body = reader.take(length)
        if selected is not None and index not in selected:
            result.append(None)
            continue
        decoded = zstd.decompress(body) if codec else body
        assert not reserved and len(decoded) == raw_length
        result.append(decoded)
    reader.finish()
    return result


def merge_parts(parts):
    return b"".join(integer(len(part)) + part for part in parts)


def split_parts(data):
    reader = Reader(data)
    parts = [reader.string() for _ in range(3)]
    reader.finish()
    return parts


def synthetic(name, count=2000):
    def digest(value):
        return hashlib.sha256(str(value).encode()).digest()  # Hash-shaped test identifiers.
    if name == "deep_paths":
        entries = []
        parent = b""
        for index in range(128):
            parent = (parent + b"/" if parent else b"") + f"directory-{index:03d}".encode()
            entries.append(Entry(1, parent))
        for index in range(300):
            identity = digest(index)
            entries.append(Entry(0, parent + f"/file-{index:04d}".encode(), size=4096,
                                 digest=identity, chunks=((identity,4096),)))
        return sorted(entries, key=lambda entry: entry.path)
    entries = [Entry(1, b"assets")]
    for index in range(count):
        path = f"assets/file-{index:06d}".encode()
        chunks = tuple((digest(0 if name == "repeated_chunks" else (index, part)), 4096) for part in range(4))
        if name == "single_chunks":
            chunks = ((digest(index), 4096),)
        entries.append(Entry(0, path, index % 4, sum(size for _, size in chunks),
                             chunks[0][0] if len(chunks) == 1 else digest((index, "file")), chunks))
    entries += [Entry(0, b"empty"), Entry(2, b"link", target=b"assets/file-000000")]
    return sorted(entries, key=lambda entry: entry.path)


def timed(function, repetitions):
    enabled = gc.isenabled()
    gc.collect()
    gc.disable()
    try:
        function()
        samples = []
        for _ in range(repetitions):
            started = time.perf_counter_ns()
            function()
            samples.append((time.perf_counter_ns() - started) / 1e6)
        return round(statistics.median(samples), 4)
    finally:
        if enabled:
            gc.enable()


def inspect_index(data):
    reader = Reader(data)
    assert reader.take(16) == b"GUTCH_INDEX\0\0\0\0\0"
    assert reader.number("H") == 1 and reader.number("H") == 0
    count, artifact_count, timestamp = reader.number("Q"), reader.number("I"), reader.string()
    artifacts = [(reader.take(32), reader.number("I"), reader.take(4)) for _ in range(artifact_count)]
    chunks = [(reader.take(32), reader.number("I"), reader.number("I"), reader.number("I")) for _ in range(count)]
    reader.finish()
    return artifacts, chunks, timestamp


def other_formats(repetitions):
    results = []
    for repo in sorted((ROOT / "tests/datasets/corpus").glob("*/repository/format-v1")):
        data = (repo / "chunk_map.gtcbin").read_bytes()
        artifacts, chunks, timestamp = inspect_index(data)
        keys = b"".join(row[0] for row in chunks)
        locations = b"".join(integer(row[column]) for column in (1,2,3) for row in chunks)
        # Dictionary IDs retained in binary; no on-disk per-chunk artifact duplication.
        artifact_columns = b"".join(row[0] for row in artifacts) + b"".join(integer(row[1]) for row in artifacts)
        candidate = pack([artifact_columns, keys, locations])
        unpacked = unpack(candidate)
        assert unpacked == [artifact_columns, keys, locations]
        for i, row in enumerate(chunks):
            assert keys[32*i:32*(i+1)] == row[0]
            assert tuple(struct.unpack_from("<I", locations, 4*(column*len(chunks)+i))[0] for column in range(3)) == row[1:]
        row = dict(case=repo.parent.parent.name, current_index_bytes=len(data),
                   current_index_zstd3_bytes=len(zstd.compress(data,level=3)),
                   candidate_index_bytes=len(candidate), artifact_count=len(artifacts), chunk_count=len(chunks),
                   removed_timestamp_bytes=len(timestamp)+4, removed_artifact_padding_bytes=4*len(artifacts),
                   python_index_decode_ms=timed(lambda: inspect_index(data),repetitions),
                   python_column_decompress_ms=timed(lambda: unpack(candidate),repetitions),
                   bundles=0,bundle_bytes=0,footer_bytes=0,footer_zstd3_bytes=0,column_footer_zstd3_bytes=0)
        for path in (repo / "bundles").glob("*/*"):
            if not path.is_file():
                continue
            size = path.stat().st_size
            with path.open("rb") as stream:
                header=stream.read(24)
                count=struct.unpack_from("<I",header,20)[0]
                stream.seek(size-84)
                footer_length=struct.unpack("<I",stream.read(4))[0]
                assert stream.read(16)==b"GUTCH_BUNDLE_END"
                assert stream.read(64).decode("ascii")==path.name
                stream.seek(size-84-footer_length)
                footer=stream.read(footer_length)
            assert len(footer)==4+36*count and struct.unpack_from("<I",footer)[0]==count
            hashes=b"".join(footer[4+36*i:36+36*i] for i in range(count))
            sizes=b"".join(footer[36+36*i:40+36*i] for i in range(count))
            rebuilt=integer(count)+b"".join(hashes[32*i:32*(i+1)]+sizes[4*i:4*(i+1)] for i in range(count))
            assert rebuilt==footer
            row['bundles']+=1; row['bundle_bytes']+=size; row['footer_bytes']+=len(footer)
            row['footer_zstd3_bytes']+=len(zstd.compress(footer,level=3))
            row['column_footer_zstd3_bytes']+=len(zstd.compress(hashes+sizes,level=3))
        results.append(row)
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "tests/.artifacts/binary-layouts/results.json")
    parser.add_argument("--repetitions", type=int, default=5)
    args = parser.parse_args()
    if args.repetitions < 1:
        parser.error("repetitions must be positive")
    cases = []
    for path in sorted((ROOT / "tests/datasets/corpus").glob("*/repository/format-v1/snapshots/*/snapshot_blueprint.gtcbin")):
        data = path.read_bytes()
        cases.append((path.relative_to(ROOT).as_posix(), data, read_current(data)))
    for name in ("single_chunks", "unique_chunks", "repeated_chunks", "deep_paths"):
        entries = synthetic(name)
        cases.append(("synthetic/" + name, write_current(entries), entries))
    report = {"python": platform.python_version(), "platform": platform.platform(),
              "repetitions": args.repetitions, "script_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(), "cases": [], "scope": "encoding experiments; not production codecs; median warm timings with cyclic GC disabled",
              "other_formats": other_formats(args.repetitions)}
    for name, data, entries in cases:
        assert write_current(entries) == data
        assert read_current(write_current(entries)) == entries
        row = {"name": name, "source_sha256": hashlib.sha256(data).hexdigest(), "entries": len(entries),
               "current_bytes": len(data), "current_zstd3_bytes": len(zstd.compress(data, level=3)), "variants": {}}
        row["current_decode_ms"] = timed(lambda: read_current(data), args.repetitions)
        tracemalloc.start()
        decoded = read_current(data)
        row["python_current_decode_peak_bytes"] = tracemalloc.get_traced_memory()[1]
        del decoded
        tracemalloc.stop()
        for path_encoding in ("full", "directory", "prefix"):
            for dictionary in (False, True):
                parts = sections(entries, path_encoding, dictionary)
                assert restore_sections(parts, path_encoding, dictionary) == entries
                for grouped in (False, True):
                    groups = parts if grouped else [merge_parts(parts)]
                    blob = pack(groups)
                    def decode(core_only=False):
                        restored = unpack(blob, {0, 1} if grouped and core_only else None)
                        if not grouped:
                            restored = split_parts(restored[0])
                        return restore_sections(restored, path_encoding, dictionary, core_only=core_only)
                    assert decode() == entries
                    core = decode(True)
                    assert [(e.kind,e.path,e.flags,e.size,e.digest,e.target) for e in core] == [
                        (e.kind,e.path,e.flags,e.size,e.digest,e.target) for e in entries]
                    key = f"{path_encoding}-{'dictionary' if dictionary else 'direct'}-{'groups' if grouped else 'whole'}"
                    row["variants"][key] = {"bytes": len(blob), "raw_section_bytes": sum(map(len,parts)),
                        "full_decode_ms": timed(decode, args.repetitions),
                        "core_decode_ms": timed(lambda: decode(True), args.repetitions),
                        "encode_ms": timed(lambda: pack(sections(entries,path_encoding,dictionary) if grouped else
                                                         [merge_parts(sections(entries,path_encoding,dictionary))]), args.repetitions)}
                    if key == "directory-direct-groups":
                        tracemalloc.start()
                        decoded = decode(True)
                        row["python_candidate_core_peak_bytes"] = tracemalloc.get_traced_memory()[1]
                        del decoded
                        tracemalloc.stop()
        report["cases"].append(row)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(f"Verified {name}", flush=True)
    report["status"] = "passed"
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"Report: {args.output}", flush=True)

if __name__ == "__main__":
    main()
