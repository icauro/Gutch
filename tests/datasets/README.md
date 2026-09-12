# Shared dataset corpus

Run from the Gutch root:

```sh
python gutch.py dataset list
python gutch.py dataset prepare 02_chunk_boundaries
python gutch.py test --dataset 02_chunk_boundaries --repository-version v1
python gutch.py dataset prepare blender
python gutch.py test --dataset blender --repository-version v1 --steps 0 --timeout 1800
python gutch.py benchmark --dataset 02_chunk_boundaries
```

Each directory under `corpus/` is one case:

```text
case.json                 # Purpose, generator inputs or pinned downloads, creator and snapshot mappings
expected.json             # Independent source inventories, indexed by source version
source/v1/                # Committed generated input bytes
source/v2/
repository/v1/            # Committed frozen Gutch repository
```

There are ten fuzz-generated cases. Each case's complete committed contents must
fit within 16 MiB, including source versions, repository and JSON metadata.
They cover tiny and empty inputs, chunk boundaries, duplicates, compressibility,
random content, Unicode, nesting, attributes, version changes and many small files.
Generation settings and exact fuzzer input bytes live in `case.json`; there is no
separate recipe directory. `legacy_reference` preserves the previous four-snapshot
fixture and its original inventories, whose source files and exact creator commit
were not retained. It is an additional historical case, not one of the ten generated cases.

Blender uses the same schema, but only its repository and metadata are committed.
Its full Windows 5.2.0 and 5.2.1 releases are frozen together (about 528 MiB encoded).
The 16 MiB cap applies only to generated cases. Pinned downloads and extracted sources
live in `tests/.artifacts/datasets/blender/`. Preparation verifies archive SHA-256
hashes and checks prepared file contents against `expected.json`. It can use a
checksum-verified mirror if the primary host rejects the download.
Normal regression tests do not download Blender. Select its case explicitly.

`generate.py` owns synthetic generation, small test trees and the dataset CLI.
`fuzzed(root, input_bytes, profile=..., version=..., max_bytes=...)` accepts arbitrary
bytes for a future fuzzer harness, expands them deterministically and caps output
at a requested 128 bytes to 4 MiB per version. The saved cases use a 1 MiB per-version
cap. Paths, file counts and version numbers are bounded as well. The default seeded
repository test uses this same generation function. Coverage-guided execution and
automatic minimization are not implemented. `prepare_blender.py` only prepares real
source data; it does not generate or overwrite repositories.

`dataset prepare` writes disposable sources under `tests/.artifacts/datasets/<case>/source/`.
It checks existing prepared files instead of silently accepting modified content.
Benchmarks select that prepared source using `--dataset`; normal test runs select the
committed repository. Generated source bytes are also committed beside each case.
Git does not preserve empty directories or read-only attributes: the generator and
expected inventory describe those properties, and tests verify them in reconstructions.

To create a new frozen major-version repository intentionally:

```sh
python gutch.py dataset freeze CASE --repository-version v2 --gutch-exe PATH_TO_GUTCH_CLI
```

Freezing derives expectations from source files before invoking Gutch, reconstructs
and compares every captured snapshot, checks the size cap, then records the exact
crate version, engine commit and binary hash. It refuses to overwrite an existing
major version. A new version must retain the same independent source inventory.
`v1` is the initial corpus compatibility generation; creator metadata records the
actual crate version (currently 0.2.0). No Cargo version is inferred from the folder.
Normal tests only mutate copies under `tests/.artifacts/runs/`. Preserve old major
folders and add new ones; never rebuild fixtures automatically to fix a failing test.

Custom larger products remain available through `python gutch.py dataset --help`,
including size/count/path/metadata ranges, version mutations and optional BLAKE3
manifests (the latter require external `b3sum`). These write disposable outputs.
