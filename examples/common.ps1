$script:FixturesRoot = "D:\Proj\Gutch\TestData"
$script:ContentRepoName = "base"
$script:DefaultPlatform = "any"
$script:DefaultArchitecture = "any"
$script:DefaultTier = "base"

function Get-Fixtures {
    param(
        [string]$Root
    )

    return @(Get-ChildItem $Root -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -notlike '.*' } |
        Sort-Object Name)
}

function Get-BuildEntries {
    param(
        [string]$FixtureDir,
        [int]$RequestedVersions
    )

    $entries = @(Get-ChildItem $FixtureDir -Force -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -notlike '.*' } |
        Sort-Object Name)

    if ($RequestedVersions -le 0) {
        return $entries
    }

    if ($RequestedVersions -gt $entries.Count) {
        throw "Requested Versions=$RequestedVersions but fixture '$FixtureDir' only has $($entries.Count) build entries."
    }

    return @($entries | Select-Object -Last $RequestedVersions)
}

function Select-Fixture {
    param(
        [string]$Root
    )

    $fixtures = Get-Fixtures -Root $Root

    if ($fixtures.Count -eq 0) {
        Write-Host "No fixtures found under $Root."
        exit 1
    }

    Write-Host "Available fixtures:"
    for ($i = 0; $i -lt $fixtures.Count; $i++) {
        $buildCount = @(Get-ChildItem $fixtures[$i].FullName -Force -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -notlike '.*' }).Count
        Write-Host ("  [{0}] {1} ({2} builds)" -f ($i + 1), $fixtures[$i].Name, $buildCount)
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

function Read-JsonFile {
    param(
        [string]$Path
    )

    return Get-Content -Path $Path -Raw | ConvertFrom-Json
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

function Get-TargetId {
    param(
        [string]$VersionId,
        [string]$Platform,
        [string]$Architecture,
        [string]$Tier
    )

    return "$VersionId-$Platform-$Architecture-$Tier"
}

function Ensure-ContentRepoRoot {
    param(
        [string]$RepoRoot,
        [string]$Product,
        [string]$ContentRepo
    )

    New-Item -ItemType Directory -Force -Path (Join-Path $RepoRoot "targets") | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $RepoRoot "bundles") | Out-Null

    $repoManifestPath = Join-Path $RepoRoot "repo.json"
    if (-not (Test-Path $repoManifestPath)) {
        $repoManifest = [ordered]@{
            format = "gutch-content-repo-v1"
            format_version = 1
            product_id = $Product
            content_repo_id = $ContentRepo
            created_at = [DateTime]::UtcNow.ToString("yyyy-MM-ddTHH:mm:ssZ")
            targets_dir = "targets"
            bundles_dir = "bundles"
        }
        $repoManifest | ConvertTo-Json | Set-Content -Path $repoManifestPath
    }
}

function Invoke-AllFixtures {
    param(
        [string]$CurrentScriptPath,
        [int]$Versions,
        [switch]$SkipBuild
    )

    $fixtures = Get-Fixtures -Root $script:FixturesRoot
    if ($fixtures.Count -eq 0) {
        Write-Host "No fixtures found under $script:FixturesRoot."
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
            $CurrentScriptPath,
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

function Resolve-FixtureDirectory {
    param(
        [string]$GameDir
    )

    if ([string]::IsNullOrWhiteSpace($GameDir)) {
        $GameDir = Select-Fixture -Root $script:FixturesRoot
    }

    $resolvedGameDir = if ([System.IO.Path]::IsPathRooted($GameDir)) {
        $GameDir
    } else {
        Join-Path $script:FixturesRoot $GameDir
    }

    if (-not (Test-Path $resolvedGameDir)) {
        Write-Host "No fixture found at $resolvedGameDir."
        exit 1
    }

    return $resolvedGameDir
}

function Build-GutchReleaseBinary {
    param(
        [string]$RepoRoot,
        [string]$ReleaseExe
    )

    Push-Location $RepoRoot
    try {
        Write-Host "=== Building release binary ==="
        $releaseBuild = & cargo build --release 2>&1
        $releaseBuildExit = $LASTEXITCODE
        foreach ($line in @($releaseBuild | ForEach-Object { $_.ToString() })) {
            Write-Host $line
        }
        if ($releaseBuildExit -ne 0) {
            exit $releaseBuildExit
        }
        if (-not (Test-Path $ReleaseExe)) {
            Write-Host "Release binary was not found at $ReleaseExe after cargo build --release."
            exit 1
        }
    } finally {
        Pop-Location
    }
}

function Invoke-GutchRunFixture {
    param(
        [string]$FixtureDir,
        [System.IO.FileSystemInfo[]]$BuildEntries,
        [string]$ReleaseExe,
        [switch]$SkipBuild
    )

    $product = Split-Path $FixtureDir -Leaf
    $summaryPath = Join-Path $script:FixturesRoot ("run-summary.{0}.txt" -f $product)
    $artifactsRoot = Join-Path $FixtureDir ".artifacts"
    $contentRepoOutputRoot = Join-Path $FixtureDir ".content_repo"
    $appliedRoot = Join-Path $artifactsRoot "applied\latest"
    $repoRoot = $contentRepoOutputRoot
    $bundlesRoot = Join-Path $repoRoot "bundles"
    $latestEntry = $BuildEntries[-1]
    $latestVersionId = $latestEntry.Name
    $latestTargetId = Get-TargetId -VersionId $latestVersionId -Platform $script:DefaultPlatform -Architecture $script:DefaultArchitecture -Tier $script:DefaultTier
    $latestManifest = Join-Path $repoRoot "targets\$latestTargetId\manifest.json"
    $runArtifactsRoot = Join-Path $artifactsRoot "run"
    $buildSummaryRoot = Join-Path $runArtifactsRoot "build"
    $exportSummaryPath = Join-Path $runArtifactsRoot "target-export.summary.txt"
    $exportStatsPath = Join-Path $runArtifactsRoot "target-export.stats.json"
    $verifySummaryPath = Join-Path $runArtifactsRoot "target-install-verify.summary.txt"
    $verifyStatsPath = Join-Path $runArtifactsRoot "target-install-verify.stats.json"
    $compareSummaryPath = Join-Path $runArtifactsRoot "path-compare.summary.txt"
    $compareStatsPath = Join-Path $runArtifactsRoot "path-compare.stats.json"

    $verifyPassed = $false
    $targetStats = New-Object System.Collections.Generic.List[object]

    if (-not $SkipBuild) {
        Write-Host "=== Building artifacts ==="
        Remove-Item -Recurse -Force $contentRepoOutputRoot -ErrorAction SilentlyContinue
        Ensure-ContentRepoRoot -RepoRoot $contentRepoOutputRoot -Product $product -ContentRepo $script:ContentRepoName

        foreach ($entry in $BuildEntries) {
            $versionId = $entry.Name
            Write-Host "  $versionId..."
            $buildOutputRoot = Join-Path $buildSummaryRoot $versionId
            $buildSummaryPath = Join-Path $buildOutputRoot "target-insert.summary.txt"
            $buildStatsPath = Join-Path $buildOutputRoot "target-insert.stats.json"

            $buildResult = Invoke-CargoCapture -ExePath $ReleaseExe -NativeArgs @(
                "target",
                "insert",
                "--dir", $entry.FullName,
                "--repo", $contentRepoOutputRoot,
                "--version", $versionId,
                "--platform", $script:DefaultPlatform,
                "--architecture", $script:DefaultArchitecture,
                "--tier", $script:DefaultTier,
                "--chunk-profile", "auto",
                "--bundle-profile", "auto",
                "--summary-out", $buildSummaryPath,
                "--stats-json-out", $buildStatsPath
            )
            if ($buildResult.ExitCode -ne 0) {
                exit $buildResult.ExitCode
            }
            $targetStats.Add((Read-JsonFile -Path $buildStatsPath))
        }
    }

    Write-Host "`n=== Applying latest target from shared bundles ==="
    Remove-Item -Recurse -Force $appliedRoot -ErrorAction SilentlyContinue

    $reconstructResult = Invoke-CargoCapture -ExePath $ReleaseExe -NativeArgs @(
        "target",
        "export",
        "--repo", $contentRepoOutputRoot,
        "--target", $latestTargetId,
        "--output", $appliedRoot,
        "--summary-out", $exportSummaryPath,
        "--stats-json-out", $exportStatsPath
    )
    if ($reconstructResult.ExitCode -ne 0) {
        exit $reconstructResult.ExitCode
    }

    Write-Host "`n=== Verifying ==="
    $verifyResult = Invoke-CargoCapture -ExePath $ReleaseExe -NativeArgs @(
        "target_install",
        "verify",
        "--repo", $contentRepoOutputRoot,
        "--target", $latestTargetId,
        "--dir", $appliedRoot,
        "--summary-out", $verifySummaryPath,
        "--stats-json-out", $verifyStatsPath
    )
    if ($verifyResult.ExitCode -eq 0) {
        Write-Host "  gutch target_install verify: PASSED"
        $verifyPassed = $true
    } else {
        Write-Host "  gutch target_install verify: FAILED"
        exit $verifyResult.ExitCode
    }

    Write-Host "`n=== Binary diff applied/latest vs $($latestEntry.Name) ==="
    $compareResult = Invoke-CargoCapture -ExePath $ReleaseExe -NativeArgs @(
        "path",
        "compare",
        "--expected", $latestEntry.FullName,
        "--actual", $appliedRoot,
        "--summary-out", $compareSummaryPath,
        "--stats-json-out", $compareStatsPath
    )
    $compareStats = Read-JsonFile -Path $compareStatsPath
    $match = $compareResult.ExitCode -eq 0

    $bundleFiles = @(Get-ChildItem $bundlesRoot -Recurse -File -Filter *.bundle -ErrorAction SilentlyContinue)
    $bundleCount = $bundleFiles.Count
    $bundleBytes = ($bundleFiles | Measure-Object -Property Length -Sum).Sum
    if ($null -eq $bundleBytes) { $bundleBytes = 0 }
    $manifestBytes = if (Test-Path $latestManifest) { (Get-Item $latestManifest).Length } else { 0 }

    $totalChunksGenerated = 0
    $totalChunksNew = 0
    $totalChunksReused = 0
    $totalBundlesRequired = 0
    $totalBundlesGenerated = 0
    $totalBundlesReused = 0
    foreach ($stat in $targetStats) {
        $totalChunksGenerated += [int64]$stat.required_chunk_count
        $totalChunksNew += [int64]$stat.new_chunk_count
        $totalChunksReused += [int64]$stat.reused_chunk_count
        $totalBundlesRequired += [int64]$stat.required_bundle_count
        $totalBundlesGenerated += [int64]$stat.generated_bundle_count
        $totalBundlesReused += [int64]$stat.reused_bundle_count
    }

    $overallChunkReusePct = if ($totalChunksGenerated -gt 0) { [math]::Round(($totalChunksReused * 100.0) / $totalChunksGenerated, 2) } else { 0.0 }
    $overallBundleReusePct = if ($totalBundlesRequired -gt 0) { [math]::Round(($totalBundlesReused * 100.0) / $totalBundlesRequired, 2) } else { 0.0 }
    $overallBundleRatio = if ($totalBundlesRequired -gt 0) { "{0}/{1}" -f $totalBundlesGenerated, $totalBundlesRequired } else { "0/0" }

    $summaryLines = New-Object System.Collections.Generic.List[string]
    $summaryLines.Add("fixture_root: $script:FixturesRoot")
    $summaryLines.Add("fixture: $FixtureDir")
    $summaryLines.Add("product: $product")
    $summaryLines.Add("content_repo: $contentRepoOutputRoot")
    $summaryLines.Add("applied_root: $appliedRoot")
    $summaryLines.Add("target: $latestTargetId")
    $summaryLines.Add("builds_selected: $($BuildEntries.Count)")
    $summaryLines.Add("latest_build: $($latestEntry.Name)")
    $summaryLines.Add("status: verify=$(if ($verifyPassed) { 'PASSED' } else { 'FAILED' }), diff=$(if ($match) { 'MATCH' } else { 'FAILED' })")
    $summaryLines.Add(("files: truth={0}, applied={1}, matched={2}, mismatched={3}, missing={4}, extra={5}" -f [int]$compareStats.expected_file_count, [int]$compareStats.actual_file_count, [int]$compareStats.matched_file_count, [int]$compareStats.mismatched_file_count, [int]$compareStats.missing_file_count, [int]$compareStats.extra_file_count))
    $summaryLines.Add(("artifacts: bundles={0}, bundle_size={1}, manifest_size={2}" -f $bundleCount, (Format-Size $bundleBytes), (Format-Size $manifestBytes)))
    $summaryLines.Add(("totals: chunks={0}, new={1}, reused={2}, chunk_reuse={3}; bundles_required={4}, generated={5}, reused={6}, bundle_reuse={7}, bundle_ratio={8}" -f `
        (Format-Int $totalChunksGenerated), (Format-Int $totalChunksNew), (Format-Int $totalChunksReused), (Format-Pct $overallChunkReusePct), `
        (Format-Int $totalBundlesRequired), (Format-Int $totalBundlesGenerated), (Format-Int $totalBundlesReused), (Format-Pct $overallBundleReusePct), $overallBundleRatio))
    $summaryLines.Add(("manifest: {0}" -f $latestManifest))
    $summaryLines.Add(("path compare summary: {0}" -f $compareSummaryPath))
    $summaryLines.Add(("path compare stats: {0}" -f $compareStatsPath))
    $summaryLines.Add(("target export summary: {0}" -f $exportSummaryPath))
    $summaryLines.Add(("target export stats: {0}" -f $exportStatsPath))
    $summaryLines.Add(("target_install verify summary: {0}" -f $verifySummaryPath))
    $summaryLines.Add(("target_install verify stats: {0}" -f $verifyStatsPath))
    $summaryLines.Add("")
    $summaryLines.Add("[target-stats]")
    $summaryLines.Add("target | chunks gen/new/reused | chunk reuse | bundles req/gen/reused | bundle reuse")
    foreach ($stat in $targetStats) {
        $summaryLines.Add(("{0} | {1}/{2}/{3} | {4} | {5}/{6}/{7} | {8}" -f `
            $stat.target_id,
            $stat.required_chunk_count,
            $stat.new_chunk_count,
            $stat.reused_chunk_count,
            $stat.reused_chunk_percent,
            $stat.required_bundle_count,
            $stat.generated_bundle_count,
            $stat.reused_bundle_count,
            $stat.reused_bundle_percent))
    }
    Set-Content -Path $summaryPath -Value $summaryLines

    if ($match) {
        Write-Host "`n=== SUCCESS ==="
        return
    }

    Write-Host "`n=== FAILED ==="
    exit 1
}

function Invoke-GutchCompressionComparison {
    param(
        [string]$FixtureDir,
        [System.IO.FileSystemInfo[]]$BuildEntries,
        [string]$ReleaseExe,
        [switch]$SkipBuild
    )

    function Invoke-CompressionMode {
        param(
            [string]$CompressionUnit
        )

        $product = Split-Path $FixtureDir -Leaf
        $modeRoot = Join-Path $FixtureDir ".artifacts\compare_compress\$CompressionUnit"
        $contentRepoOutputRoot = Join-Path $modeRoot ".content_repo"
        $appliedRoot = Join-Path $modeRoot ".artifacts\applied\latest"
        $summaryPath = Join-Path $modeRoot "run-summary.txt"
        $targetStats = New-Object System.Collections.Generic.List[object]
        $buildSummaryRoot = Join-Path $modeRoot ".artifacts\build"
        $exportSummaryPath = Join-Path $modeRoot ".artifacts\target-export.summary.txt"
        $exportStatsPath = Join-Path $modeRoot ".artifacts\target-export.stats.json"
        $verifySummaryPath = Join-Path $modeRoot ".artifacts\target-install-verify.summary.txt"
        $verifyStatsPath = Join-Path $modeRoot ".artifacts\target-install-verify.stats.json"
        $compareSummaryPath = Join-Path $modeRoot ".artifacts\path-compare.summary.txt"
        $compareStatsPath = Join-Path $modeRoot ".artifacts\path-compare.stats.json"
        $latestEntry = $BuildEntries[-1]
        $latestVersionId = $latestEntry.Name
        $latestTargetId = Get-TargetId -VersionId $latestVersionId -Platform $script:DefaultPlatform -Architecture $script:DefaultArchitecture -Tier $script:DefaultTier
        $latestManifest = Join-Path $contentRepoOutputRoot "targets\$latestTargetId\manifest.json"

        if (-not $SkipBuild) {
            Remove-Item -Recurse -Force $modeRoot -ErrorAction SilentlyContinue
        } else {
            Remove-Item -Recurse -Force $appliedRoot -ErrorAction SilentlyContinue
        }
        New-Item -ItemType Directory -Force -Path $modeRoot | Out-Null

        if (-not $SkipBuild) {
            Ensure-ContentRepoRoot -RepoRoot $contentRepoOutputRoot -Product $product -ContentRepo $script:ContentRepoName
            Write-Host "=== [$CompressionUnit] Building artifacts ==="
            foreach ($entry in $BuildEntries) {
                $versionId = $entry.Name
                Write-Host "  $versionId..."
                $buildOutputRoot = Join-Path $buildSummaryRoot $versionId
                $buildSummaryPath = Join-Path $buildOutputRoot "target-insert.summary.txt"
                $buildStatsPath = Join-Path $buildOutputRoot "target-insert.stats.json"

                $buildResult = Invoke-CargoCapture -ExePath $ReleaseExe -NativeArgs @(
                    "target",
                    "insert",
                    "--dir", $entry.FullName,
                    "--repo", $contentRepoOutputRoot,
                    "--version", $versionId,
                    "--platform", $script:DefaultPlatform,
                    "--architecture", $script:DefaultArchitecture,
                    "--tier", $script:DefaultTier,
                    "--chunk-profile", "auto",
                    "--bundle-profile", "auto",
                    "--compression-unit", $CompressionUnit,
                    "--summary-out", $buildSummaryPath,
                    "--stats-json-out", $buildStatsPath
                )
                if ($buildResult.ExitCode -ne 0) {
                    throw "gutch build failed for $CompressionUnit / $versionId"
                }
                $targetStats.Add((Read-JsonFile -Path $buildStatsPath))
            }
        }

        Write-Host ""
        Write-Host "=== [$CompressionUnit] Reconstructing latest target ==="
        $reconstructResult = Invoke-CargoCapture -ExePath $ReleaseExe -NativeArgs @(
            "target",
            "export",
            "--repo", $contentRepoOutputRoot,
            "--target", $latestTargetId,
            "--output", $appliedRoot,
            "--summary-out", $exportSummaryPath,
            "--stats-json-out", $exportStatsPath
        )
        if ($reconstructResult.ExitCode -ne 0) {
            throw "gutch reconstruct failed for $CompressionUnit"
        }

        Write-Host ""
        Write-Host "=== [$CompressionUnit] Verifying ==="
        $verifyResult = Invoke-CargoCapture -ExePath $ReleaseExe -NativeArgs @(
            "target_install",
            "verify",
            "--repo", $contentRepoOutputRoot,
            "--target", $latestTargetId,
            "--dir", $appliedRoot,
            "--summary-out", $verifySummaryPath,
            "--stats-json-out", $verifyStatsPath
        )
        if ($verifyResult.ExitCode -ne 0) {
            throw "gutch target_install verify failed for $CompressionUnit"
        }

        Write-Host ""
        Write-Host "=== [$CompressionUnit] Binary diff applied/latest vs $latestVersionId ==="
        $compareResult = Invoke-CargoCapture -ExePath $ReleaseExe -NativeArgs @(
            "path",
            "compare",
            "--expected", $latestEntry.FullName,
            "--actual", $appliedRoot,
            "--summary-out", $compareSummaryPath,
            "--stats-json-out", $compareStatsPath
        )
        $compareStats = Read-JsonFile -Path $compareStatsPath
        $latestTargetStats = $targetStats[-1]
        $verifyStats = Read-JsonFile -Path $verifyStatsPath
        $bundleFiles = @(Get-ChildItem (Join-Path $contentRepoOutputRoot "bundles") -Recurse -File -Filter *.bundle -ErrorAction SilentlyContinue)
        $bundleCount = $bundleFiles.Count
        $bundleBytes = ($bundleFiles | Measure-Object -Property Length -Sum).Sum
        if ($null -eq $bundleBytes) { $bundleBytes = 0 }
        $artifactBytes = Get-DirectorySizeBytes -Path $contentRepoOutputRoot
        $manifestBytes = (Get-Item $latestManifest).Length

        $totalChunksGenerated = 0
        $totalChunksNew = 0
        $totalChunksReused = 0
        $totalBundlesRequired = 0
        $totalBundlesGenerated = 0
        $totalBundlesReused = 0
        foreach ($stat in $targetStats) {
            $totalChunksGenerated += [int64]$stat.required_chunk_count
            $totalChunksNew += [int64]$stat.new_chunk_count
            $totalChunksReused += [int64]$stat.reused_chunk_count
            $totalBundlesRequired += [int64]$stat.required_bundle_count
            $totalBundlesGenerated += [int64]$stat.generated_bundle_count
            $totalBundlesReused += [int64]$stat.reused_bundle_count
        }

        $overallChunkReusePct = if ($totalChunksGenerated -gt 0) { [math]::Round(($totalChunksReused * 100.0) / $totalChunksGenerated, 2) } else { 0.0 }
        $overallBundleReusePct = if ($totalBundlesRequired -gt 0) { [math]::Round(($totalBundlesReused * 100.0) / $totalBundlesRequired, 2) } else { 0.0 }

        $summaryLines = New-Object System.Collections.Generic.List[string]
        $summaryLines.Add("fixture_root: $script:FixturesRoot")
        $summaryLines.Add("fixture: $FixtureDir")
        $summaryLines.Add("compression_unit: $CompressionUnit")
        $summaryLines.Add("product: $product")
        $summaryLines.Add("content_repo: $contentRepoOutputRoot")
        $summaryLines.Add("applied_root: $appliedRoot")
        $summaryLines.Add("target: $latestTargetId")
        $summaryLines.Add("builds_selected: $($BuildEntries.Count)")
        $summaryLines.Add("latest_build: $latestVersionId")
        $summaryLines.Add("status: verify=$(if ($verifyStats.verification_passed) { 'PASSED' } else { 'FAILED' }), diff=$(if ($compareResult.ExitCode -eq 0) { 'MATCH' } else { 'FAILED' })")
        $summaryLines.Add(("files: truth={0}, applied={1}, matched={2}, mismatched={3}, missing={4}, extra={5}" -f [int]$compareStats.expected_file_count, [int]$compareStats.actual_file_count, [int]$compareStats.matched_file_count, [int]$compareStats.mismatched_file_count, [int]$compareStats.missing_file_count, [int]$compareStats.extra_file_count))
        $summaryLines.Add(("artifacts: total_size={0}, bundles={1}, bundle_size={2}, manifest_size={3}" -f (Format-Size $artifactBytes), $bundleCount, (Format-Size $bundleBytes), (Format-Size $manifestBytes)))
        $summaryLines.Add(("manifest-estimate: logical={0}, compressed_estimate={1}, required_chunks={2}, required_bundles={3}" -f `
            (Format-Size ([int64]$latestTargetStats.total_logical_size)),
            (Format-Size ([int64]$latestTargetStats.total_compressed_size_estimate)),
            (Format-Int ([int64]$latestTargetStats.required_chunk_count)),
            (Format-Int ([int64]$latestTargetStats.required_bundle_count))))
        $summaryLines.Add(("totals: chunks={0}, new={1}, reused={2}, chunk_reuse={3}; bundles_required={4}, generated={5}, reused={6}, bundle_reuse={7}" -f `
            (Format-Int $totalChunksGenerated), (Format-Int $totalChunksNew), (Format-Int $totalChunksReused), (Format-Pct $overallChunkReusePct), `
            (Format-Int $totalBundlesRequired), (Format-Int $totalBundlesGenerated), (Format-Int $totalBundlesReused), (Format-Pct $overallBundleReusePct)))
        $summaryLines.Add(("manifest: {0}" -f $latestManifest))
        Set-Content -Path $summaryPath -Value $summaryLines

        return [pscustomobject]@{
            CompressionUnit = $CompressionUnit
            SummaryPath = $summaryPath
            ManifestPath = $latestManifest
            Product = $product
            BuildsSelected = $BuildEntries.Count
            LatestBuild = $latestVersionId
            VerifyPassed = [bool]$verifyStats.verification_passed
            MatchPassed = ($compareResult.ExitCode -eq 0)
            ArtifactBytes = [int64]$artifactBytes
            BundleCount = [int64]$bundleCount
            BundleBytes = [int64]$bundleBytes
            ManifestBytes = [int64]$manifestBytes
            TotalLogicalSize = [int64]$latestTargetStats.total_logical_size
            TotalCompressedEstimate = [int64]$latestTargetStats.total_compressed_size_estimate
            RequiredChunkCount = [int64]$latestTargetStats.required_chunk_count
            RequiredBundleCount = [int64]$latestTargetStats.required_bundle_count
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

    $product = Split-Path $FixtureDir -Leaf
    $compareSummaryPath = Join-Path $script:FixturesRoot ("compare-summary.{0}.txt" -f $product)

    try {
        $chunkResult = Invoke-CompressionMode -CompressionUnit "chunk"
        Write-Host ""
        $bundleResult = Invoke-CompressionMode -CompressionUnit "bundle"
    } catch {
        Write-Host $_
        exit 1
    }

    $compareLines = New-Object System.Collections.Generic.List[string]
    $compareLines.Add("fixture_root: $script:FixturesRoot")
    $compareLines.Add("fixture: $FixtureDir")
    $compareLines.Add("product: $product")
    $compareLines.Add("builds_selected: $($BuildEntries.Count)")
    $compareLines.Add("latest_build: $($BuildEntries[-1].Name)")
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
    $compareLines.Add(("chunk_summary: {0}" -f $chunkResult.SummaryPath))
    $compareLines.Add(("bundle_summary: {0}" -f $bundleResult.SummaryPath))
    Set-Content -Path $compareSummaryPath -Value $compareLines

    Write-Host ""
    Write-Host "=== COMPARISON COMPLETE ==="
    Write-Host "Summary: $compareSummaryPath"

    if ($chunkResult.VerifyPassed -and $chunkResult.MatchPassed -and $bundleResult.VerifyPassed -and $bundleResult.MatchPassed) {
        return
    }

    exit 1
}
