# ADR 0001: Desktop dashboard framework for Slipstream

- Status: Accepted (see "Decision" for the winner and the conditions that would change it)
- Date: 2026-11-26
- Deciders: Slipstream maintainers
- Supersedes: nothing
- Superseded by: nothing
- Related: `docs/adr/README.md`

## Context

Slipstream is a Rust game-streaming host plus client. The management dashboard today is a
web application: React 19 + Vite 7 + Tailwind 4 + TypeScript 5.9, with 15 feature
sections, a Nitro/h3 server under `web/server`, Storybook, an orval-generated typed API
client, and Biome.

The Rust side is the source of truth. The GUI has to do three things well:

1. Control host lifecycle (start, stop, restart, shutdown, config, display layout,
   plugin store, updates).
2. Show pairing and session state.
3. Show logs and stats.

The decision is which framework the desktop dashboard is built on, if any. The honest
framing is that this is a decision about a **native shell around an existing web
console**, not a decision about whether to keep the web console. Section "Repo evidence"
establishes the coupling that decides most of the options.

### The shape of the problem, in one paragraph

The web console is a **backend for frontend**. The browser calls same-origin relative
`/api/...` paths. A Nitro/h3 server proxies server-side to the Rust management API over
loopback HTTPS and injects the `SLIPSTREAM_MGMT_TOKEN` bearer, so the token never reaches
the browser. There is no CORS, no Rust-to-web push, no websocket. The console is the only
path from the LAN to the management API. Both processes are required: the console returns
502 with no host running. That means any "native" option either embeds the existing React
app in a webview pointed at the Nitro server (preserving the same-origin proxy path), or
reimplements that proxy inside the Rust host, or throws the React app away and rebuilds
all 15 sections in a different toolkit.

## Constraints

These are the constraints the decision must respect. They are real, not invented.

| # | Constraint | Why it binds | Evidence |
|---|---|---|---|
| C1 | Windows artifacts installable **without admin rights** via winget, scoop, msix, or a portable zip | End users install the host themselves. An MSI requiring elevation, or an app that must live in `Program Files`, fails the constraint | `docs-site/content/docs/windows-host.md` (repo docs); Windows tray crate already uses raw Win32 without elevation |
| C2 | **No new mandatory end-user runtime**: no .NET SDK, no Flutter SDK, no Dart, no Java, no Python | An end user must be able to run the dashboard after a one-click install | `web/package.json` currently requires Bun only for the *server* side, and Bun is vendored into the Linux package (`packaging/debian/build-web-deb.sh`) |
| C3 | **Cross-platform**: Windows is primary, but Linux and Steam Deck exist in the repo | The repo has Linux systemd packaging, a Steam Deck install script, a Decky client, an Android client, and an Apple client | Root `Cargo.toml` workspace members; `packaging/`, `scripts/steamdeck/`, `clients/` |
| C4 | **Memory and startup footprint** for an always-running background host with a tray | The host already runs FFmpeg, Vulkan, PipeWire, and a QUIC control plane. The dashboard must not be the thing that makes the host heavy | `crates/slipstream-tray/Cargo.toml:14-19` states the tray deliberately does **not** depend on `slipstream-host` so it does not drag FFmpeg/PipeWire into a 2 MB helper |
| C5 | **Licensing**: no GPL obligation and no commercial royalty on a project the maintainers ship | Slint has a GPL-or-commercial split with a royalty-free tier, and the exact tier must be stated. Avalonia is MIT, egui is MIT or Apache-2.0, GPUI is Apache-2.0, Flutter is BSD-3, React Native is MIT, Tauri is MIT or Apache-2.0 | Workspace `license = "MIT OR Apache-2.0"` (root `Cargo.toml` `[workspace.package]`) |
| C6 | **Effort to a polished, non-embarrassing UI**, judged against the existing React design-system work | The existing console has a vendored design system (`web/vendor/unom-ui`, `web/vendor/unom-style`), an Observatory component family, Storybook, and screenshot tooling | `web/src/components/ui/` (11 files), `web/src/components/observatory/` (9 files), `web/vendor/unom-ui/` (11 files), `web/vendor/unom-style/` (2 files) |
| C7 | The honest question of whether the operator wants **a branded native app at all**, versus keeping the web app and serving it locally | A native app is a second implementation to maintain. If the web console plus a tray shortcut is good enough, the native rewrite is a cost with no user-visible payoff | `crates/slipstream-tray/src/platform/windows.rs` already has an "Open web console" menu item |

## Repo evidence

### E1. The Rust host owns 94 HTTP route handlers

The Rust tree contains exactly two HTTP servers, both in `crates/slipstream-host`:

- Management API: axum, HTTPS, default port 47990, router at `crates/slipstream-host/src/mgmt.rs:148`
  (`fn app`), nested under `/api/v1` at `crates/slipstream-host/src/mgmt.rs:188`.
  Default port constant `DEFAULT_PORT: u16 = 47990` at `crates/slipstream-host/src/mgmt.rs:54`.
- nvhttp (Moonlight/GameStream compatibility): axum, 7 routes, router at
  `crates/slipstream-host/src/protocol/gamestream/nvhttp.rs:89` (`fn router`).

Route handler count, cross-checked three independent ways (the `routes!(...)` macro
expansion, the `#[utoipa::path(...)]` attributes, and a parse of the checked-in
`api/openapi.json`):

| Group | Count |
|---|---|
| Management API under `/api/v1` | 85 |
| Management API extra paths (`/api/docs`, `/api/v1/openapi.json`) | 2 |
| nvhttp GameStream-compat | 7 |
| **Total** | **94** |

The 85 management operations match `api/openapi.json` exactly.

`GET /api/v1/health` is the **only** route exempt from auth
(`crates/slipstream-host/src/mgmt/host.rs:250`, `security(())`).
`GET /api/v1/events` is an **SSE** handler
(`crates/slipstream-host/src/mgmt/events.rs:119`, using
`axum::response::sse::{Event, KeepAlive, Sse}` at `crates/slipstream-host/src/mgmt/events.rs:21`).

