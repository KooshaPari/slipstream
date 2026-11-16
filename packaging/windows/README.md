# Packaging slipstream for Windows

The Windows install for the **streaming host** (`slipstream-host.exe`) and the **management web
console** (`web/`, Nitro SSR on bun). There is no MSI/NSIS installer: Windows has no package
manager default, so this is a scripted install — the same semantics as the Linux packages, run
once from an elevated PowerShell.

| Linux package | Windows install |
|---|---|
| `/usr/share/slipstream-web/.output` | `C:\Program Files\Slipstream\web\.output` |
| `/usr/lib/slipstream-web/bun` (vendored, pinned) | `C:\Program Files\Slipstream\bun\bun.exe` (vendored, checksum-verified) |
| `/usr/bin/slipstream-web-server` (launcher) | `C:\Program Files\Slipstream\slipstream-web.cmd` |
| `/usr/bin/slipstream-host` | `C:\Program Files\Slipstream\slipstream-host.exe` |
| `~/.config/slipstream/{cert,key}.pem`, `mgmt-token` | `%APPDATA%\slipstream\{cert,key}.pem`, `mgmt-token` |
| `systemd --user` units `slipstream-host` / `slipstream-web` | scheduled tasks `SlipstreamHost` / `SlipstreamWeb` (at logon, highest available) |
| `/usr/lib/firewalld/services/*.xml`, `slipstream.ufw` | `firewall.ps1` rules (opt-in, never automatic) |
| `apt remove slipstream-web` | `uninstall.ps1` |
| `/usr/share/icons/hicolor/*/apps/*` | `icons\slipstream.ico` -> Start Menu shortcut |

## Icons and branding

Windows takes one image for an app: an `.ico` (16/32/48/256 frames). The repo ships no `.ico`; the
only square raster is the web console's `web/public/favicon-mark.png` (64x64, tracked), so
`icons/slipstream.ico` is generated from it:

```powershell
magick web\public\favicon-mark.png -define icon:auto-resize=256,48,32,16 packaging\windows\icons\slipstream.ico
```

The 16/32/48 frames are native resolution; the 256 frame is upscaled from 64 and is soft at that
size. A 256+ master (or the SVG mark drawn at 1024) would fix that without changing this script.
`install.ps1` copies the icon to `C:\Program Files\Slipstream\slipstream.ico` and points the
`Slipstream Console` Start Menu shortcut at it.

The tray icon is NOT branded: the Windows tray backend uses the shared system application icon
(`crates/slipstream-tray/src/platform/windows.rs`, `LoadIconW`), so it renders as a generic
Windows icon rather than the slipstream mark. That is a Rust change, out of scope here.

## Ports (identical to every Linux package)

| Surface | Address | Notes |
|---|---|---|
| management REST API | `127.0.0.1:47990` | HTTPS, host identity cert, bearer token. Loopback-only by design: off-loopback it serves only read-only status/library to paired clients. |
| web console | `0.0.0.0:47992` | HTTPS (HTTP/1.1 over TLS, host identity cert), login-gated; proxies `/api` to the mgmt API server-side and injects the bearer token, so the token never reaches the browser. |
| native QUIC control | `UDP 9777` | the streaming control plane |
| media data plane | ephemeral UDP | chosen per session, reported to the client (or pin with `--data-port` / `SLIPSTREAM_DATA_PORT`) |

The host is started as `serve --mgmt-bind 127.0.0.1:47990`, which is exactly what the Linux
packages pin — the console then has `47990` to talk to and the documented `47992` to serve on.

## Install

Requirements: Windows 10/11 x64, PowerShell 5.1+, and either a prebuilt `slipstream-host.exe` or a
Rust toolchain (`cargo`). Admin rights: the install writes Program Files and registers tasks, so it
self-elevates with a UAC prompt.

```powershell
# from the repo root
powershell -ExecutionPolicy Bypass -File packaging\windows\install.ps1

# or point it at an existing build, or reuse an existing web\.output
powershell -ExecutionPolicy Bypass -File packaging\windows\install.ps1 `
  -HostExe C:\path\to\slipstream-host.exe -SkipBuild
