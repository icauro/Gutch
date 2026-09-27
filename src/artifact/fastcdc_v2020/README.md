# FastCDC v2020 subset

Origin: fastcdc 5.0.0, https://github.com/nlfiedler/fastcdc-rs
Upstream commit: eeb3cbe8ed4eeef020aa346707bbdb29abd814ad
Source: src/v2020/mod.rs
Source SHA-256: 930f3646dbd331c6ff9d6274b2e8d6bf64a4ce9456b6b743f97bcec436f6d46d
License: MIT; preserve LICENSE and source copyright notices when redistributing.

mod.rs copies the masks, default gear tables, and two-byte cut routine. The
entry point uses the default tables directly. Seeded tables, alternate algorithms,
async adapters, owned chunk iterators, and unrelated APIs are omitted. The kernel
retains upstream boundary behavior; Gutch owns the borrowed streaming adapter in
../chunk_stream.rs. Gutch fixes normalization at level 1, as in its persisted chunking profile.

Upstream is pinned as a test-only dependency for differential boundary tests.
Future upstream fixes require review: the local source does not update with Cargo.

Include LICENSE with source redistributions and compiled products using this code.
