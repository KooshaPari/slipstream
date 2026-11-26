# One line per option

Each line states what the option is, what it costs against ADR 0001, and where the full evidence lives.

| # | Option | One line | Section |
|---|---|---|---|
| 1 | Tauri v2 | Keeps the React console as the UI inside WebView2 and gives a real native window with no rewrite of the 22,411 lines of application code, but it adds the WebView2 runtime on Windows and it cannot ship as a Steam Runtime title on the Deck, because the runtime ships no WebKitGTK 4.1. | Option 1 |
| 2 | wry | Same shape as Tauri with no plugin ecosystem and no opinionated frontend, so it is the smallest Rust webview surface, and it carries the same WebView2 and WebKitGTK 4.1 dependencies. | Option 2 |
| 3 | Avalonia | The best cross-platform native look, and a self-contained build needs no .NET runtime install and installs without admin rights, but it means rewriting the whole React UI plus creating a second bearer-token holder, and Linux is a Tier 3 platform. | Option 3 |
| 4 | Flutter and React Native for desktop | Flutter is the stronger of the two because Linux is a Supported and CI-tested target, so it dodges the WebKitGTK problem that sinks the webview options on the Deck, but it needs the whole UI rewritten and it creates a second bearer-token holder, while React Native Windows is Windows-only, three minor versions behind core, and mid-architecture-migration. | Option 4 |
| 5 | Bun compiled binary with a GUI toolkit | Bun can produce a single-file Windows executable, which is excellent for distribution, but a GUI still needs a separate toolkit and the console-subsystem and Windows API behaviour has to be verified before it can be trusted. | Option 5 |
| 6 | (reserved) | This number was not used in the final numbering. | n/a |
| 7 | Keep the web app, serve it locally | The only option that satisfies every constraint in ADR 0001 with no rewrite, no new runtime and no new token holder, and a PWA install makes it look and behave like a native app with zero native code, but it still has no native OS integration and no real native window. | Option 7 |
| 8 | Slint | A declarative Rust-native toolkit with a good designer story and no extra runtime, and the Royalty-free tier has no revenue threshold and no royalty, only an attribution obligation, but it requires a full rewrite and the GPL tier must be avoided. | Option 8 |
| 9 | egui and eframe | Immediate-mode Rust GUI with no extra runtime and permissive licensing, but the UI is rewritten from scratch and the immediate-mode model does not map onto a CSS design system. | Option 8 |
| 10 | GPUI | The Zed toolkit, Rust-native and Apache-2.0 with a real Direct3D Windows backend, but it is pre-1.0 with breaking changes, and the published crate cannot depend on its own platform layer from crates.io so consumers need a git dependency. | Option 8 |
| 11 | iced | A pure Rust retained-mode GUI with permissive licensing and no extra runtime, but it is a smaller ecosystem and it requires a full rewrite. | Option 8 |
| 12 | Hand-rolled Win32 shell with optional WebView2 | The lightest possible Windows-native process with no extra runtime on the pure Win32 path and full control over the window, but it is Windows only, it does not scale to Linux or Steam Deck, and the WebView2 variant still carries the runtime cost. | Option 12 |

# Three findings that actually decide this

These are the three results that change what should be built, stated with their evidence.

## Finding 1: the token constraint, not the UI, is what makes the native rewrites expensive

The web console works because the Nitro server is a backend for frontend: the browser calls relative `/api/...` paths, the server-side proxy injects the management bearer token, and the token never lives in the browser (ADR 0001, lines 105 to 107, VERIFIED against `docs/adr/0001-desktop-dashboard-framework.md`).

Every option that rewrites the UI (Avalonia, Flutter, React Native, Slint, egui, GPUI, iced) must also replace that proxy with a native HTTP client that holds the management token. That converts a single-token-holder design into a two-token-holder design and creates a new credential-storage surface on every platform, including Linux and Steam Deck, where the current design needs none.

**Judgment, labelled as such:** this is the dominant cost of a native rewrite, and it is larger than the UI rewrite itself. The UI rewrite is visible and painful; the token change is invisible until it is a security incident.

**Consequence:** any native rewrite should be costed with an explicit credential-storage design, not just with a UI feature list.

## Finding 2: the webview tax is real, and it is the only constraint the cheap options actually trip

Every webview-based option (Tauri, wry, a Bun webview shell, the Win32 shell with WebView2) requires a system webview runtime on the end-user machine. On Windows that is the Microsoft Edge WebView2 Runtime, which is a separate redistributable (VERIFIED in Option 1 and Option 12 against the Microsoft WebView2 distribution documentation). On Linux that is WebKitGTK 4.1 (VERIFIED in Option 1 against the `webkit2gtk-sys` crate metadata and the Debian bundler documentation).

This collides with C2 in ADR 0001 line 48, which forbids new mandatory end-user runtimes.

