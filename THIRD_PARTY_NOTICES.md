# Third-party notices

Original Gutch code, tooling and documentation are licensed under the
[MIT License](LICENSE), copyright (c) 2026 icauro. Included third-party material
retains the licenses and attribution below. The root MIT license does not
replace those terms. This notice also covers earlier paths of the same adapted
code in the retained Git history.

## Included or adapted source

| Component | Included files | License and attribution |
| --- | --- | --- |
| FastCDC v2020, fastcdc-rs 5.0.0 | `src/04_artifact/fastcdc_v2020/mod.rs`, `src/04_artifact/gear.rs` | MIT; Nathan Fiedler. Preserve the [upstream license](src/04_artifact/fastcdc_v2020/LICENSE) and source copyright notices. |
| Plakar-compatible JC chunker and Gear table | `src/04_artifact/jc.rs`, `src/04_artifact/jc_gear.rs` | ISC; copyright (c) 2024 Gilles Chehade. Full text: [jc.LICENSE](src/04_artifact/jc.LICENSE). |
| RAM / VectorCDC reference and adaptation | `src/04_artifact/ram.rs`, `tests/benchmarks/ram_reference.cpp` | Apache-2.0; copyright (c) 2023 WASL. Full text: [RAM license](src/04_artifact/ram/LICENSE). Changes for Gutch are identified in the source and [reference notes](src/04_artifact/ram/README.md). |

Pinned upstream sources:

- [fastcdc-rs, commit eeb3cbe8ed4eeef020aa346707bbdb29abd814ad](https://github.com/nlfiedler/fastcdc-rs/tree/eeb3cbe8ed4eeef020aa346707bbdb29abd814ad).
  Gutch retains the cut routine, masks and Gear table; it omits the upstream
  owned iterators, seeded tables and unrelated APIs. See the [adaptation notes](src/04_artifact/fastcdc_v2020/README.md).
- [go-cdc-chunkers, commit b6c4abce19e9691184a0aceefc4e9f53fcfe79c8](https://github.com/PlakarKorp/go-cdc-chunkers/tree/b6c4abce19e9691184a0aceefc4e9f53fcfe79c8).
  The `jc-v1.1.0` name identifies an algorithm variant, not a repository release
  tag. Gutch ports that chunking rule and reference Gear table to Rust; keyed
  tables and the upstream Go interfaces are not included.
- [UWASL/dedup-bench, commit 8e2697cbf6332ac5da6dc615bfab82a720e820e4](https://github.com/UWASL/dedup-bench/tree/8e2697cbf6332ac5da6dc615bfab82a720e820e4).
  The Rust RAM implementation replaces scratch arrays with bounded register
  reductions. The C++ benchmark retains reference kernels with a Gutch timing
  wrapper. This upstream revision contains a license and no separate NOTICE file.

The Rust engine source package therefore declares `MIT AND Apache-2.0 AND ISC`.
This expresses the included files' combined obligations; it does not relicense
the original Gutch files away from MIT. Include this notice and all three
component license texts with redistributed source or compiled products using
these components, and retain the modification notices.

## Package dependencies and external inputs

Cargo and Python dependencies are installed separately and retain their own
licenses. `Cargo.lock` and `uv.lock` identify the resolved versions; package
metadata and their distributed license/notice files identify their terms.
The license texts above cover adapted source, not a complete binary dependency
notice bundle. Before distributing compiled binaries or a bundled Python
environment, include the required notices for that exact target and dependency
set, including native libraries and Unicode data. Build caches and environments
are not included in this source repository.

Blender inputs are downloaded only into ignored caches. Blender, its bundled
assets and any other user-supplied workloads retain their upstream licenses;
they are not relicensed by Gutch. The tracked synthetic corpus is generated
project test data. Research papers and external articles are referenced by links;
copies with unverified redistribution terms are excluded from the exported history.
