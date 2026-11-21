#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Uninstall the Windows slipstream install created by install.ps1.

.DESCRIPTION
  Stops and deletes the SlipstreamHost / SlipstreamWeb scheduled tasks, removes the install
  directory (C:\Program Files\Slipstream by default), and leaves the per-user config alone unless
  -PurgeConfig is given.

  -PurgeConfig deletes the host identity (cert.pem / key.pem) and the tokens from the same config
  directory the host uses: SLIPSTREAM_CONFIG_DIR, else %APPDATA%\slipstream, else
  %USERPROFILE%\.config\slipstream. Paired devices must re-pair afterwards, so it is opt-in.

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
$ProgressPreference = 'SilentlyContinue'

function Step([string]$m) { Write-Host "==> $m" -ForegroundColor Cyan }

function Invoke-Schtasks([string[]]$schtasksArgs, [bool]$allowFailure = $false) {
  # Windows PowerShell 5.1 can turn native stderr into a terminating error under
  # $ErrorActionPreference = 'Stop'. Use cmd redirection and the real native exit code instead.
  $command = 'schtasks.exe ' + ($schtasksArgs -join ' ') + ' 2>&1'
  $out = & cmd.exe /c $command
  $code = $LASTEXITCODE
  if (-not $allowFailure -and $code -ne 0) {
    Write-Host "    schtasks $($schtasksArgs -join ' ') failed ($code): $($out -join ' ')"
  }
  return $code
}

function Add-ElevatedArgument([System.Collections.ArrayList]$argList, [string]$key, $value) {
  # Forward explicit switches, including -PurgeConfig:$false. Without the value PowerShell treats
  # a passed -PurgeConfig:$false as bare -PurgeConfig in the elevated process.
  if ($value -is [switch]) {
    if ($value.IsPresent) {
      [void]$argList.Add('-' + $key + ':$true')
    } else {
      [void]$argList.Add('-' + $key + ':$false')
    }
  } else {
    [void]$argList.Add('-' + $key)
    [void]$argList.Add('"' + [string]$value + '"')
  }
}

function Get-ConfigDir {
  if ($env:SLIPSTREAM_CONFIG_DIR) { return $env:SLIPSTREAM_CONFIG_DIR }
  if ($env:APPDATA) { return (Join-Path $env:APPDATA 'slipstream') }
  return (Join-Path $env:USERPROFILE '.config\slipstream')
}

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
  Step 're-launching elevated (UAC)'
  $argList = New-Object System.Collections.ArrayList
  [void]$argList.Add('-NoProfile')
  [void]$argList.Add('-ExecutionPolicy')
  [void]$argList.Add('Bypass')
  [void]$argList.Add('-File')
  [void]$argList.Add('"' + $PSCommandPath + '"')
  foreach ($kv in $PSBoundParameters.GetEnumerator()) {
    Add-ElevatedArgument $argList $kv.Key $kv.Value
  }
  $p = Start-Process -FilePath 'powershell.exe' -Verb RunAs -ArgumentList $argList.ToArray() -PassThru -Wait
  exit $p.ExitCode
}

foreach ($task in @('SlipstreamWeb', 'SlipstreamHost')) {
  Step "removing task $task"
  $endCode = Invoke-Schtasks @('/end', '/tn', $task) $true
  $deleteCode = Invoke-Schtasks @('/delete', '/tn', $task, '/f') $true
  if ($deleteCode -ne 0 -and $deleteCode -ne 1) {
    Write-Host "    schtasks /delete failed ($deleteCode)"
  } elseif ($endCode -ne 0 -and $deleteCode -eq 1) {
    Write-Host '    (not present)'
  }
}

Step "removing $InstallDir"
if (Test-Path $InstallDir) {
  Remove-Item $InstallDir -Recurse -Force
} else {
  Write-Host '    (not present)'
}

$lnk = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Slipstream Console.lnk'
if (Test-Path $lnk) {
  Remove-Item $lnk -Force
  Write-Host "    removed Start Menu shortcut"
}

$cfg = Get-ConfigDir
if ($PurgeConfig) {
  Step "purging config $cfg"
  foreach ($f in @('cert.pem', 'key.pem', 'mgmt-token', 'plugin-token', 'web-password', 'slipstream1-paired.json')) {
    $p = Join-Path $cfg $f
    if (Test-Path $p) { Remove-Item $p -Force; Write-Host "    removed $f" }
  }
  Write-Host '    paired devices must pair again (the identity cert is gone)'
} else {
  Write-Host ''
  Write-Host "Kept $cfg (host identity + tokens). Use -PurgeConfig to remove." -ForegroundColor Yellow
}

Write-Host ''
Write-Host 'slipstream uninstalled' -ForegroundColor Green
Write-Host 'firewall rules are not touched by this script; remove them with:'
Write-Host '  powershell -ExecutionPolicy Bypass -File packaging\windows\firewall.ps1 -Remove'
