## Option 12: Hand-rolled Win32 shell (windows crate, optionally WebView2)

Two sub-variants are covered: **(a)** a Win32 window that hosts a WebView2 control, and **(b)** pure Win32 with no webview at all.

### 1. License of the `windows` crate and `windows-webview`

**VERIFIED.** Both crates are dual-licensed `MIT OR Apache-2.0`.

- `windows` 0.62.2 reports `license: "MIT OR Apache-2.0"` (https://crates.io/api/v1/crates/windows/0.62.2).
- `windows-webview` 0.100.0 reports the same (https://crates.io/api/v1/crates/windows-webview/0.100.0), and docs.rs renders the badge "MIT OR Apache-2.0" (https://docs.rs/windows-webview/latest/windows_webview/).
- Upstream: https://github.com/microsoft/windows-rs, MIT text at https://github.com/microsoft/windows-rs/blob/master/license-mit ("Copyright (c) Microsoft Corporation.").
- Listing and API docs: https://crates.io/crates/windows-webview and https://docs.rs/windows-webview/latest/windows_webview/.

The local policy file confirms the posture: `windows_rs_policy.md:4-13` says reusable libraries should avoid the `windows` and `windows-sys` umbrella crates because they "add dependency weight and version churn", and points binaries at the umbrella crate and libraries at focused crates such as `windows-window`, `windows-webview`, `windows-canvas`, and `windows-text` (`windows_rs_policy.md:53-65`). `windows_rs_readme.md:3-13` scopes the umbrella crate to "Rust binary applications".

**INFERENCE:** Several of the repo's nine `windows` consumers (`ss-frame`, `ss-clipboard`) look library-shaped, and the policy file would prefer focused crates there. That is pre-existing, not something this option creates.

### 2. Is the repo's `windows` 0.58 current?

**VERIFIED.** No. The repo pins `windows = "0.58"` in nine crates: `crates/slipstream-host/Cargo.toml:136`, `crates/slipstream-tray/Cargo.toml:42`, `crates/ss-capture/Cargo.toml:50`, `crates/ss-clipboard/Cargo.toml:40`, `crates/ss-encode/Cargo.toml:45`, `crates/ss-frame/Cargo.toml:25`, `crates/ss-gpu/Cargo.toml:22`, `crates/ss-inject/Cargo.toml:52`, `crates/ss-vdisplay/Cargo.toml:64`.

- `windows` current `max_stable_version` is **0.62.2** (https://crates.io/api/v1/crates/windows), published 2025-10-06. Delta from the repo pin: **four minor releases** (0.58 -> 0.59 -> 0.60 -> 0.61 -> 0.62). MSRV is `rust_version = 1.82`, satisfied by the repo's pinned `rust-toolchain.toml:9` (`channel = "1.96.0"`).
- `windows-webview` current `max_stable_version` is **0.100.0** (https://crates.io/api/v1/crates/windows-webview), published 2026-09-03, `rust_version = 1.95`, also satisfied by Rust 1.96.0.
- The upstream tree on disk shows the umbrella crate at `"0.100"` (`windows_rs_readme.md:21`) and `windows-webview` at `"0.101"` (`win_webview.md:16`). The focused crates are versioned independently of the umbrella crate, which is why `windows` still reads 0.62.2 while `windows-webview` reads 0.100.0.

**INFERENCE:** `windows-webview` pulls `windows-core` ^0.100, `windows-window` ^0.100, and `windows-reactor` ^0.100, a dependency generation disjoint from the umbrella `windows` 0.58 already in the tree. This option adds a second Windows binding generation rather than migrating one.

### 3. Prerequisites on clean Windows 10 and clean Windows 11

**Sub-variant (a), WebView2 required. VERIFIED.** The WebView2 Runtime is "the underlying (or backing) web platform for WebView2 apps", conceptually "similar to Visual C++ or the .NET Runtime" (https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/evergreen-vs-fixed-version). The Evergreen Runtime "is preinstalled onto all Windows 11 devices as a part of the Windows 11 operating system", and Microsoft "installed the WebView2 Runtime to all eligible Windows 10 devices" (same URL). On Windows 10, "the vast majority of Windows 10 devices have the WebView2 Runtime installed already", but "a small number of Windows 10 devices don't have the WebView2 Runtime installed" (https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution). The app should check for the Runtime before creating a WebView2, "either by checking a registry key or by calling an API", and install it if missing. The Evergreen bootstrapper is "a tiny (approximately 2 MB) installer" for online clients. Fixed Version mode means packaging the runtime yourself, at the cost that "the WebView2 Runtime isn't automatically updated on clients" and "The Fixed Version runtime can't be installed by using an installer" (https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/evergreen-vs-fixed-version).

**Sub-variant (b), pure Win32. VERIFIED.** No prerequisite beyond the operating system. The `windows` crate is bindings to in-box Win32 APIs. The one constraint from the local docs is that `windows-text` "requires Windows 10 version 1703 or later" (`win_text.md:134`), which matters only if variant (b) uses DirectWrite text layout.

**UNKNOWN:** The exact minimum Windows 10 build for Runtime presence on a clean image. Microsoft documents "eligible Windows 10 devices" without a build floor on the evergreen page cited. The repo's ADR asserts Windows 10 version 2004 and later (`docs/adr/0001-desktop-dashboard-framework.md:355-358`), but that is a repo claim, not a Microsoft statement I verified.

### 4. Size

**VERIFIED.** The only size figure with a primary source is the repo's own description of its tray helper: `crates/slipstream-tray/Cargo.toml:15-17` says the tray deliberately avoids depending on `slipstream-host` because "a dependency would drag the whole host (FFmpeg, PipeWire, …) into a 2 MB helper and make it un-buildable standalone".

**UNKNOWN.** Binary size of a WebView2-hosting shell, of a pure-Win32 shell, of `windows-webview` 0.100.0, and of the WebView2 Runtime itself. Microsoft documents the bootstrapper as "approximately 2 MB" but I found no primary-source figure for the full Evergreen Runtime, and the Fixed Version runtime size is deliberately unstated.

### 5. What survives

**Sub-variant (a). VERIFIED, and this is the strongest property of the option.** The web console is a backend for frontend. The browser calls same-origin relative `/api/...` paths, and a nitro/h3 server under `web/server` proxies server-side to the Rust host and injects the bearer token (`web/server/routes/api/[...].ts:62-66` forwards with `authorization: Bearer ${token}`, `web/server/util/forward.ts:31` does the same, and `web/server/middleware/auth.ts:255` documents the token as injected server-side). Because the token never reaches the browser, a WebView2 pointed at `http://127.0.0.1:47992` preserves that chain untouched: the webview is just another browser client of the same BFF the console already serves. The console password gate lives in the BFF (`web/server/middleware/auth.ts:2-4` redirects to `/login` or `/setup` and keeps the API closed when unconfigured), so the webview inherits it for free. Production already runs on port 47992 via `slipstream-web.service` (`docs-site/content/docs/web-console.md:8`).

**INFERENCE:** Variant (a) is a repackaging of the existing web console, not a new UI. The result is the current React console in a native window. That is a feature if the goal is a Windows launcher, and a non-feature if the goal is a differentiated native experience.

**Sub-variant (b). VERIFIED, and the magnitude is severe.** The measured web frontend is 22,411 lines across 166 files and 789,847 bytes in `web/src`. The 41 design-system files (11 primitives under `web/src/components/ui`: badge, button, card, dialog, input, label, spinner, switch, table, tabs, tooltip, plus the wider system) would all have to be reimplemented in raw Win32 and GDI, with no CSS, no Tailwind tokens, no Storybook, no DOM.

**INFERENCE:** Variant (b) is not a GUI option, it is a full client rewrite of a 22k-line application onto a framework with no layout engine, no styling system, and no component library. The SSE stream at `GET /api/v1/events` would need a Win32 message pump integration, the orval-generated typed API client would need hand-written Rust equivalents for 85 axum routes plus 7 nvhttp routes, and Storybook's role as the visual regression harness would have no Win32 equivalent. Realistically a multi-quarter effort with a permanent feature deficit against the web console.

### 6. Token and security

**VERIFIED.** Microsoft requires treating web content as untrusted: "Always check the origin of the document that's running inside WebView2", and the control "may have navigated to another page via the end user interacting with the page or script in the page causing navigation" (https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/security). The concrete control is `NavigationStarting` and `FrameNavigationStarting`: "To prevent your application from navigating to certain pages, use the NavigationStarting and FrameNavigationStarting events to check page or frame navigation, and then conditionally block the navigation" (same URL, referenced at https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2navigationstartingeventargs). Microsoft also names the restriction properties on the settings object: `AreHostObjectsAllowed`, `IsWebMessageEnabled`, `IsScriptEnabled`, `AreDefaultScriptDialogsEnabled` (same URL, referenced at https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2settings).

**Process model. VERIFIED.** "The WebView2 Runtime uses the same process model as the Microsoft Edge browser" (https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/process-model). A process group is "A single browser process. One or more renderer processes. Other helper processes, such as the GPU process and the Audio service process." All processes in a group are tied to one browser process, associated with a single user data folder. Renderer count varies with Site Isolation and with the number of distinct disconnected origins under the same user data folder. The local README corroborates the shape, exposing `on_process_failed` to "Recover from a browser crash" (`win_webview.md:76`).

**INFERENCE:** A pinned single-origin navigation to `http://127.0.0.1:47992` means one renderer for one origin. Because the bearer token lives only in the BFF and is never handed to the renderer, the renderer holds no host credential. The webview's user data folder is a new local browser profile on disk, a small new piece of on-disk state the repo does not have today.

**Preserved posture. VERIFIED.** The console password and the loopback-only first-run setup are enforced in the BFF, not in the UI, so variant (a) inherits both. The setup route rejects non-loopback peers: `if (!isLoopbackAddress(ip))` returns "first-time setup must be completed from the host" (`web/server/routes/_auth/setup.post.ts:35-41`). The mgmt API is "loopback by default, never LAN-exposed" (`web/server/middleware/auth.ts:247-249`), and the proxy is the only place that relaxes TLS verification, scoped to loopback (`web/server/middleware/auth.ts:260-262`). Microsoft's guidance says WebView2 "cannot be run as a system user" and recommends hosting it at "standard (non-elevated) user integrity" (https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/security).

**UNKNOWN:** Whether the self-signed identity cert on 47992 causes a certificate warning inside a WebView2 control, and whether the console's "accept the warning" step (`docs-site/content/docs/web-console.md:23-24`) is portable into the embedded control.

### 7. Polish effort

**VERIFIED.** Variant (a) inherits the entire existing polish stack unchanged: CSS, Tailwind 4 tokens, Storybook, DOM, and browser text rendering all keep working, because the renderer is Chromium. Nothing about the 41 design-system files changes.

**INFERENCE for variant (b).** Everything below becomes hand-rolled Win32 work, and Microsoft documents each as a known burden for raw Win32 desktop apps.

- **DPI.** "Desktop applications using older Windows programming technologies (raw Win32 programming, Windows Forms, Windows Presentation Framework (WPF), etc.) are unable to automatically handle DPI scaling without additional developer work. Without such work, applications will appear blurry or incorrectly-sized in many common usage scenarios" (https://learn.microsoft.com/en-us/windows/win32/hidpi/high-dpi-desktop-application-development-on-windows). DPI changes mid-process are normal: multi-monitor setups, docking a laptop, Remote Desktop connections. Per-monitor v2 requires `SetProcessDpiAwarenessContext` (https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setprocessdpiawarenesscontext) or the manifest approach (https://learn.microsoft.com/en-us/windows/win32/hidpi/setting-the-default-dpi-awareness-for-a-process). The local README says the same: "Resize callbacks report physical pixels. Set any custom DPI policy before creating the first window" (`win_window.md:41-42`).
- **Text rendering.** The browser's text stack is gone. `windows-text` provides DirectWrite layout, measurement, hit-testing, and caret bounds (`win_text.md:15-40`) plus a font fallback chain (`win_text.md:106-127`), but the crate "does not draw", so `windows-canvas` (Direct2D) must paint it (`win_text.md:3-6`, `win_canvas.md:1-11`), including a swap chain and device-loss recovery (`win_canvas.md:46-55`).
- **Dark mode, focus, IME, high contrast. UNKNOWN** as applied to this specific shell. I did not locate primary Microsoft Learn pages for Win32 dark-mode theming, high-contrast handling, or IME composition during this pass. `windows-text` does confirm locale-aware layout ("Formats use the user's locale unless you call `with_locale`", `win_text.md:129-130`) and vertical writing modes for CJK (`win_text.md:83-104`), and `windows-window` documents focus-relevant message handling (`win_window.md:52-56`).

**INFERENCE:** Even done competently, variant (b) would ship a console that looks and feels like a 2005 Windows application next to the browser console it replaces, with none of the accessibility work that 41 design-system files and Storybook currently encode.
### 8. Winget and distribution

**VERIFIED.** The repo has no installer. `docs-site/content/docs/windows-host.md:13` says plainly: "There is no installer in this repository", and the shipped path is `cargo build --release -p slipstream-host -p slipstream-tray` plus Service Control Manager registration: `sc create slipstream-host binPath= "C:\path\to\slipstream-host.exe" start= auto` (`docs-site/content/docs/windows-host.md:54-57`).

**What a NEW winget package would require. VERIFIED.** Winget manifests are YAML files in the `microsoft/winget-pkgs` repository, partitioned by `PackageIdentifier` and `PackageVersion`, and "must be multi-file manifests" with a minimum of a version file, a defaultLocale file, and an installer file (https://cdn.jsdelivr.net/gh/microsoft/winget-pkgs@master/doc/Authoring.md:14-52). The recommended tooling is Winget-Create, which "guides you through the process and validates your manifest" (same URL, `:56`). Validation is `winget validate --manifest <Path to manifest>` (same URL, `:84-88`), and a GitHub App runs the Validation Pipeline on the pull request (same URL, `:90`). The `validate` command "validates a manifest for submitting software to the Microsoft Community Package Manifest Repository on GitHub" (https://learn.microsoft.com/en-us/windows/package-manager/winget/validate). Before submission you must check the package does not already exist via `winget search` and a repo and pull request search (same URL, `:38-41`). Installers need "a stable, version-specific" URL, and application version and publisher must match what the installer writes (same URL, `:94-99`).

**INFERENCE:** A winget package requires publishing a real installer first, which the repo does not have. The manifest is the second-order problem. Variant (a) additionally needs the Evergreen bootstrapper or offline installer reachable at install time, since winget cannot guarantee the WebView2 Runtime is present on a clean Windows 10 machine.

**The `sc create` route that works today. VERIFIED.** It needs no installer, no manifest, and no new third-party tooling: build the two binaries, then `sc create slipstream-host binPath= "..." start= auto` from an elevated prompt (`docs-site/content/docs/windows-host.md:50-57`). The tray looks the service name up in the Service Control Manager and can start or stop it (same URL, `:50-51`).

### 9. Cross-platform

**VERIFIED.** A raw Win32 shell is Windows-only by construction. The `windows` crate does not help on Linux, and the repo has no winit, tao, wry, or webview2 crate in Cargo.lock today.

**SteamOS.** SteamOS is Linux, so both sub-variants fail there. The repo already has a Linux story this option does not touch: the `slipstream-web.service` systemd user unit on port 47992 (`docs-site/content/docs/web-console.md:8`, `scripts/slipstream-web.service`), the SteamOS install script that starts the console as a user service (`docs-site/content/docs/web-console.md:64-65`, `scripts/steamdeck/install.sh:335-349`), and the browser console as the cross-platform fallback (`docs-site/content/docs/windows-host.md:26`).

**UNKNOWN:** Whether the `windows` crate compiles at all on Linux targets. It is declared under `cfg(target_os = "windows")` in the repo's manifests, so it is not compiled on Linux and the question is moot here.

**INFERENCE:** This option is Windows-only while every alternative under consideration (Tauri, Wry, Avalonia, Flutter, React Native, keep-web) is cross-platform. That is a permanent structural cost. The only form of this option that does not inherit the cost is sub-variant (a) framed as "keep the browser console as the primary UI, add a Windows launcher", which is arguably not a GUI option at all but a packaging change.

### 10. Verdict

**INFERENCE, and it is a split verdict.**
**INFERENCE, and it is a split verdict.**

**Sub-variant (b), pure Win32: not competitive.** It requires reimplementing 22,411 lines of React plus 41 design-system files in raw Win32 and GDI, with no CSS, no Tailwind, no Storybook, no DOM, and no browser text rendering, and would ship a visually and accessibly inferior console. It is not a GUI option, it is a client rewrite. Eliminate it.

**Sub-variant (a), WebView2-hosted: runner-up, and the winner among the raw Win32 approaches.** It is the only candidate that adds zero new mandatory end-user runtime beyond the WebView2 Runtime, which Microsoft documents as preinstalled on all Windows 11 devices and on the vast majority of Windows 10 devices (https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/evergreen-vs-fixed-version, https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution). It also adds zero new third-party licensing obligations: both `windows` and `windows-webview` are `MIT OR Apache-2.0` (https://crates.io/api/v1/crates/windows/0.62.2, https://crates.io/api/v1/crates/windows-webview/0.100.0), and the WebView2 Runtime is itself a redistributable Microsoft runtime. The repo already owns raw Win32 tray code with the `windows` crate (`crates/slipstream-tray/Cargo.toml:42-49`, `crates/slipstream-tray/src/platform/windows.rs:255-276`), so the team has demonstrated the pattern. The security posture is preserved by construction, because the bearer token lives in the nitro BFF (`web/server/routes/api/[...].ts:62-66`) and the console password plus loopback-only first-run setup live in the same BFF (`web/server/middleware/auth.ts:2-4`, `web/server/routes/_auth/setup.post.ts:35-41`), never in the browser.

**INFERENCE, why it does not win outright.** Three things keep it out of first place.

First, it is Windows-only, and the repo already has a working cross-platform console story through the systemd unit and the browser (`docs-site/content/docs/web-console.md:8`). Option 12 adds a Windows affordance to a project whose console is currently platform-neutral, which inverts the priority.

Second, it does not actually improve the product. The webview renders the exact same React console that already exists, so every UX problem in the current console remains a problem in the new window. It buys a launcher, not a GUI.

Third, the distribution work is the same distribution work every option needs. The repo has no installer (`docs-site/content/docs/windows-host.md:13`), so the `sc create` route (`docs-site/content/docs/windows-host.md:54-57`) is available immediately for any option, and a winget package requires a real installer plus a multi-file manifest validated with `winget validate` (https://cdn.jsdelivr.net/gh/microsoft/winget-pkgs@master/doc/Authoring.md:47-52, :84-88, https://learn.microsoft.com/en-us/windows/package-manager/winget/validate) regardless of which GUI stack is chosen. Option 12 does not make the installer problem smaller, and for variant (a) it makes it marginally larger because the Evergreen Runtime may be absent on a clean Windows 10 box.

**INFERENCE, the honest headline.** Sub-variant (a) adds zero new mandatory end-user runtime beyond WebView2, zero new third-party licensing obligations, and zero new maintenance of the 22,411-line web app. It reuses the repo's existing `windows` crate muscle memory and preserves the security model by construction. It is the correct answer to "how do we get a Windows launcher with the least new surface", and the wrong answer to "how do we get a better console". If the goal is a Windows-native launcher, take it. If the goal is a better console, improve the web console and ship a launcher, whatever it is made of.
**Summary table**

| Property | Sub-variant (a) WebView2 | Sub-variant (b) pure Win32 |
|---|---|---|
| License | `MIT OR Apache-2.0` (VERIFIED) | `MIT OR Apache-2.0` (VERIFIED) |
| Current crate version | `windows-webview` 0.100.0 (VERIFIED) | `windows` 0.62.2 (VERIFIED) |
| Repo pin gap | umbrella `windows` 0.58 -> 0.62.2 (VERIFIED) | umbrella `windows` 0.58 -> 0.62.2 (VERIFIED) |
| New end-user runtime | WebView2 Runtime (VERIFIED) | none (VERIFIED) |
| Existing web app survives | fully (VERIFIED) | not at all (VERIFIED) |
| New third-party license | none (VERIFIED) | none (VERIFIED) |
| Binary size | UNKNOWN | UNKNOWN |
| Cross-platform | no (VERIFIED) | no (VERIFIED) |
| Verdict | runner-up, winner among raw Win32 | not competitive |
