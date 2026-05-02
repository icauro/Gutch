param(
    [string]$GameDir,
    [int]$Versions = 0,
    [switch]$SkipBuild,
    [Alias('AllTests')]
    [switch]$AllFixtures
)

$ErrorActionPreference = "Stop"

function Get-Fixtures {
    param(
        [string]$FixturesRoot
    )

    $fixtures = @(Get-ChildItem $FixturesRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object { Test-Path (Join-Path $_.FullName "truth\v1") } |
        Sort-Object Name)
    return $fixtures
}

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

function Format-SignedInt {
    param([int64]$Value)
    if ($Value -gt 0) {
        return ("+{0:N0}" -f $Value)
    }
    return ("{0:N0}" -f $Value)
}

function Format-SignedSize {
    param([int64]$Bytes)
    $sign = ""
    $absBytes = $Bytes
    if ($Bytes -gt 0) {
        $sign = "+"
    } elseif ($Bytes -lt 0) {
        $absBytes = -1 * $Bytes
        $sign = "-"
    }
    return "$sign$(Format-Size $absBytes)"
}

function Format-SignedPctPoints {
    param([double]$Value)
    if ($Value -gt 0) {
        return ("+{0:N2} pp" -f $Value)
    }
    return ("{0:N2} pp" -f $Value)
}

