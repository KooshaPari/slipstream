#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Install the slipstream streaming host and its management web console on Windows.

.DESCRIPTION
  The Windows counterpart of the Linux packages (packaging/debian/build-web-deb.sh,
  packaging/rpm/slipstream.spec with --with web, scripts/slipstream-web.service). It produces the
  same layout and the same port contract:

      host mgmt API   127.0.0.1:47990   (HTTPS, host identity cert, bearer token)
      web console     0.0.0.0:47992     (HTTPS, proxying /api to the mgmt API server-side)

  What it does, in order:
    1. resolves a slipstream-host.exe (yours, the repo's release build, or `cargo build`)
    2. vendors bun (pinned, checksum-verified) into the private runtime dir if needed
    3. builds web/ (bun install --frozen-lockfile --ignore-scripts && bun run build)
    4. stages everything under -InstallDir (default C:\Program Files\Slipstream)
    5. registers two logon scheduled tasks: SlipstreamHost and SlipstreamWeb
    6. prints the URL to open

  Firewall: NOT touched. Mirroring the Linux packages ("packages never touch the admin's
  firewall"), run firewall.ps1 -Add yourself if you want LAN reachability.

.PARAMETER HostExe
  A prebuilt slipstream-host.exe. Default: the repo's target\release\slipstream-host.exe, else
  `cargo build --release -p slipstream-host`.

.PARAMETER BunExe
  A bun.exe to vendor instead of downloading the pinned release.

.PARAMETER NoWeb
  Host only; skip the console entirely (the Linux equivalent of --no-install-recommends).

.PARAMETER SkipBuild
  Reuse an existing web\.output (it must contain Bun.serve; the installer verifies that).

.PARAMETER NoStart
  Register the tasks but do not start them now.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File packaging\windows\install.ps1
.EXAMPLE
  powershell -ExecutionPolicy Bypass -File packaging\windows\install.ps1 -HostExe C:\path\slipstream-host.exe -StartNow
#>
[CmdletBinding()]
param(
  [string]$HostExe,
  [string]$BunExe,
  [string]$InstallDir = 'C:\Program Files\Slipstream',
  [int]$MgmtPort = 47990,
  [int]$WebPort = 47992,
  [switch]$NoWeb,
  [switch]$SkipBuild,
  [switch]$NoStart
)

$ErrorActionPreference = 'Stop'
$BunVersion = '1.3.14'
$BunSha256 = '0a0620930b6675d7ba440e81f4e0e00d3cfbe096c4b140d3fff02205e9e18922' # bun-windows-x64.zip

function Step([string]$m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Note([string]$m) { Write-Host "    $m" }
function Fail([string]$m) { Write-Host "ERROR: $m" -ForegroundColor Red; exit 1 }

# ------------------------------------------------------------------ 0. elevate
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
  Step 're-launching elevated (UAC) — the install writes Program Files and registers tasks'
  $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"")
  foreach ($kv in $PSBoundParameters.GetEnumerator()) {
    $argList += "-$($kv.Key)"
    if ($kv.Value -isnot [switch]) { $argList += "`"$($kv.Value)`"" }
  }
  $p = Start-Process -FilePath 'powershell.exe' -Verb RunAs -ArgumentList $argList -PassThru -Wait
  exit $p.ExitCode
}

$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not (Test-Path (Join-Path $repo 'web\package.json'))) { Fail "not a slipstream checkout: $repo" }
$webDir = Join-Path $repo 'web'
$runtimeBun = Join-Path $InstallDir 'bun\bun.exe'
$webOut = Join-Path $InstallDir 'web\.output'
$cfg = Join-Path $env:APPDATA 'slipstream'
$logDir = Join-Path $env:LOCALAPPDATA 'slipstream\logs'
$userSid = ([Security.Principal.WindowsIdentity]::GetCurrent()).User.Value
$userName = "$env:USERDOMAIN\$env:USERNAME"

Write-Host "slipstream Windows install" -ForegroundColor Green
Note "repo        $repo"
Note "install dir $InstallDir"
Note "ports       host $MgmtPort (loopback), console $WebPort"
Note "user        $userName ($userSid)"

New-Item -ItemType Directory -Force -Path $InstallDir, $logDir | Out-Null

# ------------------------------------------------------------------ 1. host binary
Step 'resolving slipstream-host.exe'
if (-not $HostExe) {
  $candidates = @(
    (Join-Path $repo 'target\release\slipstream-host.exe'),
    (Join-Path (Split-Path $repo -Parent) 'slipstream-target\release\slipstream-host.exe')
  )
  $HostExe = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $HostExe -and -not $NoStart -and (Get-Command cargo -ErrorAction SilentlyContinue)) {
  Step 'cargo build --release -p slipstream-host'
  Push-Location $repo
  try { & cargo build --release -p slipstream-host; if ($LASTEXITCODE -ne 0) { Fail "cargo build failed ($LASTEXITCODE)" } }
  finally { Pop-Location }
  $HostExe = Join-Path $repo 'target\release\slipstream-host.exe'
}
if (-not $HostExe -or -not (Test-Path $HostExe)) {
  Fail 'no slipstream-host.exe. Build one (`cargo build --release -p slipstream-host`) and pass -HostExe, or take a release asset.'
}
Copy-Item $HostExe (Join-Path $InstallDir 'slipstream-host.exe') -Force
Note "host: $HostExe"

# ------------------------------------------------------------------ 2. bun (vendored)
Step "vendoring bun $BunVersion"
if (-not $BunExe) {
  $local = Get-Command bun -ErrorAction SilentlyContinue
  if ($local) { $BunExe = $local.Source }
}
New-Item -ItemType Directory -Force -Path (Split-Path $runtimeBun -Parent) | Out-Null
if ($BunExe -and (Test-Path $BunExe)) {
  Copy-Item $BunExe $runtimeBun -Force
  Note "from $BunExe"
} else {
  if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64') { Fail 'automatic bun download only covers x64; pass -BunExe' }
  $zip = Join-Path $env:TEMP "bun-windows-x64-$BunVersion.zip"
  $url = "https://github.com/oven-sh/bun/releases/download/bun-v$BunVersion/bun-windows-x64.zip"
  Step "downloading $url"
  Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
  $hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($hash -ne $BunSha256) { Remove-Item $zip -Force; Fail "bun checksum mismatch: got $hash want $BunSha256" }
  Expand-Archive -Path $zip -DestinationPath (Join-Path $env:TEMP "bun-$BunVersion") -Force
  Copy-Item (Join-Path $env:TEMP "bun-$BunVersion\bun-windows-x64\bun.exe") $runtimeBun -Force
  Remove-Item $zip -Force
  Note 'checksum verified'
}
$bunVersionOut = & $runtimeBun --version
Note "bun at ${runtimeBun}: $bunVersionOut"

# ------------------------------------------------------------------ 3. web console build
if (-not $NoWeb) {
  Step 'building the web console (Nitro SSR on bun)'
  Push-Location $webDir
  try {
    if (-not $SkipBuild) {
      # --ignore-scripts: the root `prepare`/`postinstall` run codegen and bun2nix, which belong to
      # the Nix path; `bun run build` regenerates the API client and i18n itself (same as web/Dockerfile).
      & $runtimeBun install --frozen-lockfile --ignore-scripts
      if ($LASTEXITCODE -ne 0) { Fail "bun install failed ($LASTEXITCODE)" }
      & $runtimeBun run build
      if ($LASTEXITCODE -ne 0) { Fail "bun run build failed ($LASTEXITCODE)" }
    }
    $entry = Join-Path $webDir '.output\server\index.mjs'
    if (-not (Test-Path $entry)) { Fail "missing $entry" }
    if (-not (Select-String -Path $entry -Pattern 'Bun\.serve' -Quiet)) {
      Fail '.output is not a bun bundle (need the nitro bun preset + nitro-entry/bun-https.mjs)'
    }
  }
  finally { Pop-Location }

  Step 'staging the console payload'
  New-Item -ItemType Directory -Force -Path $webOut | Out-Null
  robocopy (Join-Path $webDir '.output') $webOut /MIR /NFL /NDL /NJH /NJS /NP | Out-Null
  if ($LASTEXITCODE -ge 8) { Fail "robocopy failed ($LASTEXITCODE)" }
  # The documented runtime knobs travel with the payload, exactly like /usr/share/slipstream-web.
  Copy-Item (Join-Path $webDir 'web.env.example') (Join-Path $InstallDir 'web\web.env.example') -Force
  if (Test-Path (Join-Path $webDir 'README.md')) {
    Copy-Item (Join-Path $webDir 'README.md') (Join-Path $InstallDir 'web\README.md') -Force
  }
  Note "payload: $webOut"

  # ---------------------------------------------------------------- 4. console launcher
  Step 'writing the console launcher'
  $launcher = Join-Path $InstallDir 'slipstream-web.cmd'
  $body = @"
@echo off
rem slipstream management web console launcher — Windows counterpart of /usr/bin/slipstream-web-server
rem (see scripts/slipstream-web.service). Sources the host's token + identity cert, serves HTTPS.
setlocal
set "CFG=%APPDATA%\slipstream"
set "SLIPSTREAM_MGMT_URL=https://127.0.0.1:$MgmtPort"
set "PORT=$WebPort"
set "HOST=0.0.0.0"
set "SLIPSTREAM_UI_TLS_CERT=%CFG%\cert.pem"
set "SLIPSTREAM_UI_TLS_KEY=%CFG%\key.pem"
set "SLIPSTREAM_UI_SECURE=1"
set "SLIPSTREAM_MGMT_TOKEN="
set "HERE=%~dp0"
set "LOG=%LOCALAPPDATA%\slipstream\logs\slipstream-web.log"
if not exist "%CFG%\mgmt-token" (
  echo [%DATE% %TIME%] waiting for the host to write %CFG%\mgmt-token >>"%LOG%"
  ping -n 6 127.0.0.1 >nul
  exit /b 1
)
for /f "usebackq tokens=1,* delims==" %%A in ("%CFG%\mgmt-token") do set "SLIPSTREAM_MGMT_TOKEN=%%B"
if not defined SLIPSTREAM_MGMT_TOKEN (
  echo [%DATE% %TIME%] mgmt-token present but empty >>"%LOG%"
  exit /b 1
)
:run
echo [%DATE% %TIME%] starting slipstream web console on %HOST%:%PORT% (tls) >>"%LOG%"
"%HERE%bun\bun.exe" "%HERE%web\.output\server\index.mjs" >>"%LOG%" 2>&1
echo [%DATE% %TIME%] console exited rc=%ERRORLEVEL%; restarting in 2s >>"%LOG%"
ping -n 3 127.0.0.1 >nul
goto run
"@
  Set-Content -Path $launcher -Value $body -Encoding ASCII
  Note $launcher

  # ---------------------------------------------------------------- 4b. icon + Start Menu shortcut
  # Windows branding lives in one place the shell understands: an .ico. The repo has no .ico, so
  # icons\slipstream.ico is generated from the tracked web/public/favicon-mark.png (64x64) with
  # `magick favicon-mark.png -define icon:auto-resize=256,48,32,16 icons\slipstream.ico`.
  Step 'installing the app icon and Start Menu shortcut'
  $iconSrc = Join-Path $PSScriptRoot 'icons\slipstream.ico'
  $iconDst = Join-Path $InstallDir 'slipstream.ico'
  if (Test-Path $iconSrc) {
    Copy-Item $iconSrc $iconDst -Force
    Note "icon: $iconDst"
  } else {
    Note 'no icons\slipstream.ico in this checkout — the shortcut will use the default icon'
  }
  $startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
  New-Item -ItemType Directory -Force -Path $startMenu | Out-Null
  $lnk = Join-Path $startMenu 'Slipstream Console.lnk'
  $shell = New-Object -ComObject WScript.Shell
  $sc = $shell.CreateShortcut($lnk)
  $sc.TargetPath = "https://127.0.0.1:$WebPort/"
  $sc.Description = 'Slipstream management console (HTTPS)'
  if (Test-Path $iconDst) { $sc.IconLocation = "$iconDst,0" }
  $sc.Save()
  Note "shortcut: $lnk -> https://127.0.0.1:$WebPort/"
}

# ------------------------------------------------------------------ 5. scheduled tasks
function Register-LogonTask([string]$name, [string]$command, [string]$arguments, [string]$description) {
  $esc = { param($s) [System.Security.SecurityElement]::Escape($s) }
  $argsXml = if ($arguments) { "<Arguments>$(& $esc $arguments)</Arguments>" } else { '' }
  $xml = @"
<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>$userName</Author>
    <Description>$(& $esc $description)</Description>
    <URI>\$name</URI>
  </RegistrationInfo>
  <Principals>
    <Principal id="Author">
      <UserId>$userSid</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <RestartOnFailure><Count>3</Count><Interval>PT1M</Interval></RestartOnFailure>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings>
  </Settings>
  <Triggers>
    <LogonTrigger><Enabled>true</Enabled><UserId>$userName</UserId></LogonTrigger>
  </Triggers>
  <Actions Context="Author">
    <Exec><Command>$(& $esc $command)</Command>$argsXml</Exec>
  </Actions>
</Task>
"@
  $tmp = Join-Path $env:TEMP "$name.xml"
  [System.IO.File]::WriteAllText($tmp, $xml, [System.Text.Encoding]::Unicode)
  & schtasks.exe /create /tn $name /xml $tmp /f | Out-Null
  if ($LASTEXITCODE -ne 0) { Fail "schtasks /create $name failed ($LASTEXITCODE)" }
  Remove-Item $tmp -Force -ErrorAction SilentlyContinue
  Note "task $name registered"
}

Step 'registering scheduled tasks (at logon, highest available)'
Register-LogonTask 'SlipstreamHost' (Join-Path $InstallDir 'slipstream-host.exe') `
  "serve --mgmt-bind 127.0.0.1:$MgmtPort" 'slipstream streaming host (native slipstream/1 + mgmt REST API on loopback)'
if (-not $NoWeb) {
  Register-LogonTask 'SlipstreamWeb' (Join-Path $InstallDir 'slipstream-web.cmd') '' `
    'slipstream management web console (Nitro SSR on bun, HTTPS 47992)'
}

# ------------------------------------------------------------------ 6. start + report
if (-not $NoStart) {
  Step 'starting'
  & schtasks.exe /end /tn SlipstreamHost 2>$null | Out-Null
  & schtasks.exe /run /tn SlipstreamHost | Out-Null
  Start-Sleep -Seconds 3
  if (-not $NoWeb) {
    & schtasks.exe /run /tn SlipstreamWeb | Out-Null
    Start-Sleep -Seconds 4
  }
}

function Test-Port([int]$p) {
  $c = New-Object System.Net.Sockets.TcpClient
  try { $c.Connect('127.0.0.1', $p); return $true } catch { return $false } finally { $c.Dispose() }
}
Write-Host ''
Write-Host 'slipstream installed' -ForegroundColor Green
Note "host mgmt API : https://127.0.0.1:$MgmtPort  $(if (Test-Port $MgmtPort) { 'listening' } else { 'NOT listening yet' })"
if (-not $NoWeb) {
  Note "web console   : https://127.0.0.1:$WebPort  $(if (Test-Port $WebPort) { 'listening' } else { 'NOT listening yet' })"
  Write-Host ''
  Write-Host "  Open  https://127.0.0.1:$WebPort  and choose a login password on the first visit" -ForegroundColor Yellow
  Write-Host "  (self-signed host cert — accept the warning once; also reachable at https://<host-ip>:$WebPort)"
}
Write-Host ''
Write-Host '  logs       %LOCALAPPDATA%\slipstream\logs\slipstream-web.log'
if (-not $NoWeb) { Write-Host '  shortcut   Start Menu > Slipstream Console' }
Write-Host '  firewall   powershell -ExecutionPolicy Bypass -File packaging\windows\firewall.ps1 -Add'
Write-Host '  uninstall  powershell -ExecutionPolicy Bypass -File packaging\windows\uninstall.ps1'
