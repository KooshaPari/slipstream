# winget manifest scaffolding (not for submission yet)

This directory holds the Windows Package Manager manifest set for the Slipstream Windows MSI.

Status: **scaffolding only**. The MSI does not exist yet. `packaging/msi/Slipstream.msi` is built by
the separate b2-msi lane. Do not submit these files to `microsoft/winget-pkgs` until every
placeholder below is replaced and `winget validate` passes cleanly.

## Identifier decision

`vindeckyy.Slipstream` is a **proposal**, not an existing identifier. The repo has no winget
identifier. The publisher segment follows the canonical upstream GitHub org in `Cargo.toml`
(`https://github.com/vindeckyy/slipstream`). Check the winget-pkgs repository for collisions before
submitting; if the publisher uses a different winget account, change this consistently in all three
files and in the directory layout.

## Files

- `vindeckyy.Slipstream.yaml`: version manifest, PackageVersion 0.24.0 from `Cargo.toml`.
- `vindeckyy.Slipstream.locale.en-US.yaml`: default locale. Publisher, package name, license, URLs
  and short description taken from `Cargo.toml`, `README.md` and `LICENSE-MIT`/`LICENSE-APACHE`.
- `vindeckyy.Slipstream.installer.yaml`: x64 MSI, machine scope (Program Files + machine PATH +
  Windows service), silent switches `/qn` and `/qb`.

MSI payload and shortcut names follow `msi-content-manifest.md`: `slipstream-host.exe`
(service-capable host), `slipstream-client.exe` (current client), `slipstream-tray.exe`,
`slipstream-web.cmd`, `bun\bun.exe`, `web\.output\...`, `slipstream.ico`.

## Placeholders that must be filled

1. `InstallerSha256` in `vindeckyy.Slipstream.installer.yaml`: currently
   `PLACEHOLDER_REPLACE_WITH_REAL_SHA256_AFTER_MSI_BUILD`, which `winget validate` must reject.
2. `ProductCode` in the same file: replace with the real WiX ProductCode GUID once the MSI is built.
3. `InstallerUrl`: currently the expected upstream release asset URL. Confirm the final release and
   asset name before submission.

## Exact hash-fill command

`winget hash` computes the SHA256 winget expects and takes a URL or local path. Once the MSI exists:

    winget hash "C:\Users\koosh\CodeProjects\slipstream\packaging\msi\Slipstream.msi"

Copy the 64-hex output into `InstallerSha256` in `vindeckyy.Slipstream.installer.yaml`, replacing the
placeholder. If `winget hash` is unavailable in a CI context, the equivalent local hash is:

    Get-FileHash -Algorithm SHA256 "C:\Users\koosh\CodeProjects\slipstream\packaging\msi\Slipstream.msi"

The `Hash` value from either command is the value winget expects (uppercase hexadecimal is accepted).

## Remaining work after the MSI builds

1. Build `packaging/msi/Slipstream.msi` in the b2-msi lane.
2. Fill `InstallerSha256` and `ProductCode` with real values; confirm `InstallerUrl`.
3. Re-run `winget validate packaging\winget`; it must pass with no placeholder-related failures.
4. Only then consider a submission to winget-pkgs. This lane does not submit anything.
