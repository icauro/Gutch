param(
    [string]$Name,
    [int]$SizeMB = 0,
    [int]$Versions = 0,
    [int]$ChangePercent = -1
)

$ErrorActionPreference = "Stop"

$app = Join-Path $PSScriptRoot "CreateMock.cs"
$fixturesRoot = Join-Path $PSScriptRoot "fixtures"
$argsList = @("run", "--file", $app, "--", "--fixtures-root", $fixturesRoot)

if (-not [string]::IsNullOrWhiteSpace($Name)) {
    $argsList += @("--name", $Name)
}
if ($SizeMB -gt 0) {
    $argsList += @("--size-mb", "$SizeMB")
}
if ($Versions -gt 0) {
    $argsList += @("--versions", "$Versions")
}
if ($ChangePercent -ge 0) {
    $argsList += @("--change-percent", "$ChangePercent")
}

& dotnet @argsList
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