Trees confirmed to contain **zero** HTTP routes: `crates/ss-client-core/src`,
`crates/ss-console-ui/src`, `clients/cli/src`, `clients/shared`, `clients/apple`,
`clients/linux`, `clients/decky`, `clients/probe`, `clients/session`.
`clients/windows/src` **does not exist**: `clients/` contains exactly
`android, apple, cli, decky, linux, probe, session, shared`.

The route-handler count is reproducible from the repo alone: grep the `routes!` macro
expansion in `crates/slipstream-host/src/mgmt.rs`, grep the `#[utoipa::path(...)]`
attributes under `crates/slipstream-host/src/mgmt/`, and parse `api/openapi.json`. All three
agree on 85 management operations. The exact commands are in "How to verify" below.

### E2. The web console is a backend for frontend, not a thin static page

The browser never talks to the Rust host directly.

1. Browser to console, same origin. The orval-generated client calls the mutator
   `apiFetch` at `web/src/api/fetcher.ts:22`, which does a plain
   `fetch(url, { credentials: "same-origin" })` at `web/src/api/fetcher.ts:29-33` against a
   **relative** `/api/...` path. The comment at `web/src/api/fetcher.ts:6-8` states it:
   "requests are same-origin to `/api/...`; the browser sends only the session cookie (the
   server-side proxy injects the management bearer token -- the token never lives in the
   browser)."
2. Console to Rust host, server-side. The catch-all route
   `web/server/routes/api/[...].ts:20` builds
   `target = ${mgmtUrl()}${pathname}${search}` at `web/server/routes/api/[...].ts:38` and calls
   h3 `proxyRequest(event, target, ...)` at `web/server/routes/api/[...].ts:62`, with
   `authorization: Bearer ${mgmtToken()}` injected at `web/server/routes/api/[...].ts:66` and
   the browser cookie stripped at `web/server/routes/api/[...].ts:68`.

The comment at `web/server/routes/api/[...].ts:1-5` says it outright: "We inject the
management bearer token server-side (the browser never sees it) ... The management API
itself binds loopback only -- this proxy is ONLY path to it from the LAN, and it's
authenticated."

| Item | Value | Source |
|---|---|---|
| Rust mgmt API URL (default) | `https://127.0.0.1:47990` | `web/server/util/auth.ts:251-253` (`mgmtUrl()`) |
| Env var, mgmt URL | `SLIPSTREAM_MGMT_URL` | `web/server/util/auth.ts:252` |
| Env var, mgmt bearer | `SLIPSTREAM_MGMT_TOKEN` | `web/server/util/auth.ts:256-258` (`mgmtToken()`) |
| Console port | `PORT=47992`, `HOST=0.0.0.0` | `web/web.env.example:16-17` |
| Console dev port | 47992 on `127.0.0.1` | `web/package.json:12` |

TLS on the loopback hop is relaxed only for that one hop, using Bun's per-request `tls`
extension, never a process-wide `NODE_TLS_REJECT_UNAUTHORIZED`
(`web/server/routes/api/[...].ts:56-60`, `web/server/util/forward.ts:36-41`,
`web/server/routes/api/v1/events.get.ts:46-51`, `web/server/util/pluginProxy.ts:54-56`).
The relaxation is conditional on the URL being loopback (`isLoopbackUrl`,
`web/server/util/auth.ts:262-273`).

The one streaming channel is SSE and it is **pulled**, not pushed. The generic
`proxyRequest` cannot stream, so SSE has its own route
`web/server/routes/api/v1/events.get.ts:24`, which fetches
`${base}/api/v1/events${search}` at `web/server/routes/api/v1/events.get.ts:55` and returns
the upstream `ReadableStream` untouched at `web/server/routes/api/v1/events.get.ts:79-87`.
The reason is documented at `web/server/routes/api/v1/events.get.ts:1-15`: h3's
`proxyRequest` buffers writes, so frames "arrive at the browser together, ~3 s late, when
the upstream closes. For an SSE stream that is fatal".

There is a second, separate reverse proxy for plugin UIs: `/plugin-ui/<id>/**` goes to a
plugin's own loopback UI server, and the per-boot secret is fetched server-side at
`web/server/util/pluginProxy.ts:57` and never reaches the browser
(`web/server/util/pluginProxy.ts:30-36` denies the credential leaf to the generic
passthrough).

### E3. Auth is password-for-human, bearer-for-machine

| Layer | Mechanism | Source |
|---|---|---|
| Browser to console | Session cookie `ss_session`, sealed by h3 `useSession` | `web/server/util/auth.ts:75` (`SESSION_NAME`), `web/server/util/auth.ts:289-326` |
| Console to Rust host | HTTP bearer token, injected server-side | `web/server/routes/api/[...].ts:66` |
| Console login | A single **shared console password** | `web/server/routes/_auth/login.post.ts:49-57` |
| Rust mgmt API | Bearer token **or** a paired client mTLS cert | `crates/slipstream-host/src/mgmt/auth.rs:26` (`require_auth`) |

The console password comes from `SLIPSTREAM_UI_PASSWORD` or the file
`~/.config/slipstream/web-password` (`web/server/util/auth.ts:187-215`). It is not the API
token. The login field help text says exactly that: "The management password created
during first-time setup. Signs you into this console only."
(`web/src/sections/Login/view.tsx:95`).

- First run, no password configured: `configured = Boolean(uiPassword())` at
  `web/server/middleware/auth.ts:61`. `/setup` is served
  (`web/server/middleware/auth.ts:62-63`), `/login` redirects to `/setup`
  (`web/server/middleware/auth.ts:66-69`), all `/api` returns **503**
  (`web/server/middleware/auth.ts:74-78`).
- First-run setup is **loopback-only**. `POST /_auth/setup` refuses a non-loopback peer
  with **403** "first-time setup must be completed from the host"
  (`web/server/routes/_auth/setup.post.ts:36-41`). Password minimum length is 8
  (`web/server/util/auth.ts:76`, enforced at `web/server/routes/_auth/setup.post.ts:59-65`).
- After setup: `POST /_auth/login {password}` with a constant-time compare
  (`web/server/routes/_auth/login.post.ts:51`) and a per-IP throttle
  (`web/server/routes/_auth/login.post.ts:40-47`).
- Every later request needs a session whose `epoch` matches
  (`web/server/middleware/auth.ts:86-90`), otherwise `/api` returns **401**
  (`web/server/middleware/auth.ts:92-95`) and pages redirect to `/login`
  (`web/server/middleware/auth.ts:97-101`).
