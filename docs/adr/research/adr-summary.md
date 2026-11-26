# Summary of ADR 0001: Desktop dashboard framework for Slipstream

Source: `docs/adr/0001-desktop-dashboard-framework.md` (536 lines).

## ADR 0001: what it decided

- **Title and status.** "ADR 0001: Desktop dashboard framework for Slipstream" (line 1). "Status: Accepted (see \"Decision\" for the winner and the conditions that would change it)" (line 3). "Date: 2026-11-26" (line 4). Sections present: Context (line 10), Constraints (line 41), Repo evidence E1 to E8 (lines 55 to 358), Options assessed (line 360), Evaluation against the constraints (line 386), Decision (line 418), Consequences (line 441), Alternatives considered and rejected (line 479), References (line 503).
- **Chosen framework.** Option 7: "adopt Option 7, keep the web app and serve it locally, plus a Web App Manifest. Do not adopt any GUI framework and do not add a native webview shell now." (lines 420 to 421). Concretely: a Rust launcher subcommand supervising the Nitro console as a child process with a single-instance lock, opening a browser at `http://127.0.0.1:47992` (line 424); a Web App Manifest with `name`, `short_name`, 192px and 512px icons, `start_url`, and `display: standalone` (line 425); and no new end-user runtime, no new GUI dependency in `Cargo.lock`, no second bearer-token holder (line 426).
- **Alternatives evaluated.** Eleven options numbered 1, 2, 3, 4, 5, 7, 8, 9, 10, 11, 12 (Option 6 was never assigned, line 384). Tauri v2 (line 362), wry plus Bun-compiled executable (line 364), Avalonia (line 368), Flutter and React Native for desktop (line 370), Bun compiled binary with a GUI toolkit (line 366), keep the web app and serve locally (line 372), Slint (line 374), egui and eframe (line 376), GPUI (line 378), iced (line 380), Win32 shell with optional WebView2 (line 382). Webview options fail C3 on the Steam Deck; native toolkits fail C6.
- **Web console, token, browser, Rust host API.** "The web console is a **backend for frontend**" (line 31). "The browser calls same-origin relative `/api/...` paths" and "the server-side proxy injects the management bearer token -- the token never lives in the browser" (lines 105 to 107). "The console is the only path from the LAN to the management API" (line 35). The Rust host owns 94 HTTP route handlers: 85 management plus 2 extra plus 7 nvhttp (line 76). Management API on port 47990 (line 63), SSE on `GET /api/v1/events` (lines 82 to 84).
- **Constraints.** C1 "Windows artifacts installable **without admin rights** via winget, scoop, msix, or a portable zip" (line 47). C2 "**No new mandatory end-user runtime**: no .NET SDK, no Flutter SDK, no Dart, no Java, no Python" (line 48). C3 "**Cross-platform**: Windows is primary, but Linux and Steam Deck exist in the repo" (line 49). C4 "**Memory and startup footprint** for an always-running background host with a tray" (line 50). C5 "**Licensing**: no GPL obligation and no commercial royalty on a project the maintainers ship" (line 51). C6 "**Effort to a polished, non-embarrassing UI**, judged against the existing React design-system work" (line 52). C7 "The honest question of whether the operator wants **a branded native app at all**, versus keeping the web app and serving it locally" (line 53). Section header: "These are the constraints the decision must respect. They are real, not invented." (line 43).
- **Windows minimum version, WebView2, Linux/Steam Deck.** No minimum Windows version is stated anywhere. The only version claim is the WebView2 note: "present on Windows 11 and Windows 10 version 2004 and later, but is a separate install on older Windows 10" (lines 357 to 358). "A WebView2 shell adds the WebView2 runtime" (lines 355 to 356). Linux and Steam Deck appear in C3 (line 49) and "a Steam Deck install script, a Decky client, an Android client, and an Apple client" (line 49). The Deck is reached by Option 7 because "the console is just a browser" (line 448).

## Line-numbered evidence table

