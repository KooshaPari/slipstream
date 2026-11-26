## Option 7: Keep the web app and serve it locally

### 1. What it is

Nothing about the existing architecture changes. The Rust host already serves 85 axum routes under `/api/v1` plus 7 nvhttp GameStream-compat routes on `https://127.0.0.1:47990`, with bearer-token auth and SSE on `GET /api/v1/events`. The console is a backend for frontend: the browser makes same-origin relative `/api/...` calls, and a nitro/h3 server under `web/server` proxies server-side to the Rust host and injects the bearer token, so the token never reaches the browser. There is no CORS and no direct browser-to-Rust path. VERIFIED against repo facts provided by the coordinator.

The console already runs on port 47992 as `slipstream-web.service` on Linux. On Windows there is no launcher today. So the work is two things: a launcher or Windows service that starts the nitro console and supervises it, and opening a browser at `http://127.0.0.1:47992`.

For the systemd unit, `Type=exec` reports failure when the service binary "cannot be invoked successfully", `Type=notify` "allows service program code to precisely schedule when to consider the service started up successfully", and `Restart=on-failure` is "the recommended choice for long-running services" (all VERIFIED, https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html). INFERENCE: migrate the unit to `Type=notify` if nitro can emit a readiness signal, otherwise `Type=exec` with `Restart=on-failure`.

### 2. The tiny launcher

**Windows.** `ShellExecuteW` "Performs an operation on a specified file" and opens a URL as a URL. It "returns a value greater than 32" on success, with distinct failure codes such as `ERROR_FILE_NOT_FOUND` and `SE_ERR_NOASSOC`, and COM "should be initialized before ShellExecute is called" via `CoInitializeEx(NULL, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE)` (all VERIFIED, https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shellexecutew). Rust's `std::process::Command` cannot open a URL, so a shell API call or a crate is needed.

