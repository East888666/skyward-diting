param(
    [string]$Adb = "adb",
    [string]$Serial,
    [string]$Out = (Join-Path $PWD ("skyward-logs-" + (Get-Date -Format "yyyyMMdd-HHmmss")))
)
$ErrorActionPreference = "Stop"
$adbArgs = @()
if ($Serial) { $adbArgs += @("-s", $Serial) }
# Inspect only recovery; never collect a running Android installation.
& $Adb @adbArgs shell "test -f /skyward/device.toml"
if ($LASTEXITCODE -ne 0) { throw "Connect the phone in Skyward recovery first." }
New-Item -ItemType Directory -Path $Out -Force | Out-Null
& $Adb @adbArgs pull /skyward/run/recovery.log (Join-Path $Out "recovery.log")
if ($LASTEXITCODE -ne 0) { throw "Failed to pull recovery.log." }
# exec-out avoids an interactive shell; stdout is saved as UTF-8.
$kernel = & $Adb @adbArgs exec-out dmesg 2> (Join-Path $Out "adb-stderr.log")
if ($LASTEXITCODE -ne 0) { throw "Failed to read the kernel log." }
$kernel | Set-Content -LiteralPath (Join-Path $Out "kernel.log") -Encoding UTF8
& $Adb @adbArgs pull /skyward/device.toml (Join-Path $Out "device.toml")
if ($LASTEXITCODE -ne 0) { throw "Failed to pull the device configuration." }
Write-Output ("Logs saved to " + (Resolve-Path -LiteralPath $Out).Path)