- Every mutating request must be same-origin per `Sec-Fetch-Site`
  (`web/server/middleware/auth.ts:51-59`).
- `vite dev` has **no login gate at all**. `web/vite.config.ts:288-290` says so:
  "The host requires a token on every route now, so without this `bun run dev` 401s on
  every call and `apiFetch` bounces the developer to /login -- where logging in doesn't
  help, **because dev has no login gate at all**."

### E4. The measured React surface

Counts are of `.ts`, `.tsx`, `.js`, `.jsx`, `.css`, `.mjs`, `.mts` files, measured with
`Get-ChildItem -Recurse` over the worktree at `C:\Users\koosh\Agents\worktrees\slipstream\gui-decision`.

| Area | Files | Bytes | Lines |
|---|---|---|---|
| `web/src/sections` | 76 | 553,691 | 15,557 |
| `web/src/components` | 28 | 106,259 | 2,920 |
| `web/src/routes` | 24 | 7,400 | 180 |
| `web/src` (whole) | 166 | 789,847 | 22,411 |
| `web/server` | 18 | 65,825 | 1,596 |
| `web/nitro-entry` | 1 | 6,032 | 111 |

Per feature section:

| Section | Files | Bytes | Lines |
|---|---|---|---|
| Automation | 2 | 23,853 | 659 |
| Config | 10 | 104,993 | 3,250 |
| Dashboard | 7 | 34,054 | 912 |
| Displays | 4 | 82,404 | 2,242 |
| Host | 9 | 62,531 | 1,743 |
| Library | 9 | 41,753 | 1,217 |
| Login | 2 | 6,291 | 177 |
| Logs | 4 | 21,943 | 560 |
| Pairing | 6 | 31,133 | 808 |
| Plugins | 1 | 11,795 | 301 |
| Sessions | 2 | 10,298 | 301 |
| Settings | 1 | 4,966 | 150 |
| Setup | 2 | 7,121 | 217 |
| Stats | 9 | 45,627 | 1,272 |
| Store | 8 | 64,929 | 1,748 |

Design-system surface, which is the part a rewrite has to replace, not just re-code:

| Path | Files | Bytes | Contents |
|---|---|---|---|
| `web/src/components/ui` | 11 | 17,617 | `badge, button, card, dialog, input, label, spinner, switch, table, tabs, tooltip` |
| `web/src/components/observatory` | 9 | 29,269 | `AlertStrip, EventTimeline, MetricCard, PageHeader, StatusIndicator, Summary` plus `index.ts`, `state.tsx`, `types.ts` |
| `web/src/components` (top level) | 8 | | `app-shell.tsx`, `brand-mark.tsx`, `command-palette.tsx`, `logo.tsx`, `option-help.tsx`, `os-icon.tsx`, `query-state.tsx`, `wordmark.tsx` |
| `web/vendor/unom-ui` | 11 | 12,828 | `button, card, dialog, material, section, tabs, toast` plus `form/checkbox`, `form/input-text`, `form/label` |
| `web/vendor/unom-style` | 2 | 365 | `src/index.ts` (the Tailwind 4 token layer) |

The two largest sections by byte count are **Config** (104,993 bytes, 3,250 lines) and
**Displays** (82,404 bytes, 2,242 lines). Together with **Host** (62,531) and **Store**
(64,929) they account for 314,857 bytes, which is 39.9 percent of `web/src`.

**This is the rewrite-cost number.** Rebuilding the console in a toolkit that cannot
reuse React means re-implementing 22,411 lines of application code plus 28 design-system
files plus 13 vendored design-system files, and the four heaviest sections alone are
315 KB of it.

The typed API client is generated, not hand-written. `web/orval.config.ts:8-10` reads the
Rust-generated OpenAPI document at `../api/openapi.json` and generates the typed React
Query client into `web/src/api/gen`. Regeneration commands: `pnpm api:gen` (web side) and
`cargo run -p slipstream-host -- openapi` (Rust side), per `web/orval.config.ts:4-5`.

### E5. `crates/ss-console-ui` is a Skia in-stream overlay, not a desktop GUI

`crates/ss-console-ui/Cargo.toml` describes it as "The Skia console UI for the Vulkan
session binary -- stats OSD, capture HUD, and (next) the gamepad library; renders on the
presenter's device via the Overlay contract".

It is a Linux-only crate. Every module is gated `#[cfg(target_os = "linux")]`
(`crates/ss-console-ui/src/lib.rs:20-46`). It depends on `skia-safe` with
`features = ["vulkan", "textlayout"]` and on `ash`, and links the system SDL3
(`crates/ss-console-ui/Cargo.toml`, `sdl3 = { version = "0.18", ... }`).

Measured size: **18 `.rs` files, 266,617 bytes, 6,741 lines**.

It renders into the **presenter's** Vulkan device as offscreen RGBA the presenter
composites as one premultiplied quad (`crates/ss-console-ui/src/lib.rs:1-5`). It is the
couch shell on the client side: home, library coverflow, settings, add-host, PIN pairing
(`crates/ss-console-ui/src/ui/screens.rs:6-15`, and the screen files
`crates/ss-console-ui/src/ui/screens/{add_host,home,library,pair,settings}.rs`).

**Conclusion: it is a TUI-adjacent, in-stream, gamepad-driven overlay. It is not a desktop
dashboard and cannot be one.** Its window model, its input model (gamepad plus SDL
text), its rendering model (Vulkan overlay), and its platform support (Linux only) are all
wrong for a desktop management console. It should not be counted as existing GUI
infrastructure for this decision.

### E6. `Cargo.lock` contains no GUI framework today

`Cargo.lock` has **564** packages. A scan for GUI and windowing crates returns:

| Crate | In `Cargo.lock`? | Line |
|---|---|---|
| `wry` | absent | |
| `tauri` | absent | |
| `slint` | absent | |
| `egui` | absent | |
| `eframe` | absent | |
| `gpui` | absent | |
| `iced` | absent | |
| `webview2`, `webview2-com` | absent | |
| `winit` | absent | |
| `tao` | absent | |
| `ratatui`, `crossterm` | absent | |
| `native-windows-gui` | absent | |
| `servo` | absent | |
| `windows` | **present** | 5235 |
| `windows-core` | present | 5245 |
| `windows-sys` | present (multiple) | 5305+ |
| `windows-result` | present | 5286 |
| `windows-strings` | present | 5295 |
| `windows-targets` | present | 5350+ |
| `winapi` | present | 5204 |

