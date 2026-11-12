param(
    [string]$Adb = "adb",
    [string]$Serial,
    [string]$Out = (Join-Path $PWD ("skyward-live-" + (Get-Date -Format "yyyyMMdd-HHmmss"))),
    [int]$Seconds = 300,
    [string]$Python = "python"
)
$ErrorActionPreference = "Stop"
$captureArgs = @((Join-Path $PSScriptRoot "capture-recovery-live.py"), "--adb", $Adb, "--out", $Out, "--seconds", "$Seconds")
if ($Serial) { $captureArgs += @("--serial", $Serial) }
Write-Output "Saving recovery logs to $Out for $Seconds seconds; reconnects are handled automatically."
& $Python @captureArgs
if ($LASTEXITCODE -ne 0) { throw "Live log capture failed (exit $LASTEXITCODE)." }
Write-Output "Logs saved to $Out"