```

What it does:

1. resolves `slipstream-host.exe` (`-HostExe`, else `target\release\`, else `cargo build --release -p slipstream-host`)
2. vendors bun — the installed bun if present, else the pinned `bun-v1.3.14-windows-x64.zip`
   downloaded from GitHub and verified against its published SHA-256
3. builds the console: `bun install --frozen-lockfile --ignore-scripts && bun run build`, then
   verifies the output really is the bun/Nitro bundle (`Bun.serve` must be in
   `.output/server/index.mjs` — the same guard `build-web-deb.sh` applies)
4. stages the payload under `C:\Program Files\Slipstream`
5. registers `SlipstreamHost` and `SlipstreamWeb` as logon tasks for the current user, highest
   available, with `RestartOnFailure`
6. starts both and prints the URL

Options: `-NoWeb` (host only, the `--no-install-recommends` equivalent), `-SkipBuild`, `-NoStart`,
`-InstallDir`, `-MgmtPort`, `-WebPort`, `-BunExe`.

## First run

Open **`https://127.0.0.1:47992`** (or `https://<host-ip>:47992` from another device on the LAN)
and choose a login password. The console serves the host's own self-signed identity certificate, so
the browser shows a certificate warning once — accept it. That is the same trust anchor native
clients pin.

The console is ready as soon as the host has written `%APPDATA%\slipstream\mgmt-token` and the
identity cert; the launcher waits and retries instead of failing permanently, mirroring the
systemd unit's `Restart=always` + `StartLimitIntervalSec=0`.

## Firewall

Nothing is opened by the installer, matching the Linux packages ("packages never touch the admin's
firewall"). If you want the console or streaming reachable from other devices:

```powershell
powershell -ExecutionPolicy Bypass -File packaging\windows\firewall.ps1 -Add
powershell -ExecutionPolicy Bypass -File packaging\windows\firewall.ps1 -Status
powershell -ExecutionPolicy Bypass -File packaging\windows\firewall.ps1 -Remove
```

| Rule | Port | When you need it |
|---|---|---|
| `slipstream-web` | TCP 47992 | the console from another device |
| `slipstream-native` | UDP 9777 | native clients connecting directly |
| `slipstream-mdns` | UDP 5353 | mDNS auto-discovery |
| `slipstream-mgmt` | TCP 47990 | only if you bind the mgmt API off-loopback (`-IncludeMgmt`) |

Rules are scoped to the Domain and Private profiles, never Public. The media data plane uses an
ephemeral UDP port, so a restrictive firewall must also allow the ephemeral range — the project
does not pin one.

## Verify

```powershell
curl.exe -s -k -o NUL -w "%{http_code}`n" https://127.0.0.1:47990/api/v1/health   # host API
curl.exe -s -k -o NUL -w "%{http_code}`n" https://127.0.0.1:47992/                # console
curl.exe -s -k https://127.0.0.1:47992/api/v1/health                              # proxied through the console
```

Expect `200` for each (the console root may answer `302` to `/setup` before a password is chosen).

## Uninstall

```powershell
powershell -ExecutionPolicy Bypass -File packaging\windows\uninstall.ps1
# also drop the host identity + tokens (paired devices must re-pair):
powershell -ExecutionPolicy Bypass -File packaging\windows\uninstall.ps1 -PurgeConfig
```

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| console task runs, `47992` never listens | `%LOCALAPPDATA%\slipstream\logs\slipstream-web.log`. A missing/!empty `mgmt-token` or a missing `cert.pem`/`key.pem` makes the console refuse to start on purpose (half-configured TLS would serve the login password in the clear). |
| console loads, every API call 401s | token mismatch: the console reads `SLIPSTREAM_MGMT_TOKEN` from `%APPDATA%\slipstream\mgmt-token`. If you passed `--mgmt-token` on the command line, the host never wrote the file. |
| browser warns on the cert | expected: the console uses the host's CN-only self-signed identity cert. Accept once per device. |
| `Bun.serve` missing from `.output` | wrong Nitro preset. Rebuild with `bun run build` in `web/`; the build must use `nitro-entry/bun-https.mjs`. |
| tasks do not start at logon | they are `InteractiveToken` tasks: they run when *this* user logs in interactively. `schtasks /query /tn SlipstreamWeb /v /fo LIST`. |

## Differences from the Linux install

- No `systemd --user`. The tasks are `InteractiveToken` + `HighestAvailable`, so the console and
  host start when the user logs in, not at boot. A session-0 service install is deliberately out of
  scope (the host needs the interactive desktop for WGC/DXGI capture).
- No `.deb`/RPM. `bun.nix` and the Nix packaging are untouched; this directory is standalone.
- The firewall is Windows Firewall (`firewall.ps1`) instead of firewalld/ufw.