**Linux.** `xdg-open` is "Open a URL in the user's preferred application that handles the respective URL or file type" (VERIFIED, https://www.freedesktop.org/wiki/Software/xdg-utils/). A desktop entry requires "a group header named `Desktop Entry`", with `Key=Value` pairs where "Only the characters `A-Za-z0-9-` may be used in key names" (VERIFIED, https://specifications.freedesktop.org/desktop-entry-spec/latest/basic-format.html). The `Exec` key "must contain a command line", and "if no full path is provided the executable is looked up in the `$PATH` environment variable used by the desktop environment" (VERIFIED, https://specifications.freedesktop.org/desktop-entry-spec/latest/exec-variables.html).

**macOS.** The `open` command "opens a file (or a directory or URL), just as if you had double-clicked the file's icon" using "the default application as determined via LaunchServices". The `-u URL` flag "Opens URL with whatever application claims the url scheme, even if URL also matches a file path", and `-F` opens "fresh", meaning "without restoring windows" (all VERIFIED, https://keith.github.io/xcode-man-pages/open.1.html). The Apple archive URL returned HTTP 401, so the mirrored man page is the citation. The AppKit equivalent is `NSWorkspace`, whose `open(URL)` "Opens the location at the specified URL" (VERIFIED, https://developer.apple.com/documentation/appkit/nsworkspace.md).

**Crates.** The `open` crate (MIT, 5.4.5) opens a path or URL "obeying the current user's desktop configuration", tries `xdg-open` first on unix, and warns "on some platforms and circumstances, the launcher may block", in which case you should spawn it yourself via `commands()` or `with_command()`. Its `insecure` feature "must not be enabled when paths or URLs may be attacker-controlled" (all VERIFIED, https://docs.rs/open/latest/open/). The `webbrowser` crate (MIT OR Apache-2.0, 1.2.7) "guarantees that the browser is opened, even for local files", warns that alternative libraries "may lead to an editor being opened (instead of the browser) for local html files", and offers a `hardened` feature that "disables handling of non-http(s) urls (e.g. `file:///`) as a hard security precaution" (VERIFIED, https://docs.rs/webbrowser/latest/webbrowser/).

**Single instance and relaunch.** Rust std has no cross-platform single-instance primitive, so it must be built: a named mutex or lock file on Windows, a lock file plus `flock` on unix. INFERENCE: the launcher owns the lock, and a second invocation detects the existing instance and reopens the browser URL rather than spawning a duplicate nitro process. The `self_update` crate's `restart` module "re-runs with the current arguments" and "on unix the process image is replaced with `exec` (the PID is preserved); on windows the new binary is spawned and the current process exits" (VERIFIED, https://docs.rs/self_update/latest/self_update/).

### 3. The Rust-side integration

Both crates are plain Rust with no runtime dependencies beyond the platform and permissively licensed (VERIFIED, same URLs as above). Adding either is one Cargo dependency plus a call site. INFERENCE: the launcher should be a subcommand or flag on the existing Rust host binary rather than a separate crate, because the host already owns the management API lifecycle and knows the console port. The host spawns nitro as a child process and supervises it, and on a later launch detects the existing instance via the lock file, skips spawning, and opens the browser. The `open` crate's blocking warning is why the open call belongs off the main thread.

### 4. The honest cost

The user sees their real browser, not a branded native window. Chrome or Edge tabs, extensions, bookmarks, browser chrome, the omnibox showing `127.0.0.1:47992`. This is the largest real cost.

**A PWA fixes most of that with no native code.** This is the strongest form of the option. The Web App Manifest is "a JSON text file that provides information about a web application", used to give the browser what it needs "to install a progressive web app (PWA) on a device, such as the app's name and icon", linked via `<link rel="manifest" href="manifest.json" />` (VERIFIED, https://developer.mozilla.org/en-US/docs/Web/Manifest).

Chromium-based browsers, "including Google Chrome, Samsung Internet, and Microsoft Edge", require `name` or `short_name`, `icons` with "a 192px and a 512px icon", `start_url`, `display` and/or `display_override`, and `prefer_related_applications` "must be `false` or not present". Installability also requires the app to be "served using the `https` protocol, or from a local development environment using `localhost` or `127.0.0.1` - with or without a port number" (all VERIFIED, https://developer.mozilla.org/en-US/docs/Web/Progressive_web_apps/Guides/Making_PWAs_installable). VERIFIED: `http://127.0.0.1:47992` already satisfies that origin requirement as written.

Once installed, "the PWA gets an app icon on the device, alongside platform-specific apps", and it "can be launched as a standalone app, rather than a website in a browser", where `"standalone"` "indicates that the PWA should look and feel like a platform-specific application, with no browser UI elements". "Chromium browsers support installing PWAs that have a manifest file on all supported desktop operating systems", "Safari supports Add to Dock on macOS Sonoma (Safari 17) and later for any web app with or without a manifest file", and "Firefox does not support installing PWAs using a manifest file". Fallback: Chrome, Safari, and Edge on desktop "support users installing any website as an app, whether or not it has a manifest file, and without regard to the installability criteria" (all VERIFIED, same URL).

The `beforeinstallprompt` event "is fired on the global `Window` object as soon as the browser has determined the PWA is installable", so the page can call `preventDefault()` then `prompt()` from its own Install button. Not supported on iOS (VERIFIED, same URL). INFERENCE: the console can ship an Install button that appears only when that event fires, giving a branded install flow with zero native code.

**Still missing even with a PWA.** The address bar is gone in `standalone` mode but browser permission prompts remain, and the user can navigate away or close the window. There is no native OS API access: no native tray icon, no native file dialogs, no native notifications, no global hotkeys, no OS-level file drag and drop. The `web.dev` PWA course documents a "Window management" chapter for "a PWA outside of the browser manages its own window" and an "OS Integration" chapter, both browser-managed APIs rather than native ones (VERIFIED, https://web.dev/learn/pwa/).

### 5. What is lost versus a native window

| Capability | Plain tab | Installed PWA | Native window |
| --- | --- | --- | --- |
| Start menu / Dock entry with icon | No | Yes | Yes |
| Own window without browser chrome | No | Yes (`standalone`) | Yes |
| Native window chrome and menus | No | No | Yes |
| Native tray icon | No | No | Yes |
| Native file dialogs | No | No | Yes |
| Native OS notifications | Web API only | Web API only | Yes |
| Drag files from OS file manager | Limited | Limited | Yes |
| Global hotkeys | No | No | Yes |
| Own process in task manager | No | No | Yes |
| Offline capable | Yes | Yes | Yes |

**Verdict on "a branded native app".** A PWA installed from Chrome or Edge IS installable, DOES appear in the Start menu with an icon, and DOES run in its own window with no browser UI (VERIFIED, the MDN installability page above). It is not a native binary. INFERENCE and judgement: for a LAN-local management console, an installed PWA is a credible answer, because the user-visible outcome is an app icon, an app window, and no browser chrome, which covers most of what "branded" means. It is weaker than a native binary on three axes: it is not its own process in the OS task switcher, it cannot own a tray icon, and it cannot own a native file dialog. Whether those three matter is a product judgement, not a technical one. A plain tab with no manifest installed does not meet the "branded native app" bar at all.

### 6. The security angle

This option preserves the existing security posture exactly, with no new trust boundary. The bearer token stays server-side inside the nitro proxy and never reaches the browser. There is no webview process and no separate renderer with its own network stack. The console on port 47992 is what the human authenticates against, and the Rust management API on 47990 is never exposed to the browser. First-run setup is loopback-only, and the console requires a shared console password. VERIFIED against repo facts provided by the coordinator.

Contrast with the Tauri and wry options, where the webview is a separate process whose network access must be constrained and whose renderer can be pointed at remote content, which is exactly the risk the Tauri capability model exists to limit. Here the renderer is the user's own already-trusted browser on loopback.

Two launcher-specific notes. The `open` crate's `insecure` feature "must not be enabled when paths or URLs may be attacker-controlled", so the launcher should open only a fixed `http://127.0.0.1:<port>/` URL and never one assembled from user input. The `webbrowser` crate's `hardened` feature "disables handling of non-http(s) urls (e.g. `file:///`)`, a good default for a console launcher (both VERIFIED, same URLs as above).

### 7. Size and distribution

The launcher would be a small Rust binary. UNKNOWN: no primary source states a launcher binary size, so none is asserted. VERIFIED footprint facts: `open` is MIT with a single normal dependency (`is-wsl`) plus optional `libc` and `dunce`, and `webbrowser` is MIT OR Apache-2.0 with `log`, `url`, `objc2`, `objc2-foundation`, `objc2-app-kit`, `web-sys`, `jni`, and `ndk-context` as normal dependencies (VERIFIED, same URLs as above). INFERENCE: the launcher is a small additional Rust artifact, a rounding error next to the host binary and the bundled Node or Bun runtime. UNKNOWN: total installer size, since no primary source provides it.

**Route A: winget.** The WinGet community repository requires multi-file manifests, where "the minimum required files are: a version file, a defaultLocale file, an installer file". The layout is `manifests\<first lower case letter of publisher>\<publisher>\<package>\<version>\`, and "Your PR must be limited to a single package version" (VERIFIED, https://cdn.jsdelivr.net/gh/microsoft/winget-pkgs@master/doc/Authoring.md and https://cdn.jsdelivr.net/gh/microsoft/winget-pkgs@master/doc/README.md). Manifests are validated with `winget validate --manifest <path>`, which "validates a manifest for submitting software to the Microsoft Community Package Manifest Repository on GitHub" (VERIFIED, https://learn.microsoft.com/en-us/windows/package-manager/winget/validate), and tested with `winget install --manifest <path>` after enabling `winget settings --enable LocalManifestFiles`. Manifest architecture "must reflect the installed binaries, not just the installer itself" (VERIFIED, the authoring guide). VERIFIED: there is no first-party Slipstream winget package today.

**Route B: portable zip on GitHub Releases with an auto-updater.** The `self_update` crate (MIT, 1.3.3) "provides updaters for updating rust executables in-place from various release distribution backends", supporting GitHub, GitLab, Gitea, Gitee, S3-compatible endpoints, and a static manifest backend (VERIFIED, https://docs.rs/self_update/latest/self_update/). The `checksums` feature "verifies the downloaded artifact against a digest before installing", but the crate is explicit that this "is an *integrity* check only ... so it is not a substitute for the `signatures` feature" using zipsign. `verify_archive` runs "after the crate's own content gates (checksum, release digest, signature) and before anything is extracted", and `verify_binary` runs "immediately before it replaces the installed one". Permission-wise, "the crate never escalates privileges. There is no sudo re-exec, no polkit interaction, and no UAC prompt". Defaults block on a `yes/no` prompt, so a non-interactive caller must set `.no_confirm(true)` and usually `.show_output(false)`. GitHub API limits are 60 requests per hour unauthenticated and 5000 authenticated, and "an update check costs one API request" (all VERIFIED, same URL). INFERENCE: route B is materially cheaper for a small Rust launcher and avoids Microsoft-side review. The `manifest` backend, serving releases from a `manifest.json` on any static file server, avoids GitHub API limits.

### 8. Cross-platform including Linux and Steam Deck

The web app is already cross-platform because it is a web app. There is no per-platform UI work.

**Linux.** The service story is proven by `slipstream-web.service`, with `Type=exec` or `Type=notify` and `Restart=on-failure` (VERIFIED, the systemd.service URL above). The desktop entry follows the freedesktop spec (VERIFIED, the desktop-entry-spec URLs above).

**Steam Deck and SteamOS.** SteamOS is Linux, so the Linux story applies. The canonical citation for adding the app to the Steam library is the Steamworks non-Steam shortcuts documentation at https://partner.steamgames.com/doc/store/nonsteam_shortcuts. VERIFIED: the page exists and is the canonical Steamworks reference. UNKNOWN: its body content is JavaScript-rendered and could not be read, so field-level requirements for the shortcut target and startup directory are not asserted here.

INFERENCE: three arrangements compose. (a) The console runs as a systemd service and the user opens the browser from gaming mode or desktop mode. (b) The PWA is installed in the browser and added to the Steam library as a non-Steam shortcut, giving a Deck-native grid entry. (c) A `.desktop` file per the freedesktop spec provides the desktop-mode entry point. INFERENCE: (b) combined with (c) is the most Deck-native outcome available without native code.

### 9. How much of the existing work survives

Everything. All 166 source files, all 16 sections, Storybook, Tailwind 4, the orval-generated typed API client, biome, the Vite build. The BFF token-injection architecture is untouched, so the security posture is untouched. VERIFIED against repo facts provided by the coordinator.

New work is bounded to four items: (1) a launcher or Windows service or tray that supervises the nitro console and owns the single-instance lock, (2) the `open` or `webbrowser` dependency and its call site, (3) the web manifest plus 192px and 512px icons and the `<link rel="manifest">` tag, and (4) the packaging and update channel. INFERENCE and judgement: this is the lowest-effort option by a wide margin and the only one requiring zero changes to the 16 feature sections. Its weakness is not effort, it is the branding ceiling in section 5, plus the absence of a native process, a native tray, and native file dialogs.

## Summary judgement for Option 7

This is the cheapest path and also the safest, because it adds no new trust boundary: the token stays server-side in the nitro proxy and the renderer is the user's own trusted browser on loopback. The work is a launcher, a web manifest, and a distribution channel, and none of the existing web app changes.

Its limit is the branding ceiling. A plain browser tab is not a branded native app. An installed PWA gets close: an app icon, a Start menu or Dock entry, and a standalone window with no browser chrome, installable from Chrome or Edge with no native code, on a URL that already satisfies the loopback origin requirement. It is still not a native binary and cannot own a tray icon, a native file dialog, or its own process.

INFERENCE and judgement: if "a branded native app" means a native binary with its own taskbar identity and native OS integration, this option does not fully satisfy it and the PWA is the closest available approximation. If it means a polished, branded, non-embarrassing experience for a LAN management console, the PWA path satisfies it, and this option wins on cost, security, and reuse. The path that closes the remaining gap is pairing this option with a small native Rust shell that owns a window and a tray icon and hosts the same console, which is a hybrid of Option 7 and the webview options rather than a pure Option 7.

## Source list

| Topic | URL | Status |
| --- | --- | --- |
| `open` crate, 5.4.5, MIT | https://docs.rs/open/latest/open/ | VERIFIED |
| `webbrowser` crate, 1.2.7, MIT OR Apache-2.0 | https://docs.rs/webbrowser/latest/webbrowser/ | VERIFIED |
| `self_update` crate, 1.3.3, MIT | https://docs.rs/self_update/latest/self_update/ | VERIFIED |
| `ShellExecuteW` | https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shellexecutew | VERIFIED |
| macOS `open(1)` man page | https://keith.github.io/xcode-man-pages/open.1.html | VERIFIED, Apple archive URL returned 401 |
| `NSWorkspace` (AppKit) | https://developer.apple.com/documentation/appkit/nsworkspace.md | VERIFIED |
| xdg-utils, `xdg-open` | https://www.freedesktop.org/wiki/Software/xdg-utils/ | VERIFIED |
| Desktop Entry Specification, basic format | https://specifications.freedesktop.org/desktop-entry-spec/latest/basic-format.html | VERIFIED |
| Desktop Entry Specification, `Exec` key | https://specifications.freedesktop.org/desktop-entry-spec/latest/exec-variables.html | VERIFIED |
| `systemd.service` | https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html | VERIFIED |
| Web application manifest (MDN) | https://developer.mozilla.org/en-US/docs/Web/Manifest | VERIFIED |
| Making PWAs installable (MDN), Chromium criteria | https://developer.mozilla.org/en-US/docs/Web/Progressive_web_apps/Guides/Making_PWAs_installable | VERIFIED |
| Learn PWA (web.dev) | https://web.dev/learn/pwa/ | VERIFIED |
| `winget validate` | https://learn.microsoft.com/en-us/windows/package-manager/winget/validate | VERIFIED |
| winget-pkgs authoring guide | https://cdn.jsdelivr.net/gh/microsoft/winget-pkgs@master/doc/Authoring.md | VERIFIED |
| winget-pkgs docs overview | https://cdn.jsdelivr.net/gh/microsoft/winget-pkgs@master/doc/README.md | VERIFIED |
| Steamworks non-Steam shortcuts | https://partner.steamgames.com/doc/store/nonsteam_shortcuts | Page exists, body is JS-rendered, field details UNKNOWN |
| Chrome install criteria | https://developer.chrome.com/docs/web-platform/install-criteria/ | HTTP 404, superseded by the MDN Chromium criteria page above |
| Launcher binary size | none | UNKNOWN |
