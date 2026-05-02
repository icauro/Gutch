param(
    [string]$GameDir,
    [int]$Versions = 0,
    [switch]$SkipBuild,
    [Alias('AllTests')]
    [switch]$AllFixtures
)

$ErrorActionPreference = "Stop"

function Select-Fixture {
    param(
        [string]$FixturesRoot
    )

    $fixtures = Get-Fixtures -FixturesRoot $FixturesRoot

    if ($fixtures.Count -eq 0) {
        Write-Host "No fixtures found under $FixturesRoot. Run create-mock.ps1 first."
        exit 1
    }

    Write-Host "Available fixtures:"
    for ($i = 0; $i -lt $fixtures.Count; $i++) {
        $fixturePath = Join-Path $fixtures[$i].FullName "fixture.json"
        $details = ""
        if (Test-Path $fixturePath) {
            $fixture = Get-Content $fixturePath -Raw | ConvertFrom-Json
            $details = " ($($fixture.SizeMB) MB, $($fixture.Versions) versions, $($fixture.ChangePercent)% change)"
        }
        Write-Host ("  [{0}] {1}{2}" -f ($i + 1), $fixtures[$i].Name, $details)
    }

    while ($true) {
        $choice = Read-Host "Fixture to run [1]"
        if ([string]::IsNullOrWhiteSpace($choice)) {
            return $fixtures[0].FullName
        }

        $index = 0
        if ([int]::TryParse($choice, [ref]$index) -and $index -ge 1 -and $index -le $fixtures.Count) {
            return $fixtures[$index - 1].FullName
        }

        $named = $fixtures | Where-Object { $_.Name -eq $choice } | Select-Object -First 1
        if ($named) {
            return $named.FullName
        }

        Write-Host "Enter a number from 1 to $($fixtures.Count), or an exact fixture name."
    }
}

function Get-Fixtures {
    param(
        [string]$FixturesRoot
    )

    $fixtures = @(Get-ChildItem $FixturesRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object { Test-Path (Join-Path $_.FullName "truth\v1") } |
        Sort-Object Name)
    return $fixtures
}

function Get-PowerShellRunner {
    $pwsh = Get-Command pwsh -ErrorAction SilentlyContinue
    if ($pwsh) {
        return $pwsh.Source
    }

    $powershell = Get-Command powershell -ErrorAction SilentlyContinue
    if ($powershell) {
        return $powershell.Source
    }

    throw "Neither pwsh nor powershell was found."
}

if ($AllFixtures) {
    $fixtures = Get-Fixtures -FixturesRoot (Join-Path $PSScriptRoot "fixtures")
    if ($fixtures.Count -eq 0) {
        Write-Host "No fixtures found under $(Join-Path $PSScriptRoot "fixtures"). Run create-mock.ps1 first."
        exit 1
    }

    $runner = Get-PowerShellRunner
    $failedFixtures = New-Object System.Collections.Generic.List[string]

    Write-Host "Running all fixtures:"
    for ($i = 0; $i -lt $fixtures.Count; $i++) {
        $fixture = $fixtures[$i]
        Write-Host ("  [{0}/{1}] {2}" -f ($i + 1), $fixtures.Count, $fixture.Name)

        $invokeArgs = @(
            "-NoLogo",
            "-NoProfile",
            "-File",
            $PSCommandPath,
            "-GameDir",
            $fixture.FullName
        )
        if ($Versions -gt 0) {
            $invokeArgs += @("-Versions", "$Versions")
        }
        if ($SkipBuild) {
            $invokeArgs += "-SkipBuild"
        }

        & $runner @invokeArgs
        if ($LASTEXITCODE -ne 0) {
            $failedFixtures.Add($fixture.Name)
        }
    }

    if ($failedFixtures.Count -gt 0) {
        Write-Host "Failed fixtures: $($failedFixtures -join ', ')"
        exit 1
    }

    Write-Host "All fixtures passed."
    exit 0
}

if ([string]::IsNullOrWhiteSpace($GameDir)) {
    $GameDir = Select-Fixture -FixturesRoot (Join-Path $PSScriptRoot "fixtures")
}

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

$artifactsRoot = Join-Path $GameDir ".artifacts"
$productRoot = Join-Path $artifactsRoot "v1\products\$product"
$bundlesRoot = Join-Path $productRoot "bundles"
$latestVersionId = [string]$versionIds[-1]
$latestManifest = Join-Path $productRoot "targets\$latestVersionId\$platform\$architecture\$tier\manifest.json"
$verifyPassed = $false
$matchedFiles = 0
$mismatchedFiles = 0
$missingFiles = 0
$capturedSections = New-Object System.Collections.Generic.List[string]
$targetStats = New-Object System.Collections.Generic.List[object]
$releaseExe = Join-Path $PSScriptRoot "..\target\release\gutch.exe"

