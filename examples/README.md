# Examples

## Create a mock game fixture

```pwsh
pwsh -File create-mock.ps1 -Name my-game -SizeMB 50 -Versions 3 -ChangePercent 25
```

| Param | Default | Description |
|---|---|---|
| `-Name` | `mock-game-1` | Game name (used as directory name) |
| `-SizeMB` | `4` | Total uncompressed size in MB |
| `-Versions` | `2` | Number of game versions to generate |
| `-ChangePercent` | `30` | % of files that change between each version |

Creates:

```
fixtures/<Name>/
  truth/v1/      Full game files
  truth/v2/      Version 2 (some files changed, some copied from v1)
  ...
  artifacts/v1/  (empty, filled by run.ps1)
  applied/       (empty, filled by run.ps1)
```

## Run the test

```pwsh
pwsh -File run.ps1 -GameDir fixtures/my-game -Versions 3
```

1. Builds gutch artifacts for each version
2. Applies v1 full build + subsequent deltas to reconstruct the last version
3. Verifies via gutch hash check
4. Binary diffs against ground truth

## Quick start

```pwsh
# Create default fixture (4 MB, 2 versions, 30% change)
pwsh -File create-mock.ps1

# Run it
pwsh -File run.ps1
```