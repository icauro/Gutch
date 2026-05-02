param(
    [string]$GameDir = "$PSScriptRoot\fixtures\mock-game-1",
    [int]$Versions = 0,
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$GameDir = if ([System.IO.Path]::IsPathRooted($GameDir)) {
    $GameDir
} else {
    Join-Path $PSScriptRoot $GameDir
}
Push-Location $PSScriptRoot\..

if (-not (Test-Path "$GameDir\truth\v1")) {
    Write-Host "No fixture found at $GameDir. Run create-mock.ps1 first."
    exit 1
}

$fixturePath = Join-Path $GameDir "fixture.json"
if (Test-Path $fixturePath) {
    $fixture = Get-Content $fixturePath -Raw | ConvertFrom-Json
    $product = [string]$fixture.ProductId
    $platform = [string]$fixture.Platform
    $architecture = [string]$fixture.Architecture
    $tier = [string]$fixture.TierId
    $versionIds = @($fixture.VersionIds)

    if ($Versions -le 0) {
        $Versions = [int]$fixture.Versions
    }

    if ($Versions -ne $versionIds.Count) {
        Write-Host "Requested Versions=$Versions but fixture metadata defines $($versionIds.Count) versions."
        exit 1
    }
} else {
    $truthVersions = @(Get-ChildItem (Join-Path $GameDir "truth") -Directory | Where-Object { $_.Name -match '^v\d+$' } | Sort-Object Name)
    if ($Versions -le 0) {
        $Versions = $truthVersions.Count
    }
    $product = Split-Path $GameDir -Leaf
    $platform = "windows"
    $architecture = "x86_64"
    $tier = "base"
    $versionIds = 1..$Versions | ForEach-Object { "$_.0.0" }
}

$artifactsRoot = Join-Path $GameDir "artifacts"
$productRoot = Join-Path $artifactsRoot "v1\products\$product"
$bundlesRoot = Join-Path $productRoot "bundles"
$latestVersionId = [string]$versionIds[-1]
$latestManifest = Join-Path $productRoot "targets\$latestVersionId\$platform\$architecture\$tier\manifest.json"

# --- Build ---
if (-not $SkipBuild) {
    Write-Host "=== Building artifacts ==="
    Remove-Item -Recurse -Force $artifactsRoot -ErrorAction SilentlyContinue
    for ($v = 1; $v -le $Versions; $v++) {
        $versionId = [string]$versionIds[$v - 1]
        Write-Host "  $versionId..."
        cargo run -- build `
            --dir "$GameDir\truth\v$v" `
            --product $product `
            --version $versionId `
            --platform $platform `
            --architecture $architecture `
            --tier $tier `
            --output $artifactsRoot
    }
}

# --- Apply: reconstruct latest version from shared bundle store ---
Write-Host "`n=== Applying latest target from shared bundles ==="
Remove-Item -Recurse -Force "$GameDir\applied\latest" -ErrorAction SilentlyContinue

cargo run -- reconstruct `
    -m $latestManifest `
    -b $bundlesRoot `
    --output "$GameDir\applied\latest"

# --- Verify ---
Write-Host "`n=== Verifying ==="
cargo run -- verify `
    -m $latestManifest `
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