The `windows` crate **is** already in the tree, at version `0.58`, used by nine crates for
Win32 API access (not for a GUI toolkit):
`crates/slipstream-host/Cargo.toml:136`, `crates/slipstream-tray/Cargo.toml:42`,
`crates/ss-capture/Cargo.toml:50`, `crates/ss-clipboard/Cargo.toml:40`,
`crates/ss-encode/Cargo.toml:45`, `crates/ss-frame/Cargo.toml:25`,
`crates/ss-gpu/Cargo.toml:22`, `crates/ss-inject/Cargo.toml:52`,
`crates/ss-vdisplay/Cargo.toml:64`.

The `slipstream-tray` crate already does a **raw Win32 tray** on Windows using
`windows` with features `Win32_Foundation`, `Win32_Graphics_Gdi`,
`Win32_System_LibraryLoader`, `Win32_System_Services`, `Win32_UI_Shell`,
`Win32_UI_WindowsAndMessaging` (`crates/slipstream-tray/Cargo.toml:42-49`). So the
`windows` crate in the tree is a **Win32 API binding**, not a windowing framework.

**Conclusion: no GUI framework is present. Any choice adds a new dependency tree.**
There is no sunk cost to protect and no existing native GUI to extend.

### E7. The launch story today

**There is no `slipstream host`, `slipstream ui`, or `slipstream web` command.** The
`slipstream` CLI (`clients/cli/`) is client-side only. Its complete verb list is at
`clients/cli/src/cmd/mod.rs:942-951`: `pair`, `hosts`, `wake`, `library`, `launch`, `open`,
`reachable`, `speed-test`, `profiles`, `reset`. `clients/cli/src` contains only
`main.rs` and `cmd/mod.rs`, and zero HTTP routes.

The canonical launch is two commands (root `README.md:44-53`):

```
cargo build -p slipstream-host
./target/debug/slipstream-host serve --mgmt-bind 127.0.0.1:47990
cd web && bun install && bun run dev
```

Production is the systemd user service `slipstream-web.service` on port 47992
(`scripts/slipstream-web.service:38`, `ExecStart=/usr/bin/slipstream-web-server`).

**There is no Windows launcher for the web console.** `docs-site/content/docs/windows-host.md:13`
says plainly: "There is no installer in this repository." The systemd unit and the
`/usr/bin/slipstream-web-server` wrapper are Linux-only
(`scripts/slipstream-web.service:38`, `packaging/debian/build-web-deb.sh:74`). On Windows
you must run the Bun and Nitro server yourself.

The `slipstream-tray` "Open web console" menu item is the only existing one-click
affordance and it only opens a browser at the already-running console
(`crates/slipstream-tray/src/platform/windows.rs:272-274`, `:352-355`;
Linux at `crates/slipstream-tray/src/platform/linux.rs:117-121`).

### E8. What already exists for Windows distribution

The repo already builds a Windows host and a Windows tray:
`cargo build --release -p slipstream-host -p slipstream-tray`
(`docs-site/content/docs/windows-host.md:36`), registered as a service with
`sc create slipstream-host binPath= ... start= auto`
(`docs-site/content/docs/windows-host.md:55`). Config on Windows lives in
`%APPDATA%\slipstream`, overridable with `SLIPSTREAM_CONFIG_DIR`
(`docs-site/content/docs/windows-host.md:47-48`).

This matters for C1 and C2: a native Rust shell that depends only on the `windows` crate
already in the tree adds **no new mandatory end-user runtime**. A WebView2 shell adds the
WebView2 runtime, which is present on Windows 11 and Windows 10 version 2004 and later,
but is a separate install on older Windows 10.

## Options assessed

1. **Tauri v2.** A Rust shell around a system webview (WebView2 on Windows, WebKitGTK 4.1 on Linux) that hosts the existing React console as a static asset bundle. It reuses 100 percent of `web/src` and needs no UI rewrite, but it adds the WebView2 runtime on Windows and cannot reach the Steam Deck on the WebKitGTK 4.1 evidence (matrix row 1; section 01). One fact decides its fate: `libwebkit2gtk-4.1-0` is absent from every Steam Linux Runtime branch and from the Flatpak runtimes, so a Tauri v2 app cannot be shipped to the Deck on any of the three distribution routes (section 01, lines 93-111).

2. **wry directly, plus a Bun-compiled executable.** The webview library that Tauri itself is built on, used directly without Tauri's plugin ecosystem or opinionated frontend, optionally shipped as a Bun-compiled single-file executable. It carries the same WebView2 and WebKitGTK 4.1 dependencies as Tauri (matrix row 2; findings one-liner). wry is Apache-2.0 OR MIT (docs/adr/research/section-02-wry-bun.md, line 7), so it imposes no commercial royalty and no GPL obligation. Using wry directly inherits the same WebKitGTK 4.1 / GTK 3 pins as Tauri: wry v0.58.1 pins `webkit2gtk = "3"` (`v2_38`), `webkit2gtk-sys = "3"`, `gtk = "0.19.0"`, and `soup3 = "0.9.0"` (docs/adr/research/section-02-wry-bun.md, line 13), so it does not escape the Deck blocker. It also gives up Tauri's bundler, config, IPC, updater, permissions, single-instance and tray, so it is strictly more work than Tauri for the same result (docs/adr/research/section-02-wry-bun.md, lines 11 and 19). Separately, a Bun-compiled executable embeds the Bun runtime in one file so an end user does not need Bun installed (docs/adr/research/section-02-wry-bun.md, line 31), sidesteps WebKitGTK entirely, but if Bun.serve replaces nitro then the entire BFF security proxy must be reimplemented: token injection, cookie stripping, credential denylist, scoped TLS relaxation, and 401 mapping (docs/adr/research/section-02-wry-bun.md, lines 33 and 39).