function Invoke-CargoCapture {
    param(
        [string]$ExePath,
        [string[]]$NativeArgs
    )

    $output = & $ExePath @NativeArgs 2>&1
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

function Format-Pct {
    param([double]$Value)
    if ($Value -eq [double]::NaN) { return "0.00%" }
    return ("{0:N2}%" -f $Value)
}

function Format-Int {
    param([int64]$Value)
    return ("{0:N0}" -f $Value)
}

function Format-Size {
    param([int64]$Bytes)

    if ($Bytes -ge 1TB) {
        return ("{0:N2} TiB" -f ($Bytes / 1TB))
    } elseif ($Bytes -ge 1GB) {
        return ("{0:N2} GiB" -f ($Bytes / 1GB))
    } elseif ($Bytes -ge 1MB) {
        return ("{0:N2} MiB" -f ($Bytes / 1MB))
    } elseif ($Bytes -ge 1KB) {
        return ("{0:N2} KiB" -f ($Bytes / 1KB))
    } else {
        return ("{0} B" -f $Bytes)
    }
}

Write-Host "=== Building release binary ==="
$releaseBuild = & cargo build --release 2>&1
$releaseBuildExit = $LASTEXITCODE
foreach ($line in @($releaseBuild | ForEach-Object { $_.ToString() })) {
    Write-Host $line
}
if ($releaseBuildExit -ne 0) {
    Pop-Location
    exit $releaseBuildExit
}
if (-not (Test-Path $releaseExe)) {
    Write-Host "Release binary was not found at $releaseExe after cargo build --release."
    Pop-Location
    exit 1
}

# --- Build ---
if (-not $SkipBuild) {
    Write-Host "=== Building artifacts ==="
    Remove-Item -Recurse -Force $artifactsRoot -ErrorAction SilentlyContinue
    for ($v = 1; $v -le $Versions; $v++) {
        $versionId = [string]$versionIds[$v - 1]
        Write-Host "  $versionId..."
        $previousChunkIds = New-Object 'System.Collections.Generic.HashSet[string]'
        if (Test-Path (Join-Path $productRoot "targets")) {
            Get-ChildItem (Join-Path $productRoot "targets") -Recurse -File -Filter manifest.json -ErrorAction SilentlyContinue | ForEach-Object {
                $previousManifest = Get-Content $_.FullName -Raw | ConvertFrom-Json
                foreach ($chunkId in @($previousManifest.required_chunk_set)) {
                    [void]$previousChunkIds.Add([string]$chunkId)
                }
            }
        }

        $buildResult = Invoke-CargoCapture -ExePath $releaseExe -NativeArgs @(
            "build",
            "--dir", "$GameDir\truth\v$v",
            "--product", $product,
            "--version", $versionId,
            "--platform", $platform,
            "--architecture", $architecture,
            "--tier", $tier,
            "--chunk-profile", "auto",
            "--bundle-profile", "auto",
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

        $manifestPath = Join-Path $productRoot "targets\$versionId\$platform\$architecture\$tier\manifest.json"
        $manifest = $null
        $requiredChunkCount = 0
        $requiredBundleCount = 0
        if (Test-Path $manifestPath) {
            $manifest = Get-Content $manifestPath -Raw | ConvertFrom-Json
            $requiredChunkCount = @($manifest.required_chunk_set).Count
            $requiredBundleCount = @($manifest.required_bundle_set).Count
        }

        $generatedBundleCount = @($buildResult.Lines | Where-Object { $_ -match '^wrote bundle:' }).Count
        $generatedBundlePct = if ($requiredBundleCount -gt 0) {
            [math]::Round(($generatedBundleCount * 100.0) / $requiredBundleCount, 2)
        } else {
            0.0
        }
        $reusedBundleCount = if ($requiredBundleCount -gt $generatedBundleCount) {
            $requiredBundleCount - $generatedBundleCount
        } else {
            0
        }
        $reusedBundlePct = if ($requiredBundleCount -gt 0) {
            [math]::Round(($reusedBundleCount * 100.0) / $requiredBundleCount, 2)
        } else {
            0.0
        }

        $reusedChunkCount = 0
        if ($manifest) {
            foreach ($chunkId in @($manifest.required_chunk_set)) {
                if ($previousChunkIds.Contains([string]$chunkId)) {
                    $reusedChunkCount++
                }
            }
        }
        $newChunkCount = $requiredChunkCount - $reusedChunkCount
        $reusedChunkPct = if ($requiredChunkCount -gt 0) {
            [math]::Round(($reusedChunkCount * 100.0) / $requiredChunkCount, 2)
        } else {
            0.0
        }

        $targetStats.Add([pscustomobject]@{
            VersionId = $versionId
            ChunkCount = $requiredChunkCount
            NewChunkCount = $newChunkCount
            ReusedChunkCount = $reusedChunkCount
            SkippedChunkCount = $reusedChunkCount
            ReusedChunkPercent = $reusedChunkPct
            RequiredBundleCount = $requiredBundleCount
            GeneratedBundleCount = $generatedBundleCount
            GeneratedBundlePercent = $generatedBundlePct
            ReusedBundleCount = $reusedBundleCount
            ReusedBundlePercent = $reusedBundlePct
        })
    }
}

# --- Apply: reconstruct latest version from shared bundle store ---
Write-Host "`n=== Applying latest target from shared bundles ==="
Remove-Item -Recurse -Force "$GameDir\applied\latest" -ErrorAction SilentlyContinue

$reconstructResult = Invoke-CargoCapture -ExePath $releaseExe -NativeArgs @(
    "reconstruct",
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
$verifyResult = Invoke-CargoCapture -ExePath $releaseExe -NativeArgs @(
    "verify",
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

$totalChunksGenerated = 0
$totalChunksNew = 0
$totalChunksReused = 0
$totalBundlesRequired = 0
$totalBundlesGenerated = 0
$totalBundlesReused = 0
foreach ($stat in $targetStats) {
    $totalChunksGenerated += [int64]$stat.ChunkCount
    $totalChunksNew += [int64]$stat.NewChunkCount
    $totalChunksReused += [int64]$stat.ReusedChunkCount
    $totalBundlesRequired += [int64]$stat.RequiredBundleCount
    $totalBundlesGenerated += [int64]$stat.GeneratedBundleCount
    $totalBundlesReused += [int64]$stat.ReusedBundleCount
}

$overallChunkReusePct = if ($totalChunksGenerated -gt 0) { [math]::Round(($totalChunksReused * 100.0) / $totalChunksGenerated, 2) } else { 0.0 }
$overallBundleReusePct = if ($totalBundlesRequired -gt 0) { [math]::Round(($totalBundlesReused * 100.0) / $totalBundlesRequired, 2) } else { 0.0 }
$overallBundleRatio = if ($totalBundlesRequired -gt 0) { "{0}/{1}" -f $totalBundlesGenerated, $totalBundlesRequired } else { "0/0" }

$summaryLines = New-Object System.Collections.Generic.List[string]
$summaryLines.Add("fixture: $GameDir")
$summaryLines.Add("product: $product")
$summaryLines.Add("target: $latestVersionId/$platform/$architecture/$tier")
$summaryLines.Add("versions: $Versions")
$summaryLines.Add("status: verify=$(if ($verifyPassed) { 'PASSED' } else { 'FAILED' }), diff=$(if ($match) { 'MATCH' } else { 'FAILED' })")
$summaryLines.Add(("files: truth={0}, applied={1}, matched={2}, mismatched={3}, missing={4}" -f $truthFiles.Count, $appliedFiles.Count, $matchedFiles, $mismatchedFiles, $missingFiles))
$summaryLines.Add(("artifacts: bundles={0}, bundle_size={1}, manifest_size={2}" -f $bundleCount, (Format-Size $bundleBytes), (Format-Size $manifestBytes)))
$summaryLines.Add(("totals: chunks={0}, new={1}, reused={2}, chunk_reuse={3}; bundles_required={4}, generated={5}, reused={6}, bundle_reuse={7}, bundle_ratio={8}" -f `
    (Format-Int $totalChunksGenerated), (Format-Int $totalChunksNew), (Format-Int $totalChunksReused), (Format-Pct $overallChunkReusePct), `
    (Format-Int $totalBundlesRequired), (Format-Int $totalBundlesGenerated), (Format-Int $totalBundlesReused), (Format-Pct $overallBundleReusePct), $overallBundleRatio))
$summaryLines.Add(("manifest: {0}" -f $latestManifest))
$summaryLines.Add("")
$summaryLines.Add("[target-stats]")
$summaryLines.Add("target | chunks gen/new/reused | chunk reuse | bundles req/gen/reused | bundle reuse")
foreach ($stat in $targetStats) {
    $summaryLines.Add(("{0} | {1}/{2}/{3} | {4} | {5}/{6}/{7} | {8}" -f `
        "$($stat.VersionId)/$platform/$architecture/$tier",
        $stat.ChunkCount,
        $stat.NewChunkCount,
        $stat.ReusedChunkCount,
        $stat.ReusedChunkPercent,
        $stat.RequiredBundleCount,
        $stat.GeneratedBundleCount,
        $stat.ReusedBundleCount,
        $stat.ReusedBundlePercent))
}
$summaryLines.Add("")
$summaryLines.Add("[gutch-cli]")
$summaryLines.AddRange($capturedSections)
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
