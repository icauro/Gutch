$script:FixturesRoot = "D:\Proj\Gutch\TestData"
$script:ContentRepoName = "base"
$script:DefaultPlatform = "any"
$script:DefaultArchitecture = "any"
$script:DefaultTier = "base"

function Get-Fixtures {
    param([string]$Root)

    @(Get-ChildItem $Root -Directory -ErrorAction SilentlyContinue |
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

    @($entries | Select-Object -Last $RequestedVersions)
}

function Select-Fixture {
    param([string]$Root)

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

function Invoke-LoggedCommand {
    param(
        [string]$ExePath,
        [string[]]$NativeArgs,
        [string]$FailureMessage
    )

    $output = & $ExePath @NativeArgs 2>&1
    $exitCode = $LASTEXITCODE
    foreach ($line in @($output | ForEach-Object { $_.ToString() })) {
        Write-Host $line
    }

    if ($exitCode -ne 0) {
        if ($FailureMessage) {
            throw $FailureMessage
        }
        exit $exitCode
    }
}

function Get-TargetId {
    param(
        [string]$VersionId,
        [string]$Platform,
        [string]$Architecture,
        [string]$Tier
    )

    "$VersionId-$Platform-$Architecture-$Tier"
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
        [ordered]@{
            format = "gutch-content-repo-v1"
            format_version = 1
            product_id = $Product
            content_repo_id = $ContentRepo
            created_at = [DateTime]::UtcNow.ToString("yyyy-MM-ddTHH:mm:ssZ")
            targets_dir = "targets"
            bundles_dir = "bundles"
        } | ConvertTo-Json | Set-Content -Path $repoManifestPath
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

        $invokeArgs = @("-NoLogo", "-NoProfile", "-File", $CurrentScriptPath, "-GameDir", $fixture.FullName)
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
    param([string]$GameDir)

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

    $resolvedGameDir
}

function Build-GutchReleaseBinary {
    param(
        [string]$RepoRoot,
        [string]$ReleaseExe
    )

    Push-Location $RepoRoot
    try {
        Write-Host "=== Building release binary ==="
        Invoke-LoggedCommand -ExePath "cargo" -NativeArgs @("build", "--release")
        if (-not (Test-Path $ReleaseExe)) {
            throw "Release binary was not found at $ReleaseExe after cargo build --release."
        }
    } finally {
        Pop-Location
    }
}

function Invoke-GutchFixturePass {
    param(
        [string]$FixtureDir,
        [System.IO.FileSystemInfo[]]$BuildEntries,
        [string]$ReleaseExe,
        [string]$RepoRoot,
        [string]$AppliedRoot,
        [string]$StatsRoot,
        [string]$Label,
        [switch]$SkipBuild,
        [string]$CompressionUnit = ""
    )

    $product = Split-Path $FixtureDir -Leaf
    $latestEntry = $BuildEntries[-1]
    $latestTargetId = Get-TargetId -VersionId $latestEntry.Name -Platform $script:DefaultPlatform -Architecture $script:DefaultArchitecture -Tier $script:DefaultTier

    if (-not $SkipBuild) {
        if ($Label) {
            Write-Host "=== [$Label] Building artifacts ==="
        } else {
            Write-Host "=== Building artifacts ==="
        }
        Remove-Item -Recurse -Force $RepoRoot -ErrorAction SilentlyContinue
        Ensure-ContentRepoRoot -RepoRoot $RepoRoot -Product $product -ContentRepo $script:ContentRepoName

        foreach ($entry in $BuildEntries) {
            Write-Host ("  {0}..." -f $entry.Name)
            $buildArgs = @(
                "target", "insert",
                "--dir", $entry.FullName,
                "--repo", $RepoRoot,
                "--version", $entry.Name,
                "--platform", $script:DefaultPlatform,
                "--architecture", $script:DefaultArchitecture,
                "--tier", $script:DefaultTier,
                "--chunk-profile", "auto",
                "--bundle-profile", "auto",
                "--stats-json-out", (Join-Path $StatsRoot "build\$($entry.Name)\target-insert.stats.json")
            )
            if ($CompressionUnit) {
                $buildArgs += @("--compression-unit", $CompressionUnit)
            }
            Invoke-LoggedCommand -ExePath $ReleaseExe -NativeArgs $buildArgs
        }
    } else {
        Remove-Item -Recurse -Force $AppliedRoot -ErrorAction SilentlyContinue
    }

    Write-Host ""
    if ($Label) {
        Write-Host "=== [$Label] Reconstructing latest target ==="
    } else {
        Write-Host "=== Applying latest target from shared bundles ==="
    }
    Invoke-LoggedCommand -ExePath $ReleaseExe -NativeArgs @(
        "target", "export",
        "--repo", $RepoRoot,
        "--target", $latestTargetId,
        "--output", $AppliedRoot,
        "--stats-json-out", (Join-Path $StatsRoot "target-export.stats.json")
    ) -FailureMessage ($(if ($Label) { "gutch reconstruct failed for $Label" } else { $null }))

    Write-Host ""
    if ($Label) {
        Write-Host "=== [$Label] Verifying ==="
    } else {
        Write-Host "=== Verifying ==="
    }
    Invoke-LoggedCommand -ExePath $ReleaseExe -NativeArgs @(
        "target_install", "verify",
        "--repo", $RepoRoot,
        "--target", $latestTargetId,
        "--dir", $AppliedRoot,
        "--stats-json-out", (Join-Path $StatsRoot "target-install-verify.stats.json")
    ) -FailureMessage ($(if ($Label) { "gutch target_install verify failed for $Label" } else { $null }))

    Write-Host ""
    if ($Label) {
        Write-Host "=== [$Label] Binary diff applied/latest vs $($latestEntry.Name) ==="
    } else {
        Write-Host "=== Binary diff applied/latest vs $($latestEntry.Name) ==="
    }
    Invoke-LoggedCommand -ExePath $ReleaseExe -NativeArgs @(
        "path", "compare",
        "--expected", $latestEntry.FullName,
        "--actual", $AppliedRoot,
        "--stats-json-out", (Join-Path $StatsRoot "path-compare.stats.json")
    ) -FailureMessage ($(if ($Label) { "gutch path compare failed for $Label" } else { $null }))
}

function Invoke-GutchRunFixture {
    param(
        [string]$FixtureDir,
        [System.IO.FileSystemInfo[]]$BuildEntries,
        [string]$ReleaseExe,
        [switch]$SkipBuild
    )

    Invoke-GutchFixturePass `
        -FixtureDir $FixtureDir `
        -BuildEntries $BuildEntries `
        -ReleaseExe $ReleaseExe `
        -RepoRoot (Join-Path $FixtureDir ".content_repo") `
        -AppliedRoot (Join-Path $FixtureDir ".artifacts\applied\latest") `
        -StatsRoot (Join-Path $FixtureDir ".artifacts\run") `
        -SkipBuild:$SkipBuild

    Write-Host ""
    Write-Host "=== SUCCESS ==="
}

function Invoke-GutchCompressionComparison {
    param(
        [string]$FixtureDir,
        [System.IO.FileSystemInfo[]]$BuildEntries,
        [string]$ReleaseExe,
        [switch]$SkipBuild
    )

    try {
        Invoke-GutchFixturePass `
            -FixtureDir $FixtureDir `
            -BuildEntries $BuildEntries `
            -ReleaseExe $ReleaseExe `
            -RepoRoot (Join-Path $FixtureDir ".artifacts\compare_compress\chunk\.content_repo") `
            -AppliedRoot (Join-Path $FixtureDir ".artifacts\compare_compress\chunk\.artifacts\applied\latest") `
            -StatsRoot (Join-Path $FixtureDir ".artifacts\compare_compress\chunk\.artifacts") `
            -Label "chunk" `
            -SkipBuild:$SkipBuild `
            -CompressionUnit "chunk"

        Write-Host ""

        Invoke-GutchFixturePass `
            -FixtureDir $FixtureDir `
            -BuildEntries $BuildEntries `
            -ReleaseExe $ReleaseExe `
            -RepoRoot (Join-Path $FixtureDir ".artifacts\compare_compress\bundle\.content_repo") `
            -AppliedRoot (Join-Path $FixtureDir ".artifacts\compare_compress\bundle\.artifacts\applied\latest") `
            -StatsRoot (Join-Path $FixtureDir ".artifacts\compare_compress\bundle\.artifacts") `
            -Label "bundle" `
            -SkipBuild:$SkipBuild `
            -CompressionUnit "bundle"
    } catch {
        Write-Host $_
        exit 1
    }

    Write-Host ""
    Write-Host "=== COMPARISON COMPLETE ==="
}