3. **Avalonia.** A C# and XAML cross-platform native toolkit on .NET 8.0, MIT licensed, with a self-contained build that needs no .NET install and installs without admin rights. It fails C6 because the entire React UI is rewritten and the native process becomes a second bearer-token holder, and Linux is a Tier 3 platform (matrix row 3; section 03).

4. **Flutter and React Native for desktop.** Flutter is the stronger of the two: Linux is a Supported and CI-tested target and it renders with its own engine, sidestepping the WebKitGTK problem on the Deck, but it still rewrites the whole UI and creates a second token holder. React Native Windows is Windows-only, three minor versions behind core, and mid-architecture-migration (matrix row 4; section 04).

5. **Bun compiled binary with a GUI toolkit.** Bun can produce a single-file Windows executable, which is attractive for distribution, but a GUI still requires a separate toolkit and the console-subsystem and Windows API behaviour must be verified before it can be trusted (adr-summary). Matrix row 5 scores most cells `?` because no primary source verified them. This overlaps with Option 2's Bun-compiled aspect; the two are kept separate because the matrix scores them separately, but the practical distinction is small. This option is not pursued independently.

7. **Keep the web app and serve it locally.** The existing React console and Nitro server stay as they are; the change is serving topology plus a small Rust launcher that supervises the console and opens a browser, optionally with a Web App Manifest so it installs as a standalone PWA window. It satisfies C1 through C7 with no rewrite, no new runtime, and no new token holder (matrix row 7; section 05).

8. **Slint.** A declarative Rust-native toolkit with a designer story and no extra runtime. The Royalty-free tier is a genuine closed-source shipping option with no revenue threshold and no royalty, only an attribution obligation, but it requires a full rewrite and the GPL tier must be avoided (matrix row 8; section 08).

9. **egui and eframe.** Immediate-mode Rust GUI with no extra runtime and MIT OR Apache-2.0 licensing, but the UI is rewritten from scratch and the immediate-mode model does not map onto a CSS design system (matrix row 9; section 08).

10. **GPUI.** The Zed toolkit, Rust-native and Apache-2.0 with a real Direct3D Windows backend, but pre-1.0 with breaking changes, and the published crate cannot depend on its own platform layer from crates.io so consumers need a git dependency (matrix row 10; section 08).

11. **iced.** A pure Rust retained-mode GUI with permissive licensing and no extra runtime, but a smaller ecosystem and a full rewrite (matrix row 11; section 08).

12. **Win32 shell with optional WebView2.** The lightest possible Windows-native process with no extra runtime on the pure Win32 path and full control over the window, but Windows only, does not scale to Linux or the Steam Deck, and the WebView2 variant still carries the runtime cost (matrix row 12; section 12).

There is no Option 6. The numbering used by the research sections skips from 5 to 7, and this ADR records that gap explicitly rather than inventing an option. Option 6 was never assigned to a candidate in any section.

## Evaluation against the constraints

Table reproduced from `matrix.md` (constraint grid). Cells: pass, fail, partial, `?`.

| Option | C1 no admin install | C2 no new end-user runtime | C3 cross-platform (Win primary, Linux + Steam Deck exist) | C4 memory and startup footprint | C5 no GPL, no commercial royalty | C6 effort to a polished UI vs the React design system | C7 is a native app even wanted |
|---|---|---|---|---|---|---|---|
| 1 Tauri v2 | partial (NSIS per-user installs to LOCALAPPDATA without admin) | fail (WebView2 runtime) | fail on Steam Deck, partial on Linux | partial | pass | pass (reuses React) | open |
| 2 wry | ? | fail (WebView2 runtime) | fail on Steam Deck for the wry half, partial on Linux; Bun-compiled binary has no GTK dependency but provides no native window | partial | pass | pass (reuses React) | open |
| 3 Avalonia | pass (self-contained installs without admin rights) | pass if self-contained, fail if framework-dependent | pass | partial | pass | fail (rewrite) | open |
| 4 Flutter and React Native for desktop | partial | pass for Flutter (engine ships in the bundle), fail for React Native Windows (needs C++, MSVC v143 and the Windows App SDK at build time) | pass for Flutter (Linux is Supported and CI-tested), fail for RNW (Windows-only) | partial for Flutter, fail for RNW (Fabric plus Windows App SDK) | pass (BSD-3 and MIT) | fail (rewrite) | open |
| 5 Bun compiled binary with a GUI toolkit | ? | ? | ? | ? | pass | partial | open |
| 7 Keep the web app, serve locally | pass | pass | pass | pass | pass | pass (no new UI at all) | answered against |
| 8 Slint | pass | pass | pass | pass | pass on the Royalty-free tier, fail on GPL, with an attribution obligation | fail (rewrite) | open |
| 9 egui and eframe | pass | pass | pass | pass | pass (MIT or Apache-2.0) | fail (rewrite) | open |
| 10 GPUI | pass | pass | pass | pass | pass (Apache-2.0) | fail (rewrite) | open |
| 11 iced | pass | pass | pass | pass | pass (MIT) | fail (rewrite) | open |
| 12 Win32 shell with optional WebView2 | pass | pass (pure Win32 path) or fail (WebView2 path) | fail (Windows only) | pass | pass | partial | open |

Which constraints eliminate which options outright:

- C3 eliminates the webview options on the Steam Deck. Tauri v2 and wry link against `libwebkit2gtk-4.1-0` and `libgtk-3-0` (section 01, lines 89-92). The Steam Linux Runtime ships no WebKitGTK in any of its four branches, soldier, sniper, steamrt4, steamrt5 (section 01, line 97). Arch, which SteamOS derives from, provides `libwebkit2gtk-4.0.so` rather than the 4.1 pkg-config name the Tauri build probes for (section 01, line 99). The Flatpak route also fails: there is no `webkit2gtk` runtime extension in the complete 19,576-ref Flathub inventory, and the GNOME runtime ships WebKitGTK 6.0 on GTK 4 rather than the 4.1 GTK 3 API that wry pins (section 01, lines 105-108). Tauri makes no Steam Deck statement at all: zero occurrences of "Steam Deck" and zero of "SteamOS" across the v2 docs corpus (section 01, line 95). So options 1 and 2 fail C3 outright on the Deck. Option 2 splits here: its wry half fails C3 on the Deck for the same WebKitGTK reason (docs/adr/research/section-02-wry-bun.md, lines 13 and 41), while a Bun-compiled binary has no GTK or WebKitGTK dependency and therefore does not share that specific blocker, although a Bun binary does not by itself provide a native window (docs/adr/research/section-02-wry-bun.md, lines 31 and 41).

