# Comparison matrix

All cells below are either `VERIFIED (source)` with the source given in the corresponding section, `UNKNOWN` where no primary source was found, or an explicitly labelled judgment. Nothing in this matrix is asserted beyond what the sections prove.

Legend for the constraint columns: `pass` = the option does not violate the constraint; `fail` = the option violates it as stated; `partial` = it violates it under conditions the section names; `?` = the constraint is not decidable from primary sources.

## Constraint grid from ADR 0001

| Option | C1 no admin install | C2 no new end-user runtime | C3 cross-platform (Win primary, Linux + Steam Deck exist) | C4 memory and startup footprint | C5 no GPL, no commercial royalty | C6 effort to a polished UI vs the React design system | C7 is a native app even wanted |
|---|---|---|---|---|---|---|---|
| 1 Tauri v2 | partial (NSIS per-user installs to LOCALAPPDATA without admin) | fail (WebView2 runtime) | fail on Steam Deck, partial on Linux | partial | pass | pass (reuses React) | open |
| 2 wry | ? | fail (WebView2 runtime) | fail on Steam Deck, partial on Linux | partial | pass | pass (reuses React) | open |
| 3 Avalonia | pass (self-contained installs without admin rights) | pass if self-contained, fail if framework-dependent | pass | partial | pass | fail (rewrite) | open |
| 4 Flutter and React Native for desktop | partial | pass for Flutter (engine ships in the bundle), fail for React Native Windows (needs C++, MSVC v143 and the Windows App SDK at build time) | pass for Flutter (Linux is Supported and CI-tested), fail for RNW (Windows-only) | partial for Flutter, fail for RNW (Fabric plus Windows App SDK) | pass (BSD-3 and MIT) | fail (rewrite) | open |
| 5 Bun compiled binary with a GUI toolkit | ? | ? | ? | ? | pass | partial | open |
| 7 Keep the web app, serve locally | pass | pass | pass | pass | pass | pass (no new UI at all) | answered against |
| 8 Slint | pass | pass | pass | pass | pass on the Royalty-free tier, fail on GPL, with an attribution obligation | fail (rewrite) | open |
| 9 egui and eframe | pass | pass | pass | pass | pass (MIT or Apache-2.0) | fail (rewrite) | open |
| 10 GPUI | pass | pass | pass | pass | pass (Apache-2.0) | fail (rewrite) | open |
| 11 iced | pass | pass | pass | pass | pass (MIT) | fail (rewrite) | open |
| 12 Win32 shell with optional WebView2 | pass | pass (pure Win32 path) or fail (WebView2 path) | fail (Windows only) | pass | pass | partial | open |

**The Steam Deck result deserves its own note, because it is the one place where a webview option fails a constraint outright.** The Tauri v2 and wry Linux paths link against `libwebkit2gtk-4.1-0` and `libgtk-3-0` (VERIFIED in Option 1 against the Debian bundler documentation and the `webkit2gtk-sys` crate metadata). The Steam Linux Runtime ships no WebKitGTK in any of its four current branches, `soldier`, `sniper`, `steamrt4` and `steamrt5` (VERIFIED in Option 1 against the Steam Runtime ABI expectation manifests), and Arch, which SteamOS derives from, provides the `libwebkit2gtk-4.0.so` soname rather than the `webkit2gtk-4.1` pkg-config name that the Tauri build probes for (VERIFIED in Option 1 against the Arch `webkit2gtk` PKGBUILD). Tauri itself makes no Steam Deck statement at all: across the entire v2 documentation corpus there are zero occurrences of "Steam Deck" and zero of "SteamOS" (VERIFIED in Option 1). A Flatpak route exists in the Tauri distribution guides, but whether the GNOME or KDE Flatpak runtime on SteamOS exposes a `webkit2gtk-4.1` pkg-config is UNKNOWN.

The upstream fix is in motion: wry has open issues 1767 ("Migrating to gtk4/webkit6") and 1474 ("Upgrade wry to gtk4-rs and webkit6") (VERIFIED in Option 1). WebKitGTK 6.0 is the API Arch ships today, so that migration would change the Deck equation once it lands.

INFERENCE, labelled as such: if the Steam Deck is a firm requirement, the webview options need a Flatpak answer that no primary source here confirms, and the browser-based Option 7 is the safe answer for the Deck specifically.

## Axes that matter most for Slipstream

### Axis 1: does the option preserve the existing React 19 console

