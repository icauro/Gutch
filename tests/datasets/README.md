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

`generator/product.py` owns product generation, mutation and archive preparation.
Its optional BLAKE3 manifest uses the external `b3sum` utility, not Gutch.
`trees.py` supplies small fixed and seeded trees with independent SHA-256 expected
inventories. Neither module imports runners, benchmark adapters or test assertions.
Consumers supply the seed and output path and own execution and cleanup.

Frozen encoded repositories belong in `tests/fixtures/repositories/`, not here.
