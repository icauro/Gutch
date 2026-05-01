param(
    [string]$GameDir = "$PSScriptRoot\fixtures\mock-game-1",
    [int]$Versions = 2,
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
Push-Location $PSScriptRoot\..

if (-not (Test-Path "$GameDir\truth\v1")) {
    Write-Host "No fixture found at $GameDir. Run create-mock.ps1 first."
    exit 1
}

# --- Build ---
if (-not $SkipBuild) {
    Write-Host "=== Building artifacts ==="
    for ($v = 1; $v -le $Versions; $v++) {
        Write-Host "  v$v ($(if ($v -eq 1){"full"}else{"delta"}))..."
        cargo run -- build `
            --dir "$GameDir\truth\v$v" `
            --product mock-game `
            --version "$v.0.0" `
            --output "$GameDir\artifacts\v$v"
    }
}

# --- Apply: reconstruct latest version from all bundles ---
Write-Host "`n=== Applying (v1 full + deltas -> v$Versions) ==="
$bundleDir = "$GameDir\applied\_bundles"
Remove-Item -Recurse -Force $bundleDir,"$GameDir\applied\latest" -ErrorAction SilentlyContinue
$null = mkdir -Force $bundleDir
for ($v = 1; $v -le $Versions; $v++) {
    Copy-Item "$GameDir\artifacts\v$v\bundle-000.gutch.bundle" "$bundleDir\v$v.gutch.bundle" -ErrorAction SilentlyContinue
}

cargo run -- reconstruct `
    -m "$GameDir\artifacts\v$Versions\version.gutch.json" `
    -f "$GameDir\artifacts\v$Versions" `
    -b $bundleDir `
    --output "$GameDir\applied\latest"

Remove-Item -Recurse -Force $bundleDir -ErrorAction SilentlyContinue

# --- Verify ---
Write-Host "`n=== Verifying ==="
cargo run -- verify `
    -m "$GameDir\artifacts\v$Versions\version.gutch.json" `
    -f "$GameDir\artifacts\v$Versions" `
    --dir "$GameDir\applied\latest"
if ($LASTEXITCODE -eq 0) {
    Write-Host "  gutch verify: PASSED"
} else {
    Write-Host "  gutch verify: FAILED"
    exit 1
}

Write-Host "`n=== Binary diff applied/latest vs truth/v$Versions ==="
$match = $true
Get-ChildItem "$GameDir\truth\v$Versions" -Recurse -File | ForEach-Object {
    $rel = $_.FullName.Replace("$GameDir\truth\v$Versions\", "")
    $appliedPath = "$GameDir\applied\latest\$rel"
    if (Test-Path $appliedPath) {
        $tHash = (Get-FileHash $_.FullName -Algorithm SHA256).Hash
        $aHash = (Get-FileHash $appliedPath -Algorithm SHA256).Hash
        if ($tHash -ne $aHash) {
            Write-Host "  MISMATCH: $rel"
            $match = $false
        } else {
            Write-Host "  OK: $rel"
        }
    } else {
        Write-Host "  MISSING: $rel"
        $match = $false
    }
}

if ($match) {
    Write-Host "`n=== SUCCESS ==="
    Pop-Location
    exit 0
} else {
    Write-Host "`n=== FAILED ==="
    Pop-Location
    exit 1
}