- C2 eliminates or weakens the webview options on Windows. WebView2 is a separate redistributable, present on Windows 11 and Windows 10 version 2004 and later but a separate install on older Windows 10 (ADR 0001, lines 357-358). It is mostly present in practice, but it is a runtime dependency the constraint forbids as mandatory. Options 1 and 2 fail C2 on that basis. Option 12's WebView2 variant fails C2 the same way; its pure Win32 variant passes.

- C2 also shapes the native toolkits. Avalonia needs the .NET Desktop Runtime unless self-contained (section 03, lines 15-24). Flutter ships its own engine in the bundle and passes; React Native Windows needs C++, MSVC v143 and the Windows App SDK at build time (section 04).

- C5 and Slint: the GPL-or-commercial split and the royalty-free tier, and why that is a real obligation. Slint 1.19.1 is triple-licensed GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0 (section 08). The Royalty-free edition grants a "world-wide, royalty-free, non-exclusive license to use, reproduce, make available, modify, display, perform, distribute the Software as part of a Desktop, Mobile, or Web Application", closed-source shipping is allowed, and there is no revenue threshold on that tier (the 50-employee and 10M EUR thresholds apply only to the paid Commercial plans) (section 08; matrix note). The sole obligation is attribution via the `AboutSlint` widget or a badge. C5 is satisfiable without a royalty provided the project takes the Royalty-free tier and shows the attribution, and avoids the GPL tier entirely. This is a real obligation, not a formality: shipping Slint without attribution, or shipping under GPL, would either breach the licence or impose copyleft on the combined work. The GPL tier is the one that would violate C5.

- C6 and rewrite cost: every non-React-reusing option discards 22,411 lines across 166 files plus the design-system surface catalogued in E4 (ADR 0001, lines 188-240). That is 15,557 lines in `web/src/sections`, 2,920 in `web/src/components`, plus 28 design-system files and 13 vendored design-system files. Options 3, 4, 8, 9, 10, 11 all fail C6 on that basis. Option 12's pure Win32 variant fails C6 for the same reason; its WebView2 variant passes because it keeps the React app.

- C7 is the operator's question and the matrix leaves it open for every option except Option 7, which is "answered against" because it does not attempt a native app at all. The research in findings.md and adr-summary.md argues that the branding gap is narrower than it first appears: a PWA covers the start menu entry, the icon, and the browser chrome, with no native code and no new runtime, and what a PWA does not cover is native OS integration (tray icon, native file dialogs, native notifications, global hotkeys, OS-level drag and drop). Whether that remaining gap matters is a product judgement, not a technical one.

## Decision

Primary decision: **adopt Option 7, keep the web app and serve it locally, plus a Web App Manifest**. Do not adopt any GUI framework and do not add a native webview shell now.

Concretely, the deliverable is:

1. A Rust launcher subcommand on the existing host binary that supervises the Nitro console as a child process, owns a single-instance lock, and opens a browser at `http://127.0.0.1:47992` (section 05, lines 23-27).
2. A Web App Manifest served from the console origin, with `name`, `short_name`, 192px and 512px icons, `start_url`, and `display: standalone`, plus the `<link rel="manifest">` tag, so Chromium-based browsers offer an install that lands an app icon in the Start menu or Dock and launches without browser chrome (section 05, lines 31-37). The console origin already satisfies Chromium's installability requirement because it is served from `127.0.0.1` (section 05, line 33).
3. No new end-user runtime, no new GUI dependency in `Cargo.lock`, and no second bearer-token holder. The token stays server-side in the Nitro proxy exactly as today (ADR 0001, lines 105-107).

Why this and not the webview shell: the decisive constraints are C3 (the Deck must not be stranded), C2 (no new mandatory end-user runtime), and C7 (whether a native app is wanted at all). On C3, every webview option fails the Deck on the WebKitGTK 4.1 evidence, and there is no vendor support story to lean on (section 01, lines 93-111). On C2, WebView2 is a separate redistributable that collides with the constraint even though it is mostly present (ADR 0001, lines 355-358). On C7, the research shows the branding gap is narrower than it first appears: a PWA covers the start menu entry, the icon, and the browser chrome, with no native code and no new runtime, and what remains uncovered is native OS integration, which is a product judgement rather than a technical one (findings.md, Finding 3; adr-summary.md, Finding 3). Option 7 is the only option that satisfies C1 through C6 with no rewrite, no new runtime, and no new token holder (matrix row 7). It is also the only option whose cost is fully knowable.

Secondary decision, explicitly scoped: **do not adopt a webview shell now, and record the triggers that would reverse this.** The webview shell (Option 1, Tauri v2) is the strongest of the native options for Windows and Linux desktop because it reuses 100 percent of `web/src` and needs no UI rewrite (section 01, summary judgement). It is worth its cost only if the two blockers below are removed and the operator decides native OS integration is required. Until then it is correctly rejected.

Triggers that would reverse this decision:

1. **wry lands its GTK 4 / WebKitGTK 6.0 migration.** Open issues 1767 ("Migrating to gtk4/webkit6") and 1474 ("Upgrade wry to gtk4-rs and webkit6") (section 01, lines 111 and 154). WebKitGTK 6.0 is the API Arch already ships and the GNOME Flatpak runtime already ships, so a wry on 6.0 would align Tauri with both the Flatpak and Arch worlds instead of against them (section 01, line 111). That would unblock the Deck and remove the C3 failure.
2. **The operator decides a branded native window with OS integration is required.** Native tray icon, native file dialogs, native notifications, global hotkeys, OS-level drag and drop, or a distinct process identity in the task switcher. These are the things a PWA does not cover (section 05, line 39; findings.md, Finding 3). If any of them becomes a requirement, the webview shell's cost is justified and this ADR should be superseded.
3. **The React console is rewritten for other reasons.** If the 22,411 lines of application code and 41 design-system files are replaced anyway, the C6 barrier falls and a native rewrite's cost becomes acceptable, making Options 3, 4, 8, 9, 10 or 11 viable. Nothing in this ADR prevents that.
4. **A minimum Windows version is declared and the WebView2 posture is accepted.** This is a one-line decision that unblocks three of the options (findings.md, Finding 2). It is a prerequisite for any webview-based decision and is currently unstated in the repo.

