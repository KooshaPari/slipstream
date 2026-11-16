#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Add, remove, or show the Windows Firewall rules for slipstream.

.DESCRIPTION
  The Windows counterpart of packaging/linux/slipstream-native.xml, slipstream-web.xml and
  slipstream.ufw. Like the Linux packages, NOTHING is enabled automatically — the installer never
  touches the firewall. Run this yourself when you want LAN reachability.

  Rules (all inbound):
    slipstream-web      TCP 47992   the management web console, login-gated HTTPS
    slipstream-native   UDP 9777    native slipstream/1 QUIC control plane
    slipstream-mdns     UDP 5353    mDNS auto-discovery (_slipstream._udp.local)
    slipstream-mgmt     TCP 47990   mgmt/library REST API. Only needed when the host is bound
                                    off-loopback (default 0.0.0.0:47990); with the packaged
                                    `serve --mgmt-bind 127.0.0.1:47990` it is loopback-only and
                                    this rule is unnecessary. Off-loopback the API serves only
                                    read-only status/library to paired clients.

  The media DATA plane uses an EPHEMERAL UDP port negotiated per session (or the fixed port given
  to --data-port / SLIPSTREAM_DATA_PORT), so there is no fixed data port to open. On a restrictive
  firewall allow the ephemeral UDP range too — the project does not pin one.

.PARAMETER Add
  Create the rules.

.PARAMETER Remove
  Delete the rules.

.PARAMETER Status
  Show the current rules (default).

.PARAMETER IncludeMgmt
  Also add TCP 47990 (only meaningful with an off-loopback mgmt bind).

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File packaging\windows\firewall.ps1 -Add -IncludeMgmt
#>
[CmdletBinding()]
param(
  [switch]$Add,
  [switch]$Remove,
  [switch]$Status,
  [switch]$IncludeMgmt
)

$ErrorActionPreference = 'Stop'

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin -and ($Add -or $Remove)) {
  Write-Host '==> re-launching elevated (UAC)' -ForegroundColor Cyan
  $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"")
  foreach ($kv in $PSBoundParameters.GetEnumerator()) {
    $argList += "-$($kv.Key)"
    if ($kv.Value -isnot [switch]) { $argList += "`"$($kv.Value)`"" }
  }
  $p = Start-Process -FilePath 'powershell.exe' -Verb RunAs -ArgumentList $argList -PassThru -Wait
  exit $p.ExitCode
}

# name, protocol, port, description (mirrors the firewalld XML files and slipstream.ufw)
$rules = @(
  @{ Name = 'slipstream-web';    Protocol = 'TCP'; Port = 47992; Desc = 'slipstream web console (HTTPS, login-gated)' },
  @{ Name = 'slipstream-native'; Protocol = 'UDP'; Port = 9777;  Desc = 'slipstream native QUIC control plane' },
  @{ Name = 'slipstream-mdns';   Protocol = 'UDP'; Port = 5353;  Desc = 'slipstream mDNS discovery' }
)
if ($IncludeMgmt) {
  $rules += @{ Name = 'slipstream-mgmt'; Protocol = 'TCP'; Port = 47990; Desc = 'slipstream mgmt REST API (only when bound off-loopback)' }
}

if ($Status -or -not ($Add -or $Remove)) {
  Write-Host 'slipstream firewall rules:' -ForegroundColor Green
  foreach ($r in $rules) {
    $found = Get-NetFirewallRule -DisplayName $r.Name -ErrorAction SilentlyContinue
    $state = if ($found) { ($found | Get-NetFirewallPortFilter | ForEach-Object { "$($_.Protocol)/$($_.LocalPort)" }) -join ', ' } else { 'absent' }
    Write-Host ("  {0,-18} {1,-4} {2,-6} {3}" -f $r.Name, $r.Protocol, $r.Port, $state)
  }
  exit 0
}

if ($Remove) {
  foreach ($r in $rules) {
    if (Get-NetFirewallRule -DisplayName $r.Name -ErrorAction SilentlyContinue) {
      Remove-NetFirewallRule -DisplayName $r.Name
      Write-Host "removed $($r.Name)"
    } else {
      Write-Host "$($r.Name): absent"
    }
  }
  exit 0
}

if ($Add) {
  foreach ($r in $rules) {
    if (Get-NetFirewallRule -DisplayName $r.Name -ErrorAction SilentlyContinue) {
      Write-Host "$($r.Name): already present"
      continue
    }
    New-NetFirewallRule -DisplayName $r.Name -Direction Inbound -Action Allow `
      -Protocol $r.Protocol -LocalPort $r.Port -Profile Domain,Private -Description $r.Desc | Out-Null
    Write-Host "added $($r.Name) ($($r.Protocol)/$($r.Port))"
  }
  Write-Host ''
  Write-Host 'Rules are scoped to the Domain and Private profiles (not Public).' -ForegroundColor Yellow
}
