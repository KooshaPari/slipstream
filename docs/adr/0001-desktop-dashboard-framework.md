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
