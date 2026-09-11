# Generate a reproducible dataset

Run from the Gutch root:

```sh
python gutch.py dataset --help
python gutch.py dataset --name small-example --versions 2 --size-mib 4 --seed 123
```

The generator writes products beneath `tests/.artifacts/datasets/products/` by
default. `--fixtures-root` selects another output root. Record the seed and all
size/version settings to reproduce the same generation request. Recipes under
`recipes/` define the fixed benchmark workloads; they are source-controlled inputs.

`generator/product.py` owns generation, mutation and archive preparation.
Correctness assertions do not use the generator's engine-derived manifest as an
oracle. The small correctness trees and independent SHA-256 inventories currently
live in `tests/support/datasets.py`.

Frozen encoded repositories belong in `tests/fixtures/repositories/`, not here.
