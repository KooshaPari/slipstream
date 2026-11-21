# Slipstream MSI packaging

WiX v4, x64, per-machine installer source for the Slipstream Windows host, tray and CLI.

## WARNING: current MSI is a structural fixture

`packaging\msi\bin\Slipstream.msi` was built only to prove the WiX source compiles and the File table
lays out correctly. It contains 15-byte `PLACEHOLDER` text files in place of the three executables.
It is **not a product**. Do **not** install it, distribute it, sign it, or treat it as a release
artifact. Think of this filename as `Slipstream.STRUCTURAL-FIXTURE.msi` even though the build output
is named `Slipstream.msi`. Installing it would define a `slipstream-host` service whose binary is a
text file, then fail at start with an opaque SCM error.

The build wrapper now refuses to run WiX until real payload artifacts are present.

## Build

```bat
cd packaging\msi
build-msi.cmd
```

The wrapper checks every payload file before invoking WiX:

| Payload | Floor |
|---|---:|
| `payload\slipstream-host.exe` | 1,000,000 bytes |
| `payload\slipstream-tray.exe` | 1,000,000 bytes |
| `payload\slipstream.exe` | 1,000,000 bytes |
| `payload\slipstream.ico` | 1,000 bytes |

Missing files or files below the floor produce a clear error and a non-zero exit before any MSI is
built. To inspect or compile the authored source only, use `dotnet build Slipstream.wixproj -c Release
-p:OutputPath=bin/` directly, but remember that any resulting MSI with placeholder payloads is a
structural fixture, not a distributable installer.

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

Copy the built exes into `packaging\msi\payload\` before using `build-msi.cmd`. The checked-in payload
files contain the literal text `PLACEHOLDER` and exist only for source validation; they are not
runnable programs and must be replaced before a real MSI build.

The requested `slipstream-client-windows.exe` does not exist anywhere in the `b2-msi` tree. The
only Windows-capable client in this checkout is the headless CLI at `clients/cli`, whose binary is
`slipstream.exe`. A native Windows GUI client build step is still missing upstream.

## Identity and upgrade rules

Do not undo these rules:

- `UpgradeCode` is `6E8D3A28-9F1C-4A2B-8D3E-7C1F4A6B9E52` and is fixed forever. Changing it makes
  Windows treat the next MSI as an unrelated product and leaves two installed copies.
- `ProductCode` is not authored in `Package.wxs`. WiX auto-generates a distinct one for every build,
  which is what allows MajorUpgrade to detect and replace the previous product. Never add a literal
  `ProductCode`. A published version must therefore be built exactly once: rebuilding the same
  version produces a different `ProductCode` and makes the published artifact disagree with copies
  already installed. If a rebuild is unavoidable, bump the version.
- For a release, the winget manifest's `ProductCode` must be read from the actual built MSI (for
  example with `wix msi decompile`). Never copy a `ProductCode` from source or invent one; source has
  no published `ProductCode`.
- `Version` comes from the single `Version` define in `Slipstream.wixproj` and is used in `Package.wxs`
  as `$(Version)`. Releases must pass it explicitly with `-d:Version=<major.minor.build.revision>`;
  the source default is only for local builds. Do not replace the variable with a literal.
- `MajorUpgrade` uses `AllowSameVersionUpgrades="yes"` so a same-version MSI reinstalls over the
  existing copy instead of sitting beside it. This is deliberate and tied to the never-rebuild rule:
  same-version reinstalls are only safe because a published version has exactly one build.
  `DowngradeErrorMessage` blocks downgrades. Keep both.

Because the payload guard refuses to build with placeholder exes, compile validation of these changes
waits for the real release artifacts.

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
from real payload artifacts and before distribution; never sign the placeholder-payload fixture.
