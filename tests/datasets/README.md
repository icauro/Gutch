# Correctness Datasets

Generated inputs live in each run, using `../support/datasets.py`. They never
read or modify benchmark inputs.

`reference-v2/` freezes four compact snapshots in the current format, recaptured
on 2026-09-07 against the original independent inventories. `expected.json` records paths, SHA-256 content hashes, sizes and
attributes from the original input bytes. It is not derived by reconstructing
the fixture under test. The repository payload is under 100 KiB.

Normal runs only copy this fixture. For an intentional format change, capture
a replacement from independently specified source bytes and review the new
expected inventory together with the format change. Do not automatically
rewrite expectations after a failing test.
