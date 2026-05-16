param(
    [string]$GameDir,
    [int]$Versions = 0,
    [switch]$SkipBuild,
    [Alias('AllTests')]
    [switch]$AllFixtures
)

$ErrorActionPreference = "Stop"
. $PSScriptRoot\common.ps1

if ($AllFixtures) {
    Invoke-AllFixtures -CurrentScriptPath $PSCommandPath -Versions $Versions -SkipBuild:$SkipBuild
}

$fixtureDir = Resolve-FixtureDirectory -GameDir $GameDir
$buildEntries = @(Get-BuildEntries -FixtureDir $fixtureDir -RequestedVersions $Versions)
if ($buildEntries.Count -eq 0) {
    Write-Host "No build entries found at $fixtureDir."
    exit 1
}

$releaseExe = Join-Path $PSScriptRoot "..\..\.artifacts\obj\rust\release\gutch.exe"
Build-GutchReleaseBinary -RepoRoot (Join-Path $PSScriptRoot "..") -ReleaseExe $releaseExe
Invoke-GutchRunFixture -FixtureDir $fixtureDir -BuildEntries $buildEntries -ReleaseExe $releaseExe -SkipBuild:$SkipBuild