| Claim | File:line | Verbatim quote (short) |
|---|---|---|
| ADR title | 0001-desktop-dashboard-framework.md:1 | "ADR 0001: Desktop dashboard framework for Slipstream" |
| Status | :3 | "Status: Accepted (see \"Decision\" for the winner and the conditions that would change it)" |
| Date | :4 | "Date: 2026-11-26" |
| Primary decision | :420-421 | "adopt Option 7, keep the web app and serve it locally, plus a Web App Manifest. Do not adopt any GUI framework and do not add a native webview shell now." |
| No GUI framework adopted | :466 | "No Tauri, wry, Avalonia, Flutter, React Native, Slint, egui, GPUI, iced, or Win32 GUI framework is adopted." |
| Console is a backend for frontend | :31 | "The web console is a **backend for frontend**." |
| Token never reaches the browser | :105-107 | "the server-side proxy injects the management bearer token -- the token never lives in the browser" |
| Console is the only LAN path | :35 | "The console is the only path from the LAN to the management API" |
| Rust host route count | :76 | "**Total** \| **94**" |
| Management API port | :63 | "Default port constant `DEFAULT_PORT: u16 = 47990`" |
| SSE handler | :82-84 | "`GET /api/v1/events` is an **SSE** handler" |
| C1 no admin rights | :47 | "Windows artifacts installable **without admin rights** via winget, scoop, msix, or a portable zip" |
| C2 no new mandatory runtime | :48 | "**No new mandatory end-user runtime**: no .NET SDK, no Flutter SDK, no Dart, no Java, no Python" |
| C3 cross-platform, Steam Deck | :49 | "**Cross-platform**: Windows is primary, but Linux and Steam Deck exist in the repo" |
| C4 memory and startup footprint | :50 | "**Memory and startup footprint** for an always-running background host with a tray" |
| C5 licensing | :51 | "no GPL obligation and no commercial royalty on a project the maintainers ship" |
| C6 polished UI effort | :52 | "**Effort to a polished, non-embarrassing UI**, judged against the existing React design-system work" |
| C7 native app or not | :53 | "whether the operator wants **a branded native app at all**, versus keeping the web app and serving it locally" |
| Constraints framing | :43 | "These are the constraints the decision must respect. They are real, not invented." |
| Minimum Windows version | UNKNOWN | No minimum Windows version is stated in the file |
| WebView2 availability | :357-358 | "present on Windows 11 and Windows 10 version 2004 and later, but is a separate install on older Windows 10" |
| WebView2 adds a runtime | :355-356 | "A WebView2 shell adds the WebView2 runtime" |
| Steam Deck packaging | :448 | "It runs as a systemd service in desktop mode and can be added to the Steam library as a non-Steam shortcut in gaming mode" |
| No GUI framework in Cargo.lock | :312-313 | "no GUI framework is present. Any choice adds a new dependency tree" |
| Skia crate is not a desktop GUI | :265-267 | "it is a TUI-adjacent, in-stream, gamepad-driven overlay. It is not a desktop dashboard and cannot be one." |
| Rewrite cost figure | :236-239 | "re-implementing 22,411 lines of application code plus 28 design-system files plus 13 vendored design-system files" |
| No Windows installer | :334-335 | "There is no installer in this repository." |
| Webview shell triggers | :432-437 | "Triggers that would reverse this decision" with wry GTK 4 migration (trigger 1), native OS integration required (trigger 2), React console rewritten (trigger 3), minimum Windows version declared (trigger 4) |
| Option 6 numbering gap | :384 | "There is no Option 6. The numbering used by the research sections skips from 5 to 7" |

## Gaps

The ADR does not decide the following. A fresh decision must still answer them.

- **The minimum Windows version.** Trigger 4 (line 437) names it as a prerequisite for any webview decision, but no floor is declared anywhere in the file. This is the single most concrete open item.
- **Native OS integration requirement.** Trigger 2 (line 435) leaves it to the operator whether tray icon, native file dialogs, native notifications, global hotkeys, or OS-level drag and drop matter. The ADR records this as a product judgement, not a technical one.
- **Whether the React console is ever rewritten.** Trigger 3 (line 436) keeps Options 3, 4, 8, 9, 10, 11 viable only if the 22,411 lines and 41 design-system files are replaced for other reasons. Nothing in the ADR forces or forbids that.
- **wry GTK 4 / WebKitGTK 6.0 landing.** Trigger 1 (line 434) depends on upstream wry issues 1767 and 1474, outside the project's control.
- **Packaging implementation details.** Windows distribution is "a portable zip on GitHub Releases with `self_update`" or "a winget manifest once a real installer exists" (lines 475 and 462). The exact installer, signing, and update mechanism remain to be built.
- **Accessibility and i18n of the PWA path.** The ADR notes Firefox lacks manifest-based install and Safari supports Add to Dock only on macOS Sonoma and later (line 455), but does not resolve how non-Chromium users get the branded experience.
- **The numbering gap.** Option 6 was never assigned (line 384). Any future research that wants a slot 6 must not assume one exists.