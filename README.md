# Gutch

Gutch packages software builds into compact, versioned snapshots and installs
them anywhere as verified copies. Identical content is stored once, so
keeping many versions costs little more than keeping one, and updating an
installation reuses what is already on disk.

```sh
gutch-cli repo init --repo store
gutch-cli snapshot capture --repo store --source build-1.0 --tag version=1.0.0
gutch-cli installation reconstruct --repo store --snapshot ID --destination app
gutch-cli installation verify --repo store --snapshot ID --destination app
```

Gutch can:

- capture folders into snapshots, with tags such as versions and platforms;
- install a snapshot safely: the old installation stays in place until the
  new one is complete and checked;
- verify an installation and repair only what is missing or damaged;
- download a snapshot and install it at the same time;
- enforce filename rules so every snapshot installs on chosen filesystems;
- synchronize repositories with a server provided by a host application.

Gutch is built for software delivery, not backup: it does not keep
ownership, timestamps or extended attributes.

## Documentation

The [user guide](_docs/README.md) starts with
[getting started](_docs/01_getting_started.md) and covers every command, plus
[embedding Gutch](_docs/08_embedding_gutch.md) in your own Rust application.

To work on Gutch itself, see the [developer guide](_dev_guide/README.md).

## Getting Gutch

Prebuilt `gutch-cli` binaries for Linux and Windows on x86-64 and ARM64 are
attached to successful runs of **Actions > Build Gutch CLI**. To build from
source with Rust 1.100 or newer:

```sh
cargo build --release -p gutch-cli
```

## License

Original project code is [MIT licensed](LICENSE). Included third-party source
retains its own terms; see [third-party notices](THIRD_PARTY_NOTICES.md).
