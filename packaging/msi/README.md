# Slipstream MSI packaging

WiX v4, x64, per-machine installer for the Slipstream Windows host, tray and CLI.

## Build

```bat
cd packaging\msi
dotnet build Slipstream.wixproj -c Release -p:OutputPath=bin/
```

Output: `packaging\msi\bin\Slipstream.msi`.

The project uses `WixToolset.Sdk/4.0.4` and `WixToolset.Firewall.wixext/4.0.4` as NuGet
`PackageReference`s. `dotnet build` restores both; do not point PATH at a global WiX v7 install
because its extension cache is v7-only and cannot satisfy v4.

## Payload

`Package.wxs` references three binaries from `packaging\msi\payload\`:

| Source | Cargo crate / build command |
|---|---|
| `slipstream-host.exe` | `cargo build --release -p slipstream-host` |
| `slipstream-tray.exe` | `cargo build --release -p slipstream-tray` |
| `slipstream.exe` | `cargo build --release -p slipstream-cli` |

Copy the built exes into `packaging\msi\payload\` before building the MSI. The checked-in payload
files are tiny placeholders used only to compile and validate the installer source; they are not
runnable programs. A release build must replace them with the real artifacts.

The requested `slipstream-client-windows.exe` does not exist anywhere in the `b2-msi` tree. The
only Windows-capable client in this checkout is the headless CLI at `clients/cli`, whose binary is
`slipstream.exe`. A native Windows GUI client build step is still missing upstream.

## Installed layout

`[ProgramFiles64Folder]\Slipstream`: the three exes, `slipstream.ico`, and
`Slipstream Dashboard.url`.

Features:

- Service `slipstream-host`, auto-start, launched as `slipstream-host.exe --service serve`.
  `ServiceControl` starts it on install, stops it on uninstall/upgrade, and waits.
- Machine `PATH` gets `[INSTALLFOLDER]` appended and removed on uninstall.
- Start Menu folder `Slipstream` with Host, Client and Dashboard shortcuts; Dashboard opens the
  `.url` shortcut to `https://127.0.0.1:47992/login`.
- Per-user HKCU Run entry launches `slipstream-tray.exe --autostart`; removed on uninstall.
- Firewall exceptions for TCP 47984/47989/48010 and UDP 47999/9777, removed with the component
  on uninstall. Rules are scoped to `localSubnet`; `Profile="all"` matches the brief.
- `MajorUpgrade` with a stable `UpgradeCode`; version `0.24.0.0` comes from the workspace
  `Cargo.toml` (`version = "0.24.0"`).

## Signing

The MSI is unsigned. A later signing step needs a code-signing certificate with a private key
and `signtool sign /fd SHA256 /f <cert.pfx> /p <password> <msi>`. Sign after the final MSI build
and before distribution; do not sign the placeholder-payload MSI.