Four facts cut against treating this as disqualifying. First, the WebView2 runtime is included as part of the Windows 11 operating system and was installed by Microsoft on "all eligible Windows 10 devices", so the "separate install" case is a small minority of Windows 10 machines, not the mainstream one (VERIFIED in Option 1 against the Microsoft WebView2 distribution documentation, and in ADR 0001 lines 357 to 358). Second, Microsoft updates the runtime automatically and the app does not need to ship it (VERIFIED, same URL). Third, the Evergreen Bootstrapper is "a tiny (approximately 2 MB) installer" and the per-user install path does not require elevation (VERIFIED, same URL), so it fits C1 as well as C2. Fourth, the pure Win32 path in Option 12 avoids the runtime entirely at the cost of a hand-written UI.

The Steam Deck is where the webview tax stops being a minor annoyance. The Tauri v2 and wry Linux paths link against `libwebkit2gtk-4.1-0` and `libgtk-3-0`, the Steam Linux Runtime ships no WebKitGTK in any of its four current branches, and Arch, which SteamOS derives from, provides the 4.0 soname rather than the 4.1 pkg-config that the Tauri build probes for (all VERIFIED in Option 1 against the Steam Runtime ABI expectation manifests and the Arch `webkit2gtk` PKGBUILD). Tauri makes no Steam Deck statement at all (VERIFIED in Option 1).

**Judgment, labelled as such:** the operator has to decide whether to (a) accept the webview dependency, (b) declare a minimum Windows version and a Deck packaging route, or (c) use the pure Win32 path and treat the Deck separately. Option (b) is the cheapest of the three and is currently missing from the ADR entirely: no minimum Windows version is stated anywhere in `docs/adr/0001-desktop-dashboard-framework.md` (VERIFIED, UNKNOWN in the ADR summary). The Deck half of (b) is not a one-line decision, because the Flatpak runtime extension list could not be read from a primary source and remains UNKNOWN.

**Consequence:** declaring a minimum Windows version is a prerequisite for any webview-based decision, and it is a one-line decision that unblocks three of the options. The Steam Deck question is separate and harder, and it is the one place where a webview option fails a stated constraint outright.

## Finding 3: the cheapest option is the one that changes the serving topology, and it is the only one that leaves the React console untouched

Option 7 keeps the existing Nitro server and React console and changes only how the console is reached: served locally by the Rust host on loopback, or launched by a native shell that points a webview at it.

This is the only option that satisfies C1 through C6 with no rewrite, no new runtime on the end-user machine and no new token holder (Option 7, VERIFIED against the repository's own `web/` layout and the Nitro server configuration). It also does not need a new installer to exist today, which ADR 0001 records as absent at lines 334 to 335.

The PWA result strengthens this materially, and it was not obvious before the research. A Web App Manifest served from `http://127.0.0.1:47992` is installable, because Chromium's installability criteria require the app to be "served using the `https` protocol, or from a local development environment using `localhost` or `127.0.0.1`", which the existing console origin already satisfies (VERIFIED in Option 7 against the MDN PWA installability guide). Once installed, the app "gets an app icon on the device, alongside platform-specific apps" and launches "as a standalone app, rather than a website in a browser", where `standalone` means the PWA looks and feels like a platform application with no browser UI elements (VERIFIED, same URL). Firefox does not support manifest-based PWA install (VERIFIED, same URL), and Safari supports Add to Dock on macOS Sonoma and later (VERIFIED, same URL).

**Judgment, labelled as such:** the branding gap that C7 is really asking about is narrower than it first appears. A PWA covers the start menu entry, the icon, and the browser chrome, with no native code and no new runtime. What a PWA does not cover is native OS integration: no native tray icon, no native file dialogs, no native notifications, no global hotkeys, no OS-level file drag and drop (VERIFIED in Option 7 against the `web.dev` PWA course). If the requirement is "looks like an app", Option 7 plus a PWA already answers it. If the requirement is "integrates with the OS the way a native app does", then a native shell is required and the token and rewrite costs come with it.

**Consequence:** Option 7 is the correct baseline, because it is the option whose cost is knowable. Any native option should be justified as the premium above that baseline rather than as the only path, and the question of what the native app adds over a locally served console plus a PWA is exactly the C7 question the ADR poses at line 53. The trailing answer to that question is narrower than "branded native app at all": it is "native OS integration at all".

# What this document does not decide

- It does not pick a winner. The winner depends on C7, which is the operator's question.
- It does not quote startup times, resident memory or binary sizes that no section verified.
- It does not rate how the toolkits look. That is a judgment a human has to make by opening them.
- It does not assume a minimum Windows version, because the repository does not state one.
- It does not pick a licensing tier for Slint, only to note that the Royalty-free tier has no revenue threshold and no royalty, only an attribution obligation (VERIFIED in Option 8).
