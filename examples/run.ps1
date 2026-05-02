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
$summaryPath = Join-Path $GameDir "run-summary.txt"
New-Item -ItemType Directory -Force -Path $GameDir | Out-Null
Remove-Item $summaryPath -ErrorAction SilentlyContinue
Push-Location $PSScriptRoot\..

if (-not (Test-Path "$GameDir\truth\v1")) {
    Write-Host "No fixture found at $GameDir. Run create-mock.ps1 first."
    Pop-Location
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
        Pop-Location
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
$verifyPassed = $false
$matchedFiles = 0
$mismatchedFiles = 0
$missingFiles = 0
$capturedSections = New-Object System.Collections.Generic.List[string]

function Invoke-CargoCapture {
    param(
        [string[]]$CargoArgs
    )

    $output = & cargo @CargoArgs 2>&1
    $exitCode = $LASTEXITCODE
    $lines = @($output | ForEach-Object { $_.ToString() })
    foreach ($line in $lines) {
        Write-Host $line
    }

    return @{
        ExitCode = $exitCode
        Lines = $lines
    }
}

function Get-InterestingCliLines {
    param(
        [string[]]$Lines
    )

    $interesting = New-Object System.Collections.Generic.List[string]
    foreach ($line in $Lines) {
        if (
            $line -match '^wrote target manifest:' -or
            $line -match '^wrote bundle:' -or
            $line -match '^done\.' -or
            $line -match '^estimate:' -or
            $line -match '^reconstructed to:' -or
            $line -match '^verification passed for ' -or
            $line -match '^timings:' -or
            $line -match '^\s{2}[A-Za-z0-9]'
        ) {
            $interesting.Add($line)
        }
    }

    return $interesting
}

# --- Build ---
if (-not $SkipBuild) {
    Write-Host "=== Building artifacts ==="
    Remove-Item -Recurse -Force $artifactsRoot -ErrorAction SilentlyContinue
    for ($v = 1; $v -le $Versions; $v++) {
        $versionId = [string]$versionIds[$v - 1]
        Write-Host "  $versionId..."
        $buildResult = Invoke-CargoCapture @(
            "run", "--", "build",
            "--dir", "$GameDir\truth\v$v",
            "--product", $product,
            "--version", $versionId,
            "--platform", $platform,
            "--architecture", $architecture,
            "--tier", $tier,
            "--output", $artifactsRoot
        )
        if ($buildResult.ExitCode -ne 0) {
            Pop-Location
            exit $buildResult.ExitCode
        }
        $capturedSections.Add("[build $versionId]")
        foreach ($line in (Get-InterestingCliLines $buildResult.Lines)) {
            $capturedSections.Add($line)
        }
        $capturedSections.Add("")
    }
}

# --- Apply: reconstruct latest version from shared bundle store ---
Write-Host "`n=== Applying latest target from shared bundles ==="
Remove-Item -Recurse -Force "$GameDir\applied\latest" -ErrorAction SilentlyContinue

$reconstructResult = Invoke-CargoCapture @(
    "run", "--", "reconstruct",
    "-m", $latestManifest,
    "-b", $bundlesRoot,
    "--output", "$GameDir\applied\latest"
)
if ($reconstructResult.ExitCode -ne 0) {
    Pop-Location
    exit $reconstructResult.ExitCode
}
$capturedSections.Add("[reconstruct]")
foreach ($line in (Get-InterestingCliLines $reconstructResult.Lines)) {
    $capturedSections.Add($line)
}
$capturedSections.Add("")

# --- Verify ---
Write-Host "`n=== Verifying ==="
$verifyResult = Invoke-CargoCapture @(
    "run", "--", "verify",
    "-m", $latestManifest,
    "--dir", "$GameDir\applied\latest"
)
if ($verifyResult.ExitCode -eq 0) {
    Write-Host "  gutch verify: PASSED"
    $verifyPassed = $true
} else {
    Write-Host "  gutch verify: FAILED"
    Pop-Location
    exit $verifyResult.ExitCode
}
$capturedSections.Add("[verify]")
foreach ($line in (Get-InterestingCliLines $verifyResult.Lines)) {
    $capturedSections.Add($line)
}
$capturedSections.Add("  gutch verify: $(if ($verifyPassed) { 'PASSED' } else { 'FAILED' })")
$capturedSections.Add("")

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
            $mismatchedFiles++
        } else {
            Write-Host "  OK: $rel"
            $matchedFiles++
        }
    } else {
        Write-Host "  MISSING: $rel"
        $match = $false
        $missingFiles++
    }
}

$bundleFiles = @(Get-ChildItem $bundlesRoot -Recurse -File -Filter *.bundle -ErrorAction SilentlyContinue)
$bundleCount = $bundleFiles.Count
$bundleBytes = ($bundleFiles | Measure-Object -Property Length -Sum).Sum
if ($null -eq $bundleBytes) { $bundleBytes = 0 }
$truthFiles = @(Get-ChildItem "$GameDir\truth\v$Versions" -Recurse -File)
$appliedFiles = @(Get-ChildItem "$GameDir\applied\latest" -Recurse -File -ErrorAction SilentlyContinue)
$manifestBytes = if (Test-Path $latestManifest) { (Get-Item $latestManifest).Length } else { 0 }
$summaryLines = @(
    "fixture: $GameDir"
    "product: $product"
    "target: $latestVersionId/$platform/$architecture/$tier"
    "versions: $Versions"
    "verify: $(if ($verifyPassed) { 'PASSED' } else { 'FAILED' })"
    "diff: $(if ($match) { 'MATCH' } else { 'FAILED' })"
    "truth_files: $($truthFiles.Count)"
    "applied_files: $($appliedFiles.Count)"
    "matched_files: $matchedFiles"
    "mismatched_files: $mismatchedFiles"
    "missing_files: $missingFiles"
    "bundle_count: $bundleCount"
    "bundle_bytes: $bundleBytes"
    "manifest_path: $latestManifest"
    "manifest_bytes: $manifestBytes"
)
$summaryLines += ""
$summaryLines += "[gutch-cli]"
$summaryLines += $capturedSections
Set-Content -Path $summaryPath -Value $summaryLines

if ($match) {
    Write-Host "`n=== SUCCESS ==="
    Pop-Location
    exit 0
} else {
    Write-Host "`n=== FAILED ==="
    Pop-Location
    exit 1
}
