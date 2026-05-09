# Examples

The example test scripts now use the real fixture root at `D:\Proj\Gutch\TestData`.

## Fixture layout

Each top-level non-dot directory under `D:\Proj\Gutch\TestData` is a selectable product fixture.

Examples:

- `D:\Proj\Gutch\TestData\Blender`
- `D:\Proj\Gutch\TestData\LLVM`
- `D:\Proj\Gutch\TestData\Quake`

Inside each product folder, every non-dot child entry is treated as one source build:

- directories are used directly as build inputs
- files are passed directly to `gutch target insert`, which stages them internally

Dot-prefixed entries are ignored so generated outputs do not become source builds.

## Run the test

```pwsh
pwsh -File run.ps1 -GameDir Blender
```

If `-GameDir` is omitted, `run.ps1` lists product fixtures under `D:\Proj\Gutch\TestData` and asks which one to run.

To run every fixture in one pass, use:

```pwsh
pwsh -File run.ps1 -AllTests
```

By default the script uses every non-dot child build inside the selected product folder. If `-Versions` is provided, it uses the last `N` sorted build entries.

This script:

1. Builds gutch artifacts for each selected build into the product-local `.content_repo`
2. Reconstructs the latest selected build from the shared bundle store
3. Verifies via gutch hash check
4. Uses `gutch path compare` to binary-compare the reconstructed output against the latest selected source build

The script also asks `gutch` to write JSON stats under the product-local `.artifacts\run\` tree.

Per-product generated output lives under hidden dot folders:

- `.content_repo/` for the content repo root, including `repo.json`, `targets/`, and `bundles/`
- `.artifacts/applied/` for reconstructed output

## Compare compression modes

```pwsh
pwsh -File compare-compress.ps1 -GameDir Blender
```

If `-GameDir` is omitted, `compare-compress.ps1` uses the same fixture picker as `run.ps1`.

This script:

1. Builds the same selected product twice, once with `--compression-unit chunk` and once with `--compression-unit bundle`
2. Writes each mode under `D:\Proj\Gutch\TestData\<Product>\.artifacts\compare_compress\`
3. Reconstructs and verifies the latest selected build for each mode
4. Uses `gutch path compare` to binary-compare both outputs against the latest selected source build

Each compare mode also asks `gutch` to write JSON stats under that mode's local `.artifacts\` tree.

## Notes

- The old mock fixture flow with `fixture.json` and `truth/vN` is no longer the active test workflow.
- Build ordering is based on child entry name sort order.