The losing option, the webview shell, becomes correct under the combined conditions of trigger 1 (Deck unblocked) plus trigger 2 (native OS integration required) plus trigger 4 (Windows floor declared). If only trigger 2 fires, the shell is still wrong for the Deck and the Deck keeps the browser path. If only trigger 1 fires, there is no reason to pay for a shell the operator has not asked for.

## Consequences

What becomes easier:

- No new end-user runtime. The install stays a Rust binary plus a browser the user already has (C2 satisfied).
- No UI rewrite. All 166 source files, 16 feature sections, Storybook, Tailwind 4, the orval-generated typed client, and Biome survive untouched (E4).
- No new token holder. The bearer token stays server-side in the Nitro proxy and the security posture is unchanged (E2, E3).
- The Steam Deck is reached, because the console is just a browser. It runs as a systemd service in desktop mode and can be added to the Steam library as a non-Steam shortcut in gaming mode (section 05, lines 74-82).
- The branding gap closes most of the way with no native code, via the PWA install (section 05, lines 31-39).

What becomes harder:

- No native OS integration beyond what the browser provides. No native tray icon, no native file dialogs, no native notifications, no global hotkeys, no OS-level drag and drop, and no distinct process identity in the task switcher (section 05, line 39 and table at lines 43-55).
- The user sees a browser window in `standalone` mode, with browser permission prompts remaining and the ability to navigate away or close the window (section 05, line 39).
- Firefox does not support manifest-based PWA install, and Safari supports Add to Dock only on macOS Sonoma and later (section 05, line 35), so the branded install experience is Chromium-first.

New obligations the team takes on:

- Build the Rust launcher subcommand with single-instance locking and nitro supervision (section 05, lines 21-27).
- Ship the Web App Manifest and 192px and 512px icons (section 05, lines 31-37).
- Declare a minimum Windows version. This is currently unstated and is a prerequisite for any future webview decision (findings.md, Finding 2).
- Packaging and distribution for three platforms: Windows (portable zip on GitHub Releases with the `self_update` auto-updater, or a winget manifest once a real installer exists), Linux (systemd user service, `.desktop` entry per the freedesktop spec), and Steam Deck (systemd service in desktop mode, PWA as a non-Steam shortcut in gaming mode) (section 05, lines 66-83; section 12, lines 75-82).

What is explicitly NOT being done:

- No Tauri, wry, Avalonia, Flutter, React Native, Slint, egui, GPUI, iced, or Win32 GUI framework is adopted.
- No React console rewrite.
- No native webview shell.
- No second bearer-token holder.
- No admin-requiring installer.
- No GPL-licensed dependency.

Packaging and distribution consequences:

- Windows: the launcher is a small Rust binary. Distribution is a portable zip on GitHub Releases with `self_update` (MIT, 1.3.3) for auto-updates, or a winget manifest once a real installer exists (section 05, lines 68-73). No admin rights needed.
- Linux: the existing `slipstream-web.service` systemd unit on port 47992, plus a `.desktop` entry per the freedesktop spec (section 05, lines 9-15 and 74-83).
- Steam Deck: the Linux story applies. The console runs as a systemd service and the PWA can be added to the Steam library as a non-Steam shortcut for a Deck-native grid entry (section 05, lines 80-82). The Steamworks non-Steam shortcuts documentation is the canonical reference, though its field-level details are UNKNOWN because the page is JavaScript-rendered (section 05, line 80).

## Alternatives considered and rejected

- **Tauri v2 (Option 1).** Rejected as the primary now, but retained as the strongest native candidate. It reuses 100 percent of `web/src` and needs no UI rewrite, and it gives a real native window (section 01, summary judgement). It is rejected today because it fails C3 on the Steam Deck on the WebKitGTK 4.1 evidence and fails C2 on the WebView2 runtime, and because C7 does not yet demand native OS integration. It becomes correct under triggers 1, 2 and 4 above.

- **wry directly, plus a Bun-compiled executable (Option 2).** Rejected for the same C3 and C2 reasons as Tauri, since it carries the same WebView2 and WebKitGTK 4.1 dependencies (findings one-liner; matrix row 2; docs/adr/research/section-02-wry-bun.md, lines 13 and 15). 2a is more work than Tauri for the same dependency and Deck outcome, because it gives up Tauri's bundler, config, IPC, updater, permissions, single-instance and tray (docs/adr/research/section-02-wry-bun.md, lines 11 and 19). 2b is only worth a bounded proof of concept if a branded native app is required and the security proxy rewrite is accepted (docs/adr/research/section-02-wry-bun.md, lines 33 and 47).

- **Avalonia (Option 3).** Rejected on C6 and on the token consequence. It requires rewriting the whole React UI in XAML and C#, and the native process becomes a second bearer-token holder (section 03, lines 41-48 and 74-76). Linux is a Tier 3 platform (section 03, line 69). It passes C5 (MIT) and C2 (self-contained), but that does not offset the rewrite.

- **Flutter and React Native for desktop (Option 4).** Rejected on C6. Both throw away the React console and neither can use the Nitro proxy the way a browser does, so both create a second token holder (section 04, lines 70-76). Flutter is the stronger of the two and would be the one to spike if a native rewrite were ever justified, because it has a Supported and CI-tested Linux target (section 04, lines 9-11). React Native Windows is weaker on every axis: Windows-only, three minor versions behind core, mid-migration, and carrying the heaviest build toolchain (section 04, lines 44-51 and 78-80).

- **Bun compiled binary with a GUI toolkit (Option 5).** Rejected as an independent option. Bun can produce a single-file Windows executable, which is attractive for distribution, but a GUI still needs a separate toolkit and the console-subsystem and Windows API behaviour must be verified first (adr-summary). Most of its matrix cells are `?` because no primary source verified them. It overlaps with Option 2 and is not pursued on its own.

