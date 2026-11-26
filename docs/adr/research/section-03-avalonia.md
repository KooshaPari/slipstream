## Option 3: Avalonia (C# and XAML on .NET)

### 1. License

**VERIFIED**: Avalonia is MIT licensed. The canonical file is `https://github.com/AvaloniaUI/Avalonia/blob/master/licence.md` (British spelling), confirmed by the official site's structured data, which pairs that license URL with a price of 0 and a "Free and open-source under the MIT licence" description (https://avaloniaui.net/support).

**VERIFIED**: The MIT license covers the framework itself with no commercial condition. The official FAQ is explicit: "You can build and ship commercial applications with Avalonia completely free. The Community License only applies to the professional tooling (Visual Studio extension, Dev Tools, Parcel), not the framework itself." (https://docs.avaloniaui.net/tools/faq) The Enterprise page agrees: "Do we need Enterprise to build commercial applications? No." (https://avaloniaui.net/support)

**VERIFIED**: Paid tiers cover tooling and premium controls, not the framework. Plus covers DevTools, Parcel, and Hot Reload; Pro adds seven premium controls including Tree Data Grid and Charts; Enterprise adds support and LTS (https://avaloniaui.net/support). A seat is needed only by someone who writes XAML with premium controls, uses the paid tools, or opens a support ticket.

**Inference**: Framework license cost for Slipstream is zero. The only plausible cost is paid tooling or the premium TreeDataGrid and Charts controls.

### 2. End-user runtime requirements

**VERIFIED**: Avalonia targets .NET 8.0 as the minimum for desktop, and Avalonia 12.0 is the current LTS release (https://docs.avaloniaui.net/docs/supported-platforms).

**VERIFIED**: A framework-dependent build requires the .NET Desktop Runtime, not the base .NET Runtime, whose table shows the base runtime includes the .NET Runtime only while the Desktop Runtime includes both and "runs desktop apps" (https://learn.microsoft.com/en-us/dotnet/core/install/windows).

**VERIFIED**: A self-contained build ships the runtime with the app, so "the environment that runs the app doesn't need to have the .NET runtime preinstalled" (https://learn.microsoft.com/en-us/dotnet/core/deploying/). The tradeoff is larger downloads and disk usage, and the app no longer rolls forward to the latest security patch, so runtime updates only by shipping a new app version (same source).

**VERIFIED**: .NET is not preinstalled on Windows. Microsoft Learn treats installation as a required user step, listing the .NET Installer, WinGet, the `dotnet-install` PowerShell script, and manual extraction as the install paths (https://learn.microsoft.com/en-us/dotnet/core/install/windows). No primary source states that negative claim in those exact words, so the direct sentence is **UNKNOWN** and the documented install-required posture stands in.

**VERIFIED**: A self-contained build installs without administrator rights. Avalonia's Linux packaging guide publishes with `--self-contained true` and notes "self-contained is recommended, so final users won't need to install .NET" (https://docs.avaloniaui.net/docs/deployment/linux), and `dotnet-install.ps1` is "recommended for continuous integration and nonadmin installs" (https://learn.microsoft.com/en-us/dotnet/core/install/windows).

### 3. WebView dependency

**VERIFIED**: Avalonia does NOT use WebView2 for core rendering. "It uses its own rendering engine to draw controls, so your app looks and behaves the same on every platform", with "No native control wrappers, no platform-specific quirks" (https://docs.avaloniaui.net/docs/welcome). Rendering is Skia by default.

**VERIFIED**: WebView2 appears only in the optional `Avalonia.Controls.WebView` package, which "leverages the platform's native web rendering capabilities" and does not bundle Chromium (https://docs.avaloniaui.net/docs/app-development/embedding-web-content). On Windows it uses Edge WebView2, on macOS and iOS `WKWebView`, on Linux WebKitGTK or WPE WebKit. It is not required for a standard application.

**VERIFIED**: `Avalonia.Headless` is a separate package replacing windowing and rendering with in-memory implementations, exposing the full control tree, layout, styling, and data binding (https://docs.avaloniaui.net/docs/testing/setting-up-the-headless-platform). It is a testing tool, not a browser-backed renderer.

**Consequence (inference)**: An Avalonia app in XAML does not reuse the existing React UI, since Avalonia UI is authored in XAML and C# (https://docs.avaloniaui.net/docs/welcome).

### 4. Size

**UNKNOWN**: No primary source states a numeric figure for the size of a self-contained or single-file Avalonia application. The single-file page describes size only qualitatively: "The size of the single file in a self-contained application is large since it includes the runtime and the framework libraries" (https://learn.microsoft.com/en-us/dotnet/core/deploying/single-file/overview). The publishing overview lists self-contained deployments under "Larger deployments" with "greater download size and hard drive space required" (https://learn.microsoft.com/en-us/dotnet/core/deploying/). No byte count appears on either page.

**VERIFIED**: The mitigation levers are documented. Trimming works only for self-contained apps and "can significantly reduce the total size of trim-compatible applications" (https://learn.microsoft.com/en-us/dotnet/core/deploying/trimming/trim-self-contained), and Native AOT is "Often smaller than self-contained deployment with the full runtime" (https://learn.microsoft.com/en-us/dotnet/core/deploying/).

### 5. What survives of the existing work

**VERIFIED**: Avalonia UI is authored in XAML and C#, with a CSS-inspired styling system using selectors, style classes, pseudoclasses, and control themes (https://docs.avaloniaui.net/docs/welcome).

**Blunt assessment**: Of the 166 source files and roughly 790 KB of React, Tailwind, Storybook, and TypeScript, essentially none of the UI survives. React components do not run in Avalonia, Tailwind utility classes do not apply because Avalonia's styling is CSS-inspired but not CSS (https://docs.avaloniaui.net/docs/styling/styles), and Storybook stories do not apply because Avalonia has no Storybook equivalent; its DevTools attach to a running app rather than rendering components in isolation (https://docs.avaloniaui.net/docs/welcome).

**What does survive**: the API contract. The orval-generated typed client encodes the 85 axum routes plus 7 GameStream-compat routes as TypeScript types, and stays valuable as the specification of the Rust host's API surface, transliterable into C# records. The 16 feature sections remain usable as the information architecture checklist. Everything visual is rewritten, so this is a full rewrite from React to XAML plus C# with no component reuse (inference). The existing work buys an API map and a proven information architecture, not UI code.

### 6. Effort to a polished non-embarrassing UI

**VERIFIED**: Avalonia ships 60+ built-in controls including DataGrid, TreeView, TabControl, and Calendar, all "fully styleable and templatable" (https://docs.avaloniaui.net/docs/welcome).

**VERIFIED**: Theming is first class. Control themes are styles with a `TargetType` that can replace a control's complete look, live in a `ResourceDictionary`, and are assigned via the `Theme` property. Two open-source themes ship in the repository: Simple Theme and Fluent Theme (https://docs.avaloniaui.net/docs/styling/control-themes). Theme variants handle light and dark (https://docs.avaloniaui.net/docs/styling/theme-variants). The styling primitives that map onto design-system work are all present: style classes, pseudoclasses, nested styles for interaction states, control templates, container queries, and typography (same source).

**Blunt assessment**: The team already knows React and TypeScript, so Avalonia adds two new learning surfaces, XAML and C#, with XAML carrying the bulk of the ramp-up cost since it has no analogue in the current stack. A polished UI is achievable because Fluent Theme gives a credible starting point, but a distinctive one requires authoring control themes and templates by hand. Rewriting 16 feature sections of a 790 KB dashboard is a comparable order of effort to the original build, since XAML is not faster to write than JSX, though compiled bindings checked at build time catch errors early (https://docs.avaloniaui.net/docs/welcome).

### 7. Winget and distribution

**VERIFIED**: The winget contributing documentation is at `https://github.com/microsoft/winget-pkgs/tree/master/contributing`, with the manifest authoring spec at `https://github.com/microsoft/winget-pkgs/blob/master/doc/manifest/schema`. Microsoft's authoring guide is at https://learn.microsoft.com/en-us/windows/package-manager/package/manifest.

**VERIFIED**: There is no first-party Slipstream winget package. A new manifest is YAML under `manifests/<publisher>/<package>/<version>/` declaring `PackageIdentifier`, `PackageVersion`, `Publisher`, `PackageName`, `License`, `ShortDescription`, and an `Installers` block with `Architecture`, `InstallerType`, `InstallerUrl`, and `InstallerSha256` (https://learn.microsoft.com/en-us/windows/package-manager/package/manifest). The schema is at version 1.12.0. `InstallerType` includes `exe`, `msi`, `msix`, `inno`, `wix`, `nullsoft`, `appx`, and `font`, so a self-contained Slipstream build ships as an `exe` with a SHA256 unless packaged with WiX or Inno Setup. Constraints: unique package identifier, one pull request per package version, silent install required, and `PackageName` and `Publisher` must match the Add/Remove Programs entries (same source). Manifests are "automatically validated and may also be reviewed manually", and the `wingetcreate` utility scaffolds and submits one (https://learn.microsoft.com/en-us/windows/package-manager/).

**VERIFIED**: The alternative route is a framework-dependent app plus a bootstrapper that installs the .NET Desktop Runtime via the .NET Installer, WinGet (`winget install Microsoft.DotNet.DesktopRuntime.10`), the `dotnet-install.ps1` script for nonadmin installs, or manual zip extraction with `DOTNET_ROOT` set (https://learn.microsoft.com/en-us/dotnet/core/install/windows, https://learn.microsoft.com/en-us/dotnet/core/install/).

### 8. Cross-platform including Linux and Steam Deck

**VERIFIED**: Avalonia supports Desktop Linux via X11 directly, with a native Wayland backend available as an opt-in package starting with Avalonia 12.1.0 (https://docs.avaloniaui.net/docs/supported-platforms).

**VERIFIED**: The Linux matrix is tiered. Tier 1 is Ubuntu 25.x (x64, ARM64), Fedora 43 (x64), and Debian 13 Trixie (x64, ARM64); Tier 2 covers older Ubuntu, Fedora, and Debian; Tier 3 covers everything else, including Arch, Alpine, NixOS, and Gentoo. Tier 1 is "Tested against every release. Bugs on Tier 1 platforms get fixed" (https://docs.avaloniaui.net/docs/supported-platforms).

**VERIFIED**: Required Linux packages are `libx11-6 libice6 libsm6 libfontconfig1` (https://docs.avaloniaui.net/docs/supported-platforms, https://docs.avaloniaui.net/docs/deployment/linux). Rendering is OpenGL via Skia on X11, OpenGL or OpenGL ES via EGL on Wayland, and software rendering on framebuffer. Skia is built against glibc 2.17, so musl-based distributions need a custom `libSkiaSharp.so` build and sit in Tier 3.

**UNKNOWN**: Avalonia has no official Steam Deck or SteamOS support. The documentation site contains no Steam or Deck page at all (a full scan of the sitemap at https://docs.avaloniaui.net/sitemap.xml returns zero matches for either term), and the supported-platforms matrix never mentions SteamOS or Steam Deck.

**Inference**: SteamOS 3 is Arch-based, placing it in Avalonia's Tier 3 bucket for Linux, so a Steam Deck build is plausible but not vendor-supported. The framework does ship an embedded-Linux path with framebuffer and DRM rendering at Tier 1 for Raspberry Pi OS (https://docs.avaloniaui.net/docs/supported-platforms), and Valve documents the Steam Linux Runtime 4 container approach (https://gitlab.steamos.cloud/steamrt/steamrt4/sdk), so a containerized build is the plausible route.

### 9. Cross-language interop

**VERIFIED**: The Avalonia documentation set has no dedicated HTTP client page. The Services section covers clipboard, file dialogs, storage, and focus management (https://docs.avaloniaui.net/docs/services/clipboard). Networking comes from the base .NET class libraries, principally `System.Net.Http.HttpClient`, which is standard .NET rather than an Avalonia-specific facility.

**VERIFIED**: The existing architecture fits this cleanly. The Rust host already serves `https://127.0.0.1:47990` with bearer-token auth over 85 axum routes plus 7 GameStream-compat routes, and the console password flow and loopback-only first-run setup are server-side concerns (repo facts taken as given). A C# client using `HttpClient` with a handler that pins the bearer token on outgoing requests is sufficient. Native platform interop is a supported pattern via `TryGetPlatformHandle()` (https://docs.avaloniaui.net/docs/app-development/native-interop).

**Inference (clearly labelled)**: Embedding Rust via a C ABI, compiling the axum host as a `cdylib` and calling it through P/Invoke, is technically possible and would remove the HTTP hop and the bearer-token handling. But no primary source describes this pattern for an Avalonia client, the tradeoffs are substantial (no HTTP isolation, the Rust side loses its async runtime model, and the SSE stream at `GET /api/v1/events` would need a different transport), and it carries materially more engineering risk than an HTTP client. Recommendation: use HTTP, matching the existing web console.