| Option | Preserves the React console | Why |
|---|---|---|
| 1 Tauri v2 | yes | The React app is the UI, loaded into a WebView2. See Option 1 for the token and proxy story. |
| 2 wry | yes | Same, with the webview chosen by the author. See Option 2. |
| 7 Keep the web app | yes, trivially | It is the same app with a different serving topology. See Option 7. |
| 7 plus a PWA manifest | yes | Chromium installability requires the app to be served over `https` or from `localhost` or `127.0.0.1`, which the existing console origin already satisfies, and an installed PWA launches standalone with no browser chrome (VERIFIED in Option 7 against the MDN PWA installability guide). |
| 12 Win32 shell with WebView2 | yes, optionally | Only the WebView2 variant keeps the React app. See Option 12. |
| 3 Avalonia | no | Requires a new native control tree and a new HTTP client. See Option 3. |
| 4 Flutter, React Native | no for Flutter; partial for React Native | Flutter is a full rewrite. React Native Windows reuses React logic but not the DOM or the CSS design system, and Tailwind has no React Native support (VERIFIED in Option 4). See Option 4. |
| 5 Bun compiled binary | depends on the chosen GUI toolkit | A webview-based Bun shell keeps the React app. See Option 5. |
| 8 to 11 Rust-native toolkits | no | Slint, egui, GPUI and iced all require a rewrite of the 22,411 lines of application code plus 28 design-system files plus 13 vendored design-system files (ADR 0001, lines 236 to 239). See Option 8. |

### Axis 2: what happens to the bearer token

The architecture constraint from ADR 0001 is that the Nitro server is a backend for frontend: the browser calls relative `/api/...` paths, the server-side proxy injects the management bearer token, and the token never lives in the browser (ADR 0001, lines 105 to 107).

| Option | Token position | Consequence |
|---|---|---|
| 1, 2, 7, 12 (webview variants) | unchanged | The browser keeps calling relative paths and the proxy keeps injecting the token. This is the single largest reason the webview options are cheap. |
| 3 Avalonia | native HTTP client holds the token | The native process must obtain and store the management token itself. The native process therefore becomes a second bearer-token holder, which is a security surface the web console does not have today. |
| 8 to 11 Rust-native | native process holds the token | Same as Avalonia, but the token holder is the same Rust binary family as the host. |
| 4 Flutter, React Native | native HTTP client holds the token | Same new bearer-token holder problem. |

### Axis 3: what must be rewritten

| Option | Rewritten | Kept |
|---|---|---|
| 1, 2, 7 | nothing in `web/src` | All 22,411 lines of application code, plus 28 design-system files and 13 vendored design-system files (ADR 0001, lines 236 to 239). |
| 12 (WebView2 variant) | nothing in `web/src` | Same. |
| 12 (pure Win32 variant) | the entire UI | The React app survives only as a fallback path, if it is kept at all. |
| 3, 4, 8 to 11 | the entire UI | Nothing in `web/src` survives except as reference material. |

## Size and licensing facts, only where a section verified them

- Verified sizes, artifact names and binary footprints are stated inside the individual sections. Where a section could not verify a number, it says `UNKNOWN`. Do not read a number in this matrix as coming from anywhere other than the named section.
- Verified license positions are stated in the individual sections. The constraint-relevant finding is on Slint (Option 8). ADR 0001 line 51 flags "Slint has a GPL-or-commercial split with a royalty-free tier, and the exact tier must be stated". The tier is now stated and verified: the Royalty-free edition grants a "world-wide, royalty-free, non-exclusive license to use, reproduce, make available, modify, display, perform, distribute the Software as part of a Desktop, Mobile, or Web Application", closed-source shipping is allowed, there is no revenue threshold on that tier (the 50-employee and 10M EUR thresholds apply only to the paid Commercial plans), and the sole obligation is attribution via the `AboutSlint` widget or a badge. So C5 is satisfiable without a royalty, provided the project takes the Royalty-free tier and shows the attribution, and the GPL tier is avoided entirely (VERIFIED in Option 8 against the Slint FAQ and licence text).

## Rows that this matrix deliberately does not score

- Startup time and resident memory in megabytes for a running app. No section produced a reproducible measurement, and inventing one would violate the evidence rule. Where a section discusses footprint qualitatively, it labels the discussion as a judgment.
- Developer effort in person-days or story points. Not verifiable from primary sources.
- Subjective "prettier" rankings of the toolkits.
- Whether a native app is wanted at all. That is C7, and it is the operator's question, not the matrix's.