- **Slint (Option 8).** Rejected on C6 and on the licence obligation. It requires a full rewrite of the 22,411 lines of application code and 41 design-system files, and it introduces a new markup language, build step, and editor toolchain (section 08, lines 56-58). On C5 it is satisfiable only by taking the Royalty-free tier and showing the `AboutSlint` attribution, with no revenue threshold but a real attribution obligation, while avoiding the GPL tier entirely (section 08, lines 9-14; matrix note). The whole point of this ADR is that Slint was not assumed, and the research confirms it is a viable closed-source option that still costs a full rewrite.

- **egui and eframe (Option 9).** Rejected on C6. Immediate-mode Rust GUI with no extra runtime and MIT OR Apache-2.0 licensing, but the UI is rewritten from scratch and the immediate-mode model does not map onto a CSS design system (section 08, lines 23-25 and 79-81). It has the best accessibility story of the Rust-native options (AccessKit on by default in eframe on Windows) (section 08, lines 27 and 83), but that does not offset the rewrite.

- **GPUI (Option 10).** Rejected on C6 and on stability. Rust-native and Apache-2.0 with a real Direct3D Windows backend, but pre-1.0 with breaking changes, and the published crate cannot depend on its own platform layer from crates.io so consumers need a git dependency (section 08, lines 31-40). It has the closest styling analogue to the existing Tailwind tokens, but that does not offset the stability risk.

- **iced (Option 11).** Rejected on C6 and on accessibility. A pure Rust retained-mode GUI with permissive licensing and no extra runtime, but a smaller ecosystem, a full rewrite, and no documented accessibility support (section 08, lines 47-52 and 83).

- **Win32 shell, pure variant (Option 12b).** Rejected outright. It requires reimplementing 22,411 lines of React plus 41 design-system files in raw Win32 and GDI, with no CSS, no Tailwind, no Storybook, no DOM, and no browser text rendering, and would ship a visually and accessibly inferior console (section 12, lines 48-51 and 100). It is not a GUI option, it is a client rewrite.

- **Win32 shell, WebView2 variant (Option 12a).** Rejected as redundant with Option 7 plus a launcher. It adds zero new mandatory runtime beyond WebView2 and zero new licensing obligations, and preserves the security model by construction (section 12, lines 102-104). But it is Windows-only, does not improve the product because it renders the same React console, and does not make the installer problem smaller (section 12, lines 106-110). Option 7 plus a Rust launcher achieves the same Windows affordance without the Windows-only constraint and without the WebView2 runtime.

## References

| Source | What it establishes |
|---|---|
| `docs/adr/0001-desktop-dashboard-framework.md` E1 (lines 57-96) | Rust host owns 94 HTTP route handlers; management API on port 47990; SSE on `GET /api/v1/events` |
| `docs/adr/0001-desktop-dashboard-framework.md` E2 (lines 97-149) | Web console is a backend for frontend; browser calls relative `/api/...`; Nitro proxy injects bearer token; token never reaches browser |
| `docs/adr/0001-desktop-dashboard-framework.md` E3 (lines 150-187) | Auth is password-for-human, bearer-for-machine; session cookie; loopback-only first-run setup |
| `docs/adr/0001-desktop-dashboard-framework.md` E4 (lines 188-245) | Measured React surface: 22,411 lines across 166 files, 789,847 bytes; design-system file counts |
| `docs/adr/0001-desktop-dashboard-framework.md` E5 (lines 246-270) | `ss-console-ui` is a Skia in-stream overlay, not a desktop GUI |
| `docs/adr/0001-desktop-dashboard-framework.md` E6 (lines 271-313) | `Cargo.lock` contains no GUI framework today; `windows` crate is a Win32 binding |
| `docs/adr/0001-desktop-dashboard-framework.md` E7 (lines 315-344) | No `slipstream host/ui/web` command; two-command launch; no Windows console launcher |
| `docs/adr/0001-desktop-dashboard-framework.md` E8 (lines 345-358) | Windows host and tray already build; WebView2 availability note |
| `docs/adr/research/matrix.md` | Constraint grid reproduced in the evaluation table; Steam Deck and Slint notes |
| `docs/adr/research/findings.md` | Three findings: token constraint, webview tax, cheapest option changes serving topology |
| `docs/adr/research/adr-summary.md` | ADR summary, gaps, line-numbered evidence |
| `docs/adr/research/section-01-tauri.md` | Option 1 full evidence; wry facts used for Option 2 (lines 9, 91, 111, 154) |
| `docs/adr/research/section-03-avalonia.md` | Option 3 full evidence |
| `docs/adr/research/section-04-flutter-rn.md` | Option 4 full evidence |
| `docs/adr/research/section-05-keep-web.md` | Option 7 full evidence |
| `docs/adr/research/section-08-rust-native.md` | Options 8-11 full evidence |
| `docs/adr/research/section-12-native-win32.md` | Option 12 full evidence |
| https://github.com/tauri-apps/wry/issues/1767 | wry "Migrating to gtk4/webkit6", open |
| https://github.com/tauri-apps/wry/issues/1474 | wry "Upgrade wry to gtk4-rs and webkit6", open |
| https://github.com/tauri-apps/wry/blob/wry-v0.58.1/Cargo.toml | wry pins `webkit2gtk = "3"`, `gtk = "0.19"`, `soup3` |
| https://developer.mozilla.org/en-US/docs/Web/Progressive_web_apps/Guides/Making_PWAs_installable | Chromium installability criteria; 192px and 512px icons; `standalone` display |
| https://developer.mozilla.org/en-US/docs/Web/Manifest | Web App Manifest format |
| https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html | `Type=exec`, `Type=notify`, `Restart=on-failure` |
| https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shellexecutew | `ShellExecuteW` for opening a URL |
| https://docs.rs/self_update/latest/self_update/ | `self_update` crate, 1.3.3, MIT, auto-updater |
| https://partner.steamgames.com/doc/store/nonsteam_shortcuts | Steamworks non-Steam shortcuts, canonical reference |
| https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/evergreen-vs-fixed-version | WebView2 Evergreen Runtime availability |
| https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution | WebView2 distribution and offline installers |

Note on the numbering gap: Option 6 was never assigned. Options 1, 2, 3, 4, 5, 7, 8, 9, 10, 11 and 12 are all defined above. There is no Option 6 section in any research document, and this ADR does not invent one.
