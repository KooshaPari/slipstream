## Option 4: Flutter and React Native for desktop

This section covers two candidates in one pass because both are "a full rewrite into a different UI framework" and the comparison between them is the useful part. The framing constraint is the one from ADR 0001: both require throwing away the existing React console, and both therefore require a new bearer-token holder, so neither can win on the cheap axes.

### 4.1 FLUTTER

**License.** BSD-3 (VERIFIED, ADR 0001 line 51 states Flutter is BSD-3, which is consistent with the Flutter SDK repository carrying a BSD-3-Clause license at https://github.com/flutter/flutter/blob/master/LICENSE). C5 in ADR 0001 line 51 is satisfied.

**Version and platform support.** VERIFIED against the official supported-platforms page, which states "As of Flutter 3.50, Flutter supports deploying apps on the following combinations of hardware architectures and operating system versions" (https://docs.flutter.dev/reference/supported-platforms, page last updated 2026-11-11). The same page lists Windows x64 and Arm64 as "Supported 10, 11" and "CI tested 10", Debian 10 to 13 as Supported with 12 CI tested, Ubuntu 20.04 LTS to 24.04 LTS as Supported with 22.04 LTS CI tested, macOS Monterey (12) to Golden Gate (27) as Supported, plus Android, iOS and four web browsers.

**This is the load-bearing fact for the Deck.** Flutter has a first-party Linux desktop target that is Supported and CI-tested, and Flutter renders with its own engine rather than a system webview, so it does not need WebKitGTK at all. That sidesteps the WebKitGTK 4.1 problem that sinks the webview options on the Deck (see Option 1). Whether a Flutter Linux build is acceptable inside a Steam Runtime container is a separate question that no primary source here answers, and it is UNKNOWN. INFERENCE: Flutter's native Linux target removes the library-availability blocker, but the Steam Runtime packaging question is still open.

**Runtime footprint on the end-user machine.** Flutter ships its own engine in the app bundle, so there is no Flutter SDK or Dart runtime install for the end user (VERIFIED against the build output layout below). This is a better position on C2 than Avalonia or the .NET path, because the engine travels with the application.

**Windows output layout.** VERIFIED against the official building guide (https://docs.flutter.dev/platform-integration/windows/building, page last updated 2026-11-03): "The Flutter executable, `.exe`, can be found in your project under `build\windows\runner\<build mode>\`", and in addition to that executable "you need the following: From the same directory: all the `.dll` files, the `data` directory" and "The Visual C++ redistributables". The documented structure is:

```
Release
│   flutter_windows.dll
│   msvcp140.dll
│   my_app.exe
│   vcruntime140.dll
│   vcruntime140_1.dll
│
└───data
│   │   app.so
│   │   icudtl.dat
```

The same page states "At this point if desired it would be relatively simple to add this folder to a Windows installer such as Inno Setup, WiX, etc."

**Distribution routes.** VERIFIED, same URL, three documented approaches: "Use tooling to construct an MSIX installer for your application and distribute it through the Microsoft Windows App Store. You don't need to manually create a signing certificate for this option as it is handled for you"; "Construct an MSIX installer and distribute it through your own website. For this option, you need to give your application a digital signature in the form of a `.pfx` certificate"; and "Collect all of the necessary pieces and build your own zip file". The `msix` pub package is the official tool (VERIFIED, same URL), with a reference implementation in the official Flutter samples repository (VERIFIED, https://github.com/flutter/samples/tree/main/desktop_photo_search).

**The winget silent-install trap.** The winget community repository requires silent install support. The authoring guide states "All tools must support a silent install. If you have an executable that does not support a silent install, then we cannot provide that tool at this time" (VERIFIED, https://github.com/microsoft/winget-pkgs/blob/master/doc/Authoring.md). Since `flutter build windows` produces a folder rather than an installer, the Flutter path to winget requires wrapping the output in Inno Setup, WiX or NSIS first. This is a real, documented, non-automated step (VERIFIED against the Flutter building guide's own closing sentence, quoted above). INFERENCE: that wrapper is ordinary work, and it is the same wrapper any other option needs, so it is not a Flutter-specific penalty.

**Windows native look.** VERIFIED against the same guide: Flutter generates "a small C++ application that hosts Flutter", described as a "runner app" responsible for "creating and sizing a traditional Win32 window, initializing the Flutter engine and any native plugins, and running the Windows message loop". The `fluent_ui` package, a Flutter Favorite, "provides support for visuals and common controls that are commonly found in modern Windows apps, including navigation views, content dialogs, flyouts, date pickers, and tree view widgets", and `bitsdojo_window` "provides support for 'owner draw' title bars, allowing you to replace the standard Windows title bar with a custom one that matches the rest of your app" (all VERIFIED, same URL). `win32` "provides a library for accessing thousands of common Windows APIs, using metadata provided by Microsoft", with `win32_registry`, `win32_gamepad`, `biometric_storage`, `windows_taskbar` and `serial_port_win32` building on it (VERIFIED, same URL).

**Size.** UNKNOWN as a specific number for a Slipstream-sized app. Flutter's own FAQ states a minimal release app includes the engine with "a small baseline footprint (a few megabytes compressed), which varies by target platform and architecture", and that "App size is largely determined by bundled assets, fonts, and third-party packages" (VERIFIED, https://docs.flutter.dev/resources/faq). The docs provide `flutter build windows --analyze-size` for an exact measurement (VERIFIED, https://docs.flutter.dev/perf/app-size). No number is asserted here.

### 4.2 REACT NATIVE FOR DESKTOP (REACT NATIVE WINDOWS)

**License.** MIT (VERIFIED, ADR 0001 line 51 states React Native is MIT).

**Governance and platform status.** React Native Windows is an out-of-tree partner platform maintained by Microsoft, not part of the React Native core repository (VERIFIED, https://reactnative.dev/docs/out-of-tree-platforms lists RNW under the partner platforms).

**The version lag.** VERIFIED directly against the RNW documentation site, which is versioned at `0.85` (https://microsoft.github.io/react-native-windows/docs/releases). The same page states "For every major version update of `react-native`, React Native for Windows releases a matching version." The React Native core documentation site is at `0.88` (VERIFIED, the version selector on https://reactnative.dev/docs/). INFERENCE: RNW is three minor versions behind React Native core while its own stated contract is to match every major version, so RNW users wait for upstream features and fixes.

**The staleness banners.** VERIFIED: the RNW app-publishing guide and the parity-status guide both carry the banner "Architecture Review Needed: This documentation was written to support development against React Native's 'Old' or 'Legacy' Architecture. It may or may not be directly applicable to New Architecture development" (VERIFIED, https://microsoft.github.io/react-native-windows/docs/app-publishing and https://microsoft.github.io/react-native-windows/docs/parity-status). These are the two documents a Windows shipping team needs most.

**New Architecture parity gaps.** VERIFIED against the RNW New Architecture document (https://microsoft.github.io/react-native-windows/docs/new-architecture): "we also acknowledge that the React Native for Windows New Architecture experience is not yet at 100% feature parity with its Old Architecture experience, nor at 100% parity with the upstream platforms." The Windows-only components `Flyout`, `Popup` and `Glyph` are not supported in the New Architecture. C# and .NET support does not exist in the New Architecture. XAML interop is described as not quite production ready. The New Architecture requires C++ and the Windows App SDK, so the toolchain burden is higher than Flutter's.

**Tailwind.** Tailwind does not support React Native. INFERENCE from the fact that Tailwind is a browser-CSS utility and React Native does not use the DOM: neither Flutter nor React Native can consume the existing Tailwind stylesheet as-is, so both lose the design-system tooling that ADR 0001 line 52 cites as the bar for C6.

**What RNW keeps that Flutter does not.** React Native reuses React logic and npm packages, so component-level business logic can survive a port even though the DOM-based markup and the CSS do not. INFERENCE: that is a real but partial saving, because the existing console's presentation layer is built on the design system, not on raw DOM markup.

**Windows Store path.** RNW's Store publishing path requires a Microsoft Partner Center developer account, name reservation, Visual Studio's "Publish > Associate App with the Store", asset generation, `Create App Packages` to produce an `.appxupload`, and passing the Windows App Certification Kit (VERIFIED, https://microsoft.github.io/react-native-windows/docs/app-publishing). The same page carries the architecture-review banner quoted above, so the Store path is documented for an architecture that React Native core has deprecated.

### 4.3 WHAT BOTH COST AGAINST ADR 0001

| Constraint | Flutter | React Native Windows |
|---|---|---|
| C1 no admin install | pass, once an Inno Setup or NSIS wrapper exists to provide a silent install | pass, same condition |
| C2 no new end-user runtime | pass, the engine ships inside the bundle | partial, the New Architecture needs the Windows App SDK at build time, not at run time |
| C3 cross-platform | pass, Linux is a Supported and CI-tested target | fail, RNW is Windows-only |
| C4 memory and startup | partial, own renderer and engine in-process | fail, New Architecture adds Fabric, Windows App SDK and a native dependency tree |
| C5 licensing | pass, BSD-3 | pass, MIT |
| C6 polished UI effort | fail, full rewrite of 22,411 lines of application code | fail, full rewrite of the presentation layer |
| C7 | open | open |

### 4.4 THE TOKEN CONSEQUENCE

VERIFIED against ADR 0001 lines 105 to 107: the Nitro server is a backend for frontend, the browser makes same-origin relative `/api/...` calls, and "the server-side proxy injects the management bearer token -- the token never lives in the browser".

Both Flutter and React Native are native processes with their own HTTP clients, so both must hold the management token themselves. Neither can use the existing Nitro proxy the way a browser can, because neither is a browser making same-origin relative requests to it. That converts a single-token-holder design into a two-token-holder design on both options.

### 4.5 JUDGEMENT

**JUDGEMENT, labelled as such:** between the two, Flutter is the stronger candidate. It has a first-party Supported and CI-tested Linux target, which is the constraint that the webview options fail, and its Windows build is a plain folder that any existing installer tooling can wrap. React Native Windows is Windows-only, three minor versions behind core, mid-migration between architectures that its own documentation flags, and it carries the heaviest build toolchain of any option here.

**JUDGEMENT, labelled as such:** neither is a good fit for Slipstream compared with what the webview options achieve, because both throw away the existing React console and neither can use the Nitro proxy the way a browser does. They are on the table because the question "is a real native app worth it" deserves an honest answer, and if the answer is yes then Flutter is the answer, not React Native Windows.

**JUDGEMENT, labelled as such:** the interesting gap in the evidence is the desktop memory and startup cost. C4 calls out "Memory and startup footprint for an always-running background host with a tray", and the ADR notes the host already runs FFmpeg, Vulkan, PipeWire and a QUIC control plane. No primary source here gives a reproducible measurement for a Flutter or RNW desktop app next to an always-running streaming host. This is UNKNOWN and it should be measured, not estimated.

### 4.6 SOURCE LIST

| Topic | URL | Status |
|---|---|---|
| Flutter supported platforms | https://docs.flutter.dev/reference/supported-platforms | VERIFIED |
| Flutter Windows build and distribution | https://docs.flutter.dev/platform-integration/windows/building | VERIFIED |
| Flutter app size tooling | https://docs.flutter.dev/perf/app-size | VERIFIED |
| Flutter engine footprint FAQ | https://docs.flutter.dev/resources/faq | VERIFIED |
| RNW release strategy and doc version 0.85 | https://microsoft.github.io/react-native-windows/docs/releases | VERIFIED |
| RNW New Architecture parity | https://microsoft.github.io/react-native-windows/docs/new-architecture | VERIFIED |
| RNW Store publishing and staleness banner | https://microsoft.github.io/react-native-windows/docs/app-publishing | VERIFIED |
| RNW components and APIs parity | https://microsoft.github.io/react-native-windows/docs/parity-status | VERIFIED |
| RNW system requirements | https://microsoft.github.io/react-native-windows/docs/rnw-dependencies | VERIFIED |
| React Native out-of-tree platforms | https://reactnative.dev/docs/out-of-tree-platforms | VERIFIED |
| React Native core doc version 0.88 | https://reactnative.dev/docs/ | VERIFIED |
| winget silent install requirement | https://github.com/microsoft/winget-pkgs/blob/master/doc/Authoring.md | VERIFIED |
| Flutter Linux Steam Runtime packaging | n/a | UNKNOWN, no primary source found |
| Desktop memory and startup for a Flutter or RNW app next to the host | n/a | UNKNOWN, not measured |

## Summary judgement for Option 4

Flutter is the better of the two, and both are much worse than the webview options for this particular project.

The case for Flutter within this option is a genuine Linux desktop target that is Supported and CI-tested, which is exactly what the webview options cannot offer on the Deck, plus a plain folder build output that any installer can wrap. The case against is the same as for every native rewrite: the 22,411 lines of React application code and the 28 plus 13 design-system files are all replaced, the CSS and Tailwind tooling are gone, and the Nitro proxy can no longer keep the bearer token out of the client, so the app becomes a second holder of the management token.

React Native Windows is weaker on every axis that matters here. It is Windows-only, it is three minor versions behind React Native core, its two most shipping-relevant documents carry an explicit "Architecture Review Needed" banner because they describe an architecture that React Native core deprecated at `react-native@0.82.0`, its New Architecture is explicitly not at parity and has no C# support, and its New Architecture toolchain needs C++, MSVC v143 and the Windows App SDK.

INFERENCE and judgement: if the Steam Deck is a firm requirement and a webview is unacceptable, Flutter is the option to spike, and the spike should measure cold-start time and resident memory next to the running host, because C4 is the one constraint that no source in this research measured for either candidate.
