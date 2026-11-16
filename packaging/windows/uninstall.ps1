#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Uninstall the Windows slipstream install created by install.ps1.

.DESCRIPTION
  Stops and deletes the SlipstreamHost / SlipstreamWeb scheduled tasks, removes the install
  directory (C:\Program Files\Slipstream by default), and leaves the per-user config in
  %APPDATA%\slipstream alone unless -PurgeConfig is given.

  -PurgeConfig deletes the host identity (cert.pem / key.pem) and the mgmt/plugin tokens. Paired
  devices must re-pair afterwards, so it is opt-in.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File packaging\windows\uninstall.ps1
.EXAMPLE
  powershell -ExecutionPolicy Bypass -File packaging\windows\uninstall.ps1 -PurgeConfig
#>
[CmdletBinding()]
param(
  [string]$InstallDir = 'C:\Program Files\Slipstream',
  [switch]$PurgeConfig
)

$ErrorActionPreference = 'Stop'

function Step([string]$m) { Write-Host "==> $m" -ForegroundColor Cyan }

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
  Step 're-launching elevated (UAC)'
  $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"")
  foreach ($kv in $PSBoundParameters.GetEnumerator()) {
    $argList += "-$($kv.Key)"
    if ($kv.Value -isnot [switch]) { $argList += "`"$($kv.Value)`"" }
  }
  $p = Start-Process -FilePath 'powershell.exe' -Verb RunAs -ArgumentList $argList -PassThru -Wait
  exit $p.ExitCode
}

foreach ($task in @('SlipstreamWeb', 'SlipstreamHost')) {
  Step "removing task $task"
  & schtasks.exe /end /tn $task 2>$null | Out-Null
  & schtasks.exe /delete /tn $task /f 2>$null | Out-Null
  if ($LASTEXITCODE -ne 0) { Write-Host "    (not present)" }
}

Step "removing $InstallDir"
if (Test-Path $InstallDir) {
  Remove-Item $InstallDir -Recurse -Force
} else {
  Write-Host '    (not present)'
}

if ($PurgeConfig) {
  $cfg = Join-Path $env:APPDATA 'slipstream'
  Step "purging config $cfg"
  foreach ($f in @('cert.pem', 'key.pem', 'mgmt-token', 'plugin-token', 'web-password', 'slipstream1-paired.json')) {
    $p = Join-Path $cfg $f
    if (Test-Path $p) { Remove-Item $p -Force; Write-Host "    removed $f" }
  }
  Write-Host '    paired devices must pair again (the identity cert is gone)'
} else {
  Write-Host ''
  Write-Host "Kept $(Join-Path $env:APPDATA 'slipstream') (host identity + tokens). Use -PurgeConfig to remove." -ForegroundColor Yellow
}

Write-Host ''
Write-Host 'slipstream uninstalled' -ForegroundColor Green
Write-Host 'firewall rules are not touched by this script; remove them with:'
Write-Host '  powershell -ExecutionPolicy Bypass -File packaging\windows\firewall.ps1 -Remove'