function Get-DirectorySizeBytes {
    param(
        [string]$Path
    )

    if (-not (Test-Path $Path)) {
        return [int64]0
    }

    $sum = (Get-ChildItem $Path -Recurse -File -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
    if ($null -eq $sum) {
        return [int64]0
    }

    return [int64]$sum
}

function Get-FixtureConfig {
    param(
        [string]$FixtureDir,
        [int]$RequestedVersions
    )

    $fixturePath = Join-Path $FixtureDir "fixture.json"
    if (Test-Path $fixturePath) {
        $fixture = Get-Content $fixturePath -Raw | ConvertFrom-Json
        $versionIds = @($fixture.VersionIds)
        $resolvedVersions = $RequestedVersions
        if ($resolvedVersions -le 0) {
            $resolvedVersions = [int]$fixture.Versions
        }
        if ($resolvedVersions -ne $versionIds.Count) {
            throw "Requested Versions=$resolvedVersions but fixture metadata defines $($versionIds.Count) versions."
        }

        return [pscustomobject]@{
            Product = [string]$fixture.ProductId
            Platform = [string]$fixture.Platform
            Architecture = [string]$fixture.Architecture
            Tier = [string]$fixture.TierId
            Versions = $resolvedVersions
            VersionIds = $versionIds
        }
    }

    $truthVersions = @(Get-ChildItem (Join-Path $FixtureDir "truth") -Directory | Where-Object { $_.Name -match '^v\d+$' } | Sort-Object Name)
    $resolvedVersions = $RequestedVersions
    if ($resolvedVersions -le 0) {
        $resolvedVersions = $truthVersions.Count
    }

    return [pscustomobject]@{
        Product = Split-Path $FixtureDir -Leaf
        Platform = "windows"
        Architecture = "x86_64"
        Tier = "base"
        Versions = $resolvedVersions
        VersionIds = 1..$resolvedVersions | ForEach-Object { "$_.0.0" }
    }
}

function Invoke-CompressionRun {
    param(
        [string]$CompressionUnit,
        [string]$FixtureDir,
        [pscustomobject]$FixtureConfig,
        [string]$CompareRoot,
        [string]$ReleaseExe,
        [switch]$SkipBuild
    )

    $modeRoot = Join-Path $CompareRoot $CompressionUnit
    $artifactsRoot = Join-Path $modeRoot "artifacts"
    $appliedRoot = Join-Path $modeRoot "applied\latest"
    $summaryPath = Join-Path $modeRoot "run-summary.txt"
    $capturedSections = New-Object System.Collections.Generic.List[string]
    $targetStats = New-Object System.Collections.Generic.List[object]

    $productRoot = Join-Path $artifactsRoot "v1\products\$($FixtureConfig.Product)"
    $bundlesRoot = Join-Path $productRoot "bundles"
    $latestVersionId = [string]$FixtureConfig.VersionIds[-1]
    $latestManifest = Join-Path $productRoot "targets\$latestVersionId\$($FixtureConfig.Platform)\$($FixtureConfig.Architecture)\$($FixtureConfig.Tier)\manifest.json"
    $verifyPassed = $false
    $matchedFiles = 0
    $mismatchedFiles = 0
    $missingFiles = 0

    if (-not $SkipBuild) {
        Remove-Item -Recurse -Force $modeRoot -ErrorAction SilentlyContinue
    } else {
        Remove-Item -Recurse -Force $appliedRoot -ErrorAction SilentlyContinue
    }
    New-Item -ItemType Directory -Force -Path $modeRoot | Out-Null

    if (-not $SkipBuild) {
        Write-Host "=== [$CompressionUnit] Building artifacts ==="
        for ($v = 1; $v -le $FixtureConfig.Versions; $v++) {
            $versionId = [string]$FixtureConfig.VersionIds[$v - 1]
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

            $buildResult = Invoke-CargoCapture -ExePath $ReleaseExe -NativeArgs @(
                "build",
                "--dir", "$FixtureDir\truth\v$v",
                "--product", $FixtureConfig.Product,
                "--version", $versionId,
                "--platform", $FixtureConfig.Platform,
                "--architecture", $FixtureConfig.Architecture,
                "--tier", $FixtureConfig.Tier,
                "--chunk-profile", "auto",
                "--bundle-profile", "auto",
                "--compression-unit", $CompressionUnit,
                "--output", $artifactsRoot
            )
            if ($buildResult.ExitCode -ne 0) {
                throw "gutch build failed for $CompressionUnit / $versionId"
            }

            $capturedSections.Add("[build $versionId]")
            foreach ($line in (Get-InterestingCliLines $buildResult.Lines)) {
                $capturedSections.Add($line)
            }
            $capturedSections.Add("")

            $manifestPath = Join-Path $productRoot "targets\$versionId\$($FixtureConfig.Platform)\$($FixtureConfig.Architecture)\$($FixtureConfig.Tier)\manifest.json"
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
                ReusedChunkPercent = $reusedChunkPct
                RequiredBundleCount = $requiredBundleCount
                GeneratedBundleCount = $generatedBundleCount
                GeneratedBundlePercent = $generatedBundlePct
                ReusedBundleCount = $reusedBundleCount
                ReusedBundlePercent = $reusedBundlePct
            })
        }
    }

    if (-not (Test-Path $latestManifest)) {
        throw "Latest manifest not found for $CompressionUnit at $latestManifest"
    }

    Write-Host ""
    Write-Host "=== [$CompressionUnit] Reconstructing latest target ==="
    $reconstructResult = Invoke-CargoCapture -ExePath $ReleaseExe -NativeArgs @(
        "reconstruct",
        "-m", $latestManifest,
        "-b", $bundlesRoot,
        "--output", $appliedRoot
    )
    if ($reconstructResult.ExitCode -ne 0) {
        throw "gutch reconstruct failed for $CompressionUnit"
    }
    $capturedSections.Add("[reconstruct]")
    foreach ($line in (Get-InterestingCliLines $reconstructResult.Lines)) {
        $capturedSections.Add($line)
    }
    $capturedSections.Add("")

    Write-Host ""
    Write-Host "=== [$CompressionUnit] Verifying ==="
    $verifyResult = Invoke-CargoCapture -ExePath $ReleaseExe -NativeArgs @(
        "verify",
        "-m", $latestManifest,
        "--dir", $appliedRoot
    )
    if ($verifyResult.ExitCode -ne 0) {
        throw "gutch verify failed for $CompressionUnit"
    }
    $verifyPassed = $true
    $capturedSections.Add("[verify]")
    foreach ($line in (Get-InterestingCliLines $verifyResult.Lines)) {
        $capturedSections.Add($line)
    }
    $capturedSections.Add("  gutch verify: PASSED")
    $capturedSections.Add("")

    Write-Host ""
    Write-Host "=== [$CompressionUnit] Binary diff applied/latest vs truth/v$($FixtureConfig.Versions) ==="
    $match = $true
    Get-ChildItem "$FixtureDir\truth\v$($FixtureConfig.Versions)" -Recurse -File | ForEach-Object {
        $rel = $_.FullName.Replace("$FixtureDir\truth\v$($FixtureConfig.Versions)\", "")
        $appliedPath = Join-Path $appliedRoot $rel
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

    $latestManifestJson = Get-Content $latestManifest -Raw | ConvertFrom-Json
    $bundleFiles = @(Get-ChildItem $bundlesRoot -Recurse -File -Filter *.bundle -ErrorAction SilentlyContinue)
    $bundleCount = $bundleFiles.Count
    $bundleBytes = ($bundleFiles | Measure-Object -Property Length -Sum).Sum
    if ($null -eq $bundleBytes) { $bundleBytes = 0 }
    $artifactBytes = Get-DirectorySizeBytes -Path $artifactsRoot
    $truthFiles = @(Get-ChildItem "$FixtureDir\truth\v$($FixtureConfig.Versions)" -Recurse -File)
    $appliedFiles = @(Get-ChildItem $appliedRoot -Recurse -File -ErrorAction SilentlyContinue)
    $manifestBytes = (Get-Item $latestManifest).Length

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

    $summaryLines = New-Object System.Collections.Generic.List[string]
    $summaryLines.Add("fixture: $FixtureDir")
    $summaryLines.Add("compression_unit: $CompressionUnit")
    $summaryLines.Add("product: $($FixtureConfig.Product)")
    $summaryLines.Add("target: $latestVersionId/$($FixtureConfig.Platform)/$($FixtureConfig.Architecture)/$($FixtureConfig.Tier)")
    $summaryLines.Add("versions: $($FixtureConfig.Versions)")
    $summaryLines.Add("status: verify=$(if ($verifyPassed) { 'PASSED' } else { 'FAILED' }), diff=$(if ($match) { 'MATCH' } else { 'FAILED' })")
    $summaryLines.Add(("files: truth={0}, applied={1}, matched={2}, mismatched={3}, missing={4}" -f $truthFiles.Count, $appliedFiles.Count, $matchedFiles, $mismatchedFiles, $missingFiles))
    $summaryLines.Add(("artifacts: total_size={0}, bundles={1}, bundle_size={2}, manifest_size={3}" -f (Format-Size $artifactBytes), $bundleCount, (Format-Size $bundleBytes), (Format-Size $manifestBytes)))
    $summaryLines.Add(("manifest-estimate: logical={0}, compressed_estimate={1}, required_chunks={2}, required_bundles={3}" -f `
        (Format-Size ([int64]$latestManifestJson.total_logical_size)),
        (Format-Size ([int64]$latestManifestJson.total_compressed_size_estimate)),
        (Format-Int @($latestManifestJson.required_chunk_set).Count),
        (Format-Int @($latestManifestJson.required_bundle_set).Count)))
    $summaryLines.Add(("totals: chunks={0}, new={1}, reused={2}, chunk_reuse={3}; bundles_required={4}, generated={5}, reused={6}, bundle_reuse={7}" -f `
        (Format-Int $totalChunksGenerated), (Format-Int $totalChunksNew), (Format-Int $totalChunksReused), (Format-Pct $overallChunkReusePct), `
        (Format-Int $totalBundlesRequired), (Format-Int $totalBundlesGenerated), (Format-Int $totalBundlesReused), (Format-Pct $overallBundleReusePct)))
    $summaryLines.Add(("manifest: {0}" -f $latestManifest))
    $summaryLines.Add("")
    $summaryLines.Add("[target-stats]")
    $summaryLines.Add("target | chunks gen/new/reused | chunk reuse | bundles req/gen/reused | bundle reuse")
    foreach ($stat in $targetStats) {
        $summaryLines.Add(("{0} | {1}/{2}/{3} | {4} | {5}/{6}/{7} | {8}" -f `
            "$($stat.VersionId)/$($FixtureConfig.Platform)/$($FixtureConfig.Architecture)/$($FixtureConfig.Tier)",
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

    return [pscustomobject]@{
        CompressionUnit = $CompressionUnit
        SummaryPath = $summaryPath
        ManifestPath = $latestManifest
        Product = $FixtureConfig.Product
        Versions = $FixtureConfig.Versions
        VerifyPassed = $verifyPassed
        MatchPassed = $match
        ArtifactBytes = [int64]$artifactBytes
        BundleCount = [int64]$bundleCount
        BundleBytes = [int64]$bundleBytes
        ManifestBytes = [int64]$manifestBytes
        TotalLogicalSize = [int64]$latestManifestJson.total_logical_size
        TotalCompressedEstimate = [int64]$latestManifestJson.total_compressed_size_estimate
        RequiredChunkCount = [int64]@($latestManifestJson.required_chunk_set).Count
        RequiredBundleCount = [int64]@($latestManifestJson.required_bundle_set).Count
        TotalChunksGenerated = [int64]$totalChunksGenerated
        TotalChunksNew = [int64]$totalChunksNew
        TotalChunksReused = [int64]$totalChunksReused
        TotalBundlesRequired = [int64]$totalBundlesRequired
        TotalBundlesGenerated = [int64]$totalBundlesGenerated
        TotalBundlesReused = [int64]$totalBundlesReused
        OverallChunkReusePct = [double]$overallChunkReusePct
        OverallBundleReusePct = [double]$overallBundleReusePct
    }
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

Push-Location $PSScriptRoot\..

if (-not (Test-Path "$GameDir\truth\v1")) {
    Write-Host "No fixture found at $GameDir. Run create-mock.ps1 first."
    Pop-Location
    exit 1
}

$fixtureConfig = Get-FixtureConfig -FixtureDir $GameDir -RequestedVersions $Versions
$compareRoot = Join-Path $GameDir ".compare_compress"
$compareSummaryPath = Join-Path $compareRoot "compare-summary.txt"
$releaseExe = Join-Path $PSScriptRoot "..\target\release\gutch.exe"

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

New-Item -ItemType Directory -Force -Path $compareRoot | Out-Null
Remove-Item $compareSummaryPath -ErrorAction SilentlyContinue

try {
    $chunkResult = Invoke-CompressionRun -CompressionUnit "chunk" -FixtureDir $GameDir -FixtureConfig $fixtureConfig -CompareRoot $compareRoot -ReleaseExe $releaseExe -SkipBuild:$SkipBuild
    Write-Host ""
    $bundleResult = Invoke-CompressionRun -CompressionUnit "bundle" -FixtureDir $GameDir -FixtureConfig $fixtureConfig -CompareRoot $compareRoot -ReleaseExe $releaseExe -SkipBuild:$SkipBuild
} catch {
    Write-Host $_
    Pop-Location
    exit 1
}

$compareLines = New-Object System.Collections.Generic.List[string]
$compareLines.Add("fixture: $GameDir")
$compareLines.Add("product: $($fixtureConfig.Product)")
$compareLines.Add("target: $($fixtureConfig.VersionIds[-1])/$($fixtureConfig.Platform)/$($fixtureConfig.Architecture)/$($fixtureConfig.Tier)")
$compareLines.Add("versions: $($fixtureConfig.Versions)")
$compareLines.Add("compare_root: $compareRoot")
$compareLines.Add("")
$compareLines.Add("[mode chunk]")
$compareLines.Add(("status: verify={0}, diff={1}" -f $(if ($chunkResult.VerifyPassed) { 'PASSED' } else { 'FAILED' }), $(if ($chunkResult.MatchPassed) { 'MATCH' } else { 'FAILED' })))
$compareLines.Add(("artifacts: total_size={0}" -f (Format-Size $chunkResult.ArtifactBytes)))
$compareLines.Add(("bundles: count={0}, size={1}" -f (Format-Int $chunkResult.BundleCount), (Format-Size $chunkResult.BundleBytes)))
$compareLines.Add(("manifest: file={0}, logical={1}, compressed_estimate={2}, required_chunks={3}, required_bundles={4}" -f `
    (Format-Size $chunkResult.ManifestBytes), (Format-Size $chunkResult.TotalLogicalSize), (Format-Size $chunkResult.TotalCompressedEstimate), `
    (Format-Int $chunkResult.RequiredChunkCount), (Format-Int $chunkResult.RequiredBundleCount)))
$compareLines.Add(("reuse: chunks new={0}, reused={1}, chunk_reuse={2}; bundles generated={3}, reused={4}, bundle_reuse={5}" -f `
    (Format-Int $chunkResult.TotalChunksNew), (Format-Int $chunkResult.TotalChunksReused), (Format-Pct $chunkResult.OverallChunkReusePct), `
    (Format-Int $chunkResult.TotalBundlesGenerated), (Format-Int $chunkResult.TotalBundlesReused), (Format-Pct $chunkResult.OverallBundleReusePct)))
$compareLines.Add(("summary: {0}" -f $chunkResult.SummaryPath))
$compareLines.Add("")
$compareLines.Add("[mode bundle]")
$compareLines.Add(("status: verify={0}, diff={1}" -f $(if ($bundleResult.VerifyPassed) { 'PASSED' } else { 'FAILED' }), $(if ($bundleResult.MatchPassed) { 'MATCH' } else { 'FAILED' })))
$compareLines.Add(("artifacts: total_size={0}" -f (Format-Size $bundleResult.ArtifactBytes)))
$compareLines.Add(("bundles: count={0}, size={1}" -f (Format-Int $bundleResult.BundleCount), (Format-Size $bundleResult.BundleBytes)))
$compareLines.Add(("manifest: file={0}, logical={1}, compressed_estimate={2}, required_chunks={3}, required_bundles={4}" -f `
    (Format-Size $bundleResult.ManifestBytes), (Format-Size $bundleResult.TotalLogicalSize), (Format-Size $bundleResult.TotalCompressedEstimate), `
    (Format-Int $bundleResult.RequiredChunkCount), (Format-Int $bundleResult.RequiredBundleCount)))
$compareLines.Add(("reuse: chunks new={0}, reused={1}, chunk_reuse={2}; bundles generated={3}, reused={4}, bundle_reuse={5}" -f `
    (Format-Int $bundleResult.TotalChunksNew), (Format-Int $bundleResult.TotalChunksReused), (Format-Pct $bundleResult.OverallChunkReusePct), `
    (Format-Int $bundleResult.TotalBundlesGenerated), (Format-Int $bundleResult.TotalBundlesReused), (Format-Pct $bundleResult.OverallBundleReusePct)))
$compareLines.Add(("summary: {0}" -f $bundleResult.SummaryPath))
$compareLines.Add("")
$compareLines.Add("[differences bundle - chunk]")
$compareLines.Add(("artifact_total_size_delta: {0}" -f (Format-SignedSize ($bundleResult.ArtifactBytes - $chunkResult.ArtifactBytes))))
$compareLines.Add(("bundle_count_delta: {0}" -f (Format-SignedInt ($bundleResult.BundleCount - $chunkResult.BundleCount))))
$compareLines.Add(("bundle_size_delta: {0}" -f (Format-SignedSize ($bundleResult.BundleBytes - $chunkResult.BundleBytes))))
$compareLines.Add(("manifest_file_size_delta: {0}" -f (Format-SignedSize ($bundleResult.ManifestBytes - $chunkResult.ManifestBytes))))
$compareLines.Add(("manifest_compressed_estimate_delta: {0}" -f (Format-SignedSize ($bundleResult.TotalCompressedEstimate - $chunkResult.TotalCompressedEstimate))))
$compareLines.Add(("required_bundles_delta: {0}" -f (Format-SignedInt ($bundleResult.RequiredBundleCount - $chunkResult.RequiredBundleCount))))
$compareLines.Add(("chunk_reuse_delta: {0}" -f (Format-SignedPctPoints ($bundleResult.OverallChunkReusePct - $chunkResult.OverallChunkReusePct))))
$compareLines.Add(("bundle_reuse_delta: {0}" -f (Format-SignedPctPoints ($bundleResult.OverallBundleReusePct - $chunkResult.OverallBundleReusePct))))
$compareLines.Add("")
$compareLines.Add("[paths]")
$compareLines.Add(("chunk_artifacts: {0}" -f (Join-Path $compareRoot "chunk\artifacts")))
$compareLines.Add(("bundle_artifacts: {0}" -f (Join-Path $compareRoot "bundle\artifacts")))

Set-Content -Path $compareSummaryPath -Value $compareLines

Write-Host ""
Write-Host "=== COMPARISON COMPLETE ==="
Write-Host "Summary: $compareSummaryPath"

if ($chunkResult.VerifyPassed -and $chunkResult.MatchPassed -and $bundleResult.VerifyPassed -and $bundleResult.MatchPassed) {
    Pop-Location
    exit 0
}

Pop-Location
exit 1
