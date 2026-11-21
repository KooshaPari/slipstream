# Competitor gaps: what self-hosted streaming stacks already solve

Status: research snapshot, 2026-11-16. This is a reference document, not a roadmap. It records what
other self-hosted streaming hosts and clients do, where they are weak, and which gaps are worth
closing in Slipstream.

Legend for the feature tables:

| Mark | Meaning |
| --- | --- |
| Y | Present and documented by the project. |
| P | Partial, conditional, or only on some platforms. The note says which. |
| N | Absent, or no first-party support documented. |
| ? | Not established from the project's own material. Treat as unverified. |

Slipstream baseline used for comparison: host backends exist for Linux and Windows only. Native
clients exist for Android, Android TV, and Steam Deck, with an Apple client tree in the repository.
Moonlight clients can connect through the optional GameStream compatibility path. Source:
<https://github.com/vindeckyy/slipstream> and the repository docs under
`docs-site/content/docs/` (`support-matrix.md`, `virtual-displays.md`, `clients.md`).

---

## 1. Moonlight

**What it is and who maintains it.** Moonlight is the open-source client family for NVIDIA's
GameStream protocol. It is maintained by the Moonlight Stream project, not by NVIDIA. It is a client
only. It cannot host. Source: <https://github.com/moonlight-stream/moonlight-qt>.

**Host platforms.** None. Moonlight never hosts.

**Client platforms.** Windows, macOS, Linux, Raspberry Pi 4/5, Steam Link hardware, Android, Amazon
Fire, iOS, Apple TV, ChromeOS. Community ports exist for PS Vita, Xbox, LG webOS, and embedded
Linux. Source: Moonlight FAQ,
<https://github.com/moonlight-stream/moonlight-docs/wiki/Frequently-Asked-Questions>.

**Protocol.** NVIDIA GameStream, reverse engineered. Sunshine and its forks extend it with HDR,
4:4:4, AV1, and other negotiated additions. Source: Moonlight README and FAQ.

| Feature | Moonlight | Note |
| --- | --- | --- |
| Virtual displays | N | Client only. |
| Multi-monitor | N | One stream at a time. Multiple hosts can be saved and switched between. |
| HDR | Y | Requires a compatible host. |
| 4:4:4 | Y | Sunshine only, per the Moonlight README. |
| Clipboard | N | No first-party clipboard sync in the client. |
| File transfer | N | Not documented. |
| Gamepad | Y | Up to 16 players, force feedback and motion. |
| Audio device selection | N | Host chooses the sink. Client decodes what it is sent. |
| Input latency modes | Y | Frame pacing options in the Android client, from lowest latency to smoothest video. |
| Multi-client | N | A client is one endpoint. |
| Session recording | N | No built-in session recording. |
| Web UI | N | The FAQ explicitly says there is no official web client. |
| Headless/VM operation | N | Not applicable. |

**What users praise.** Broadest device coverage of any client, mature hardware decode, low latency,
free and open source, good gamepad and touch support, and a large install base that makes it the de
facto compatibility target.

**What users complain about.** The GameStream protocol is the ceiling. Official FAQ limits include:
no web client because browsers lack raw TCP/UDP sockets, no USB passthrough except through
VirtualHere, a 150 Mbps slider ceiling, App Store restrictions on iOS and Apple TV, and weak
non-Xbox input passthrough. Source: Moonlight FAQ.

**What it would take for us to be meaningfully better.** Moonlight is not our competitor as a host.
It is the client surface we must either satisfy or replace. Slipstream is meaningfully better only
if our native clients cover the same platforms plus Apple, and if one client can manage several
hosts without the user treating each host as a separate app.

---

## 2. Sunshine

**What it is and who maintains it.** Sunshine is the reference open-source GameStream host for
Moonlight, maintained by LizardByte. Source: <https://github.com/LizardByte/Sunshine>.

**Host platforms.** FreeBSD, Linux, macOS, and Windows. A Docker image exists. Source: Sunshine
README compatibility tables.

**Client platforms.** Any Moonlight client, which means the full Moonlight device list above. It
does not ship its own client.

**Protocol.** GameStream, extended by Sunshine. Source: Sunshine README.

| Feature | Sunshine | Note |
| --- | --- | --- |
| Virtual displays | P | No first-party virtual display driver. Streams an existing output or a third-party VDD. Config exposes `dd_*` options for an external virtual display device. |
| Multi-monitor | P | Selects one output per session via `output_name`. Does not present several monitors to one client. |
| HDR | Y | Windows and Linux. macOS status not established here. |
| 4:4:4 | Y | HEVC only, per Moonlight's "Sunshine only" 4:4:4 support and Sunshine's codec paths. |
| Clipboard | N | No clipboard sync option in the configuration reference. |
| File transfer | N | Not documented. |
| Gamepad | Y | Virtual HID driver or ViGEmBus on Windows, uinput on Linux. |
| Audio device selection | Y | `audio_sink` and `virtual_sink`. |
| Input latency modes | P | Client-side frame pacing. Host has `minimum_fps_target` but no named latency modes. |
| Multi-client | N | One active session at a time. |
| Session recording | N | Not a Sunshine feature. |
| Web UI | Y | Configuration and pairing UI. |
| Headless/VM operation | P | Linux KMS/DRM and headless compositor paths can work. Windows normally needs a display, dummy plug, or VDD. Docker needs GPU passthrough. |

Sources: Sunshine README and configuration reference,
<https://docs.lizardbyte.dev/projects/sunshine/latest/md_docs_2configuration.html>.

**What users praise.** Broad OS and GPU support, active maintenance, a mature web UI, hardware encode
across AMD, Intel, and NVIDIA, HDR and 4:4:4, and compatibility with the enormous Moonlight client
ecosystem.

**What users complain about.** Setup friction is the recurring theme: firewall rules, encoder
selection, display selection, and virtual display or dummy plug workarounds. HDR is fragile and can
look dark or yellow when the chain is wrong. Virtual displays depend on third-party drivers or OS
features. Linux capture depends on the compositor and the DRM stack. It is single-session. There is no
clipboard sync, file transfer, or session recording. AV1 encode is limited to recent GPUs.

**What it would take for us to be meaningfully better.** Ship first-party per-client virtual displays
on every supported compositor, allow more than one simultaneous client, add clipboard and file
transfer, and make headless operation work without a dummy plug or a third-party driver. Also record
sessions, which Sunshine does not do.

---

## 3. Zenith

**What it is and who maintains it.** Zenith is a Linux-first fork of Sunshine by Jackson Pate. It
targets Moonlight and VoidLink clients and is installed through its own `zenith` binary and service
identity while reading Sunshine's config directory. Source: <https://github.com/jacksonpate/zenith>.

**Host platforms.** Linux (KDE, GNOME, Sway/wlroots, Cinnamon, plus Asahi Linux on Apple Silicon) and
Windows 10/11. No macOS host is documented. Source: Zenith README install table.

**Client platforms.** Moonlight clients, plus VoidLink and Foundation-family clients. Client platform
coverage is therefore inherited from Moonlight plus those forks.

**Protocol.** GameStream, extended. Zenith is wire-compatible with VoidLink and Foundation-family
clients for its clipboard extension. Source: Zenith README.

| Feature | Zenith | Note |
| --- | --- | --- |
| Virtual displays | Y | "Headless" and "Dual" work out of the box. Borrows a spare connector with a generated EDID, or falls back to EVDI. Windows uses a bundled signed SudoVDA driver. |
| Multi-monitor | P | Per-client display with remembered position and zoom. Several clients can each get a display. A single client with several heads is not claimed. |
| HDR | ? | Inherits Sunshine paths, but HDR is not claimed for Zenith's virtual displays in its README. Unverified. |
| 4:4:4 | P | Inherited from Sunshine's HEVC path. Not separately claimed. |
| Clipboard | Y | Both directions, text and images, large payloads over TLS. |
| File transfer | P | Host-to-client file push, marked beta. |
| Gamepad | Y | Inherited from Sunshine. |
| Audio device selection | Y | Inherited from Sunshine. |
| Input latency modes | P | Present-paced KMS capture. No user-facing latency mode ladder. |
| Multi-client | P | Per-client display identity is claimed. Simultaneous sessions are not explicitly documented. Unverified. |
| Session recording | N | Not documented. |
| Web UI | Y | Web UI on port 47990. |
| Headless/VM operation | Y | "Headless" display is a headline feature, no dummy plug. |

**What users praise.** Plug-and-play virtual displays, no kernel module on most machines, position
and zoom memory across reconnects (including different client devices), AMD as a first-class vendor,
present-paced capture, remote microphone, and two-way clipboard. It explicitly ports Windows-only
Apollo ideas to Linux the native way.

**What users complain about.** The Windows installer is not code-signed, so SmartScreen warns. Settings
do not carry over from Sunshine even though pairings do. EVDI fallback means DKMS, Secure Boot
enrollment, and reboots on machines with no spare connector. It is a young, small project. HDR on
virtual displays is not clearly documented. Packages conflict with and replace Sunshine, so only one
host can own the ports and display stack.

**What it would take for us to be meaningfully better.** Match the no-dummy-plug virtual display
experience across our Linux compositors, then beat it on verified multi-client isolation, HDR on
virtual displays, session recording, and orchestration across more than one host. We should not
require EVDI or a kernel module where the compositor can create a headless output itself.

---

## 4. Apollo

**What it is and who maintains it.** Apollo is a Sunshine fork by ClassicOldSong aimed at Artemis, the
maintainer's Moonlight Android fork. Source: <https://github.com/ClassicOldSong/Apollo>.

**Host platforms.** Windows is the primary target and the only platform with virtual display support.
The README's requirements also list macOS 12+ and several Linux distributions, inherited from
Sunshine, but virtual display support is explicitly Windows-only at the time of writing. Source:
Apollo README.

**Client platforms.** Artemis (Android only) and Moonlight clients. The README says Apollo and Artemis
will eventually diverge from original Sunshine and Moonlight compatibility.

**Protocol.** GameStream, with Apollo/Artemis extensions. Source: Apollo README.

| Feature | Apollo | Note |
| --- | --- | --- |
| Virtual displays | Y | Windows only, built on SudoVDA. Fixed identity per client so Windows remembers layout. |
| Multi-monitor | P | One display per client. Multiple host instances can create multiple virtual displays. |
| HDR | Y | Windows 11 23H2 and later, generally 24H2. The maintainer still calls HDR generally not recommended. |
| 4:4:4 | P | Inherited Sunshine HEVC path. |
| Clipboard | Y | Clipboard sync is a headline feature. |
| File transfer | N | Not documented. |
| Gamepad | Y | ViGEm-based, inherited from Sunshine. |
| Audio device selection | Y | Inherited from Sunshine. Headless mode is documented for dual-GPU laptops. |
| Input latency modes | P | Inherited Sunshine behavior, plus an input-only mode that is not a latency mode. |
| Multi-client | P | Permissions per client and multiple instances for multiple virtual displays. Not one process serving many isolated sessions. |
| Session recording | N | Not documented. |
| Web UI | Y | Configuration and pairing UI. |
| Headless/VM operation | Y | Headless mode with no dummy plug, render and encode on the selected GPU. |

**What users praise.** The easiest Windows virtual display story. A client appears as a dedicated PnP
monitor, layout is remembered per client, HDR is available, clipboard works, permissions are
per-device, and commands can run on connect and disconnect. It removes the dummy plug.

**What users complain about.** Virtual display is Windows-only and Linux support is "planned" with no
date. The README warns users to remove every other virtual display solution first, which is a sign of
driver conflict pain. HDR is described by the maintainer as a mess across devices and standards. The
project has diverged from Sunshine and Moonlight and support is GitHub-only by policy. Multiple
clients usually means multiple host instances.

**What it would take for us to be meaningfully better.** Make virtual displays work on Linux and
Windows without a third-party driver conflict story, make HDR correct rather than conditional, support
several isolated clients from one host process, and add recording. If we support Artemis-style
extensions, do it without forking the client protocol into incompatibility.

---

## 5. Vibepollo

**What it is and who maintains it.** Vibepollo is an AI-assisted fork of Apollo by Nonary. It describes
itself as an AI-enhanced version of Apollo that integrates Nonary's scripts and features. Source:
<https://github.com/Nonary/Vibepollo>.

**Host platforms.** Windows, plus a Linux beta targeting Arch Linux and CachyOS, kernel 6.16 or newer,
KDE Plasma 6 on Wayland, and NVIDIA or modern AMD GPUs. Source: Vibepollo README.

**Client platforms.** Moonlight clients for standard H.264/HEVC/AV1. PyroWave, VRR pacing, and
DualSense waveform haptics require Nonary's VRR Moonlight client fork. Source: Vibepollo README.

**Protocol.** GameStream, with a proprietary PyroWave wavelet codec path and other extensions.

| Feature | Vibepollo | Note |
| --- | --- | --- |
| Virtual displays | Y | Bundled virtual display driver, SudoVDA kept as rollback. Up to eight clients each get a virtual screen, default four. |
| Multi-monitor | Y | Remote Monitor adds an independently streamed display. Client display arrangement can be saved. |
| HDR | Y | Virtual display HDR, PyroWave SDR/HDR. |
| 4:4:4 | Y | PyroWave 4:4:4 where the GPU allows. |
| Clipboard | P | Inherited from Apollo, but not listed in Vibepollo's README. Unverified. |
| File transfer | N | Not documented. |
| Gamepad | Y | Own free virtual gamepad driver, plus DualSense haptics. |
| Audio device selection | Y | Inherited, plus microphone passthrough in downstream forks. |
| Input latency modes | Y | VRR capture, RTSS frame limiting, frame pacing, present-driven capture on Linux. |
| Multi-client | Y | Up to eight virtual clients, default four. |
| Session recording | N | Not documented. |
| Web UI | Y | Responsive browser interface with dashboard and recovery. |
| Headless/VM operation | Y | Auto virtual display on headless setups to avoid encoder misdetection. Login-screen capture on Windows and NVIDIA Linux. |

**What users praise.** No dummy plugs, per-client virtual screens, Windows Graphics Capture in service
mode, VRR capture, PyroWave's low latency and frame independence, game library sync, DualSense
features, and a Linux beta that already targets HDR virtual displays.

**What users complain about.** The README itself says roughly 99 percent of the code is AI-generated
and carries surface-level technical debt. The Linux beta is narrow: Arch and CachyOS, kernel 6.16,
KDE Wayland, NVIDIA or modern AMD. Best features require a patched Moonlight client. The feature list
is broad and Windows-centric, and it is explicitly not going upstream in large merges. Users inherit
fork divergence and a fast-moving surface.

**What it would take for us to be meaningfully better.** Offer the same no-dummy-plug virtual display
and multi-client capability on a broader distro and compositor matrix, with a stable client that does
not need a special fork for the good path. Then add session recording and multi-host orchestration,
which Vibepollo does not have.

---

## 6. Wolf (games-on-whales)

**What it is and who maintains it.** Wolf is a Linux and Docker-first streaming server for Moonlight,
maintained by the games-on-whales project. It is designed to share one host among several users, each
running games in containers. Source: <https://github.com/games-on-whales/wolf>.

**Host platforms.** Linux only, via Docker or Podman containers. No Windows or macOS host.

**Client platforms.** Moonlight clients. Wolf does not ship its own client.

**Protocol.** GameStream, as a Moonlight server.

| Feature | Wolf | Note |
| --- | --- | --- |
| Virtual displays | Y | On-demand virtual desktops sized to the client, no monitor or dummy plug. |
| Multi-monitor | P | One virtual desktop per client. Multiple heads per client not documented. |
| HDR | ? | Not established from the project README. Unverified. |
| 4:4:4 | ? | Not established. Unverified. |
| Clipboard | N | Not documented. |
| File transfer | N | Not documented. |
| Gamepad | Y | Gamepad support is a stated goal. |
| Audio device selection | P | Per-container audio plumbing. Exact selection UI unverified. |
| Input latency modes | N | No documented mode ladder. |
| Multi-client | Y | Core design: several users stream different content from one host. |
| Session recording | N | Not documented. |
| Web UI | P | Configuration is file-first. No first-party web UI established. Unverified. |
| Headless/VM operation | Y | Containerized virtual desktops are the product. |

**What users praise.** True multi-user streaming from one machine, container isolation, multi-GPU
assignment, virtual desktops without dummy plugs, and a Linux/Docker-native design.

**What users complain about.** Steep Docker, GPU passthrough, and per-game container setup. Linux-only.
Niche ecosystem and configuration-file workflow. HDR, 4:4:4, clipboard, and recording are not the
project's focus.

**What it would take for us to be meaningfully better.** Match multi-user isolation without requiring
Docker expertise, and add the desktop conveniences Wolf lacks: HDR, 4:4:4, clipboard, recording, a
real web console, and cross-platform hosts.

---

## 7. Helios (Moonlight OS)

**What it is and who maintains it.** Helios is the Moonlight OS streaming host, maintained by the
moonlight-os project. It pairs with Selene and other Moonlight-compatible clients and adds negotiated
extensions. Source: <https://github.com/moonlight-os/helios>.

**Host platforms.** Windows 10 or later, macOS 12 or later, and current Linux distributions, per the
requirements section.

**Client platforms.** Selene, plus Moonlight-compatible clients. Extensions only apply when the client
advertises them, otherwise the session falls back to the compatible Moonlight path.

**Protocol.** Moonlight/GameStream with negotiated extensions over an authenticated QUIC transport.

| Feature | Helios | Note |
| --- | --- | --- |
| Virtual displays | Y | Stable per-client display identity. Windows uses SudoVDA, Linux uses Sway wlroots headless outputs. |
| Multi-monitor | Y | Multi-display layouts and a display topology extension are documented. |
| HDR | P | README says availability depends on host, capture path, encoder, client, and display. No blanket claim. |
| 4:4:4 | ? | Not established. Unverified. |
| Clipboard | Y | Per-client clipboard permission is listed as a feature. |
| File transfer | P | Per-client file permissions and read-only system-disk access exist. A transfer UI is not established. |
| Gamepad | P | Assumed through the Moonlight path. Not separately documented. |
| Audio device selection | ? | Not established. Unverified. |
| Input latency modes | P | QUIC transport is claimed. No named latency modes. |
| Multi-client | Y | Per-client permissions and sessions. Input-only sessions are supported. |
| Session recording | N | Not documented. |
| Web UI | Y | Local browser-based configuration interface. |
| Headless/VM operation | Y | Can start a private headless Sway session. Linux virtual displays depend on Sway. |

**What users praise.** Per-client permissions, USB forwarding, microphone and camera uplinks,
input-only sessions, stable display identities, multi-display topology, and an authenticated QUIC
transport. It is one of the few projects treating a stream as a permissioned session rather than a
screen share.

**What users complain about.** It is new and small. Extensions need Selene or an extension-aware
client. Linux virtual displays are tied to Sway. Windows uses SudoVDA, with the usual driver conflict
story. Documentation is sparse.

**What it would take for us to be meaningfully better.** Support the full Moonlight client install base
without requiring an extension-aware client, work across more Linux compositors than Sway, and add
recording and host chaining. Permissions and input-only sessions are table stakes we should match.

---

## 8. Adjacent projects and names you will see

- **NVIDIA GameStream.** The original proprietary host, Windows and NVIDIA only. NVIDIA ended the
  service. It remains the protocol's origin, not a maintained competitor. Source: Moonlight FAQ's
  GameStream end-of-service page.
- **Artemis.** ClassicOldSong's Android Moonlight fork, the companion client for Apollo.
- **VoidLink, Selene, Nebula.** Clients or companion apps in the Zenith, Moonlight OS, and Nova
  ecosystems. They show the ecosystem splitting into extension-aware client families.
- **Nova.** A newer Linux-first fork of Zenith by Fenyx, adding browser streaming, game library,
  clipboard, remote microphone, and per-device permissions. Source:
  <https://github.com/F-e-n-y-x/nova-host>. It is young and not independently verified here.
- **Sunshine-Foundation.** A Sunshine fork referenced by Zenith and Apollo as feature inspiration.
  Not separately reviewed in this document.

**Unverified / not reviewed.** Parsec is proprietary and not self-hosted, so it is out of scope.
Rainway is discontinued. A long tail of forks exists that this document does not claim to cover.

---

## Cross-project synthesis

### 1. What is table stakes for a credible host

These are the capabilities that every serious host now has, or is expected to have within one release:

1. **Per-client resolution and refresh without a dummy plug.** Sunshine, Zenith, Apollo, Vibepollo,
   Wolf, and Helios all have some form of it. A host that cannot create or borrow a display sized to
   the client is not credible in 2026.
2. **Hardware encode on AMD, Intel, and NVIDIA.** Sunshine set this expectation. Software-only is a
   fallback, not a product.
3. **H.264 and HEVC at minimum, with AV1 on newer GPUs.** Moonlight clients expect all three.
4. **HDR and 4:4:4 on at least one codec path.** HDR is fragile everywhere, but its absence is now a
   visible gap. 4:4:4 is expected for desktop text quality.
5. **Virtual gamepads with rumble, motion, and modern controller profiles.** ViGEm, Virtual HID, or a
   vendor driver. Users do not accept keyboard-only fallback.
6. **Audio capture with device selection and a virtual sink.** Muting host speakers while streaming
   is a solved expectation.
7. **PIN pairing, encrypted transport, and per-client trust.** GameStream compatibility forces a
   weaker control plane, but every project now offers encryption as an option or default.
8. **A web UI for configuration and pairing.** Sunshine, Apollo, Zenith, Vibepollo, Helios, and Nova
   all have one. A config-file-only host feels unfinished.
9. **Adaptive bitrate, FEC, and low-latency transport tuning.** The user expects the stream to
   survive Wi-Fi jitter without a manual bitrate hunt.
10. **Multi-client, becoming table stakes.** Wolf and Vibepollo treat several clients as normal.
    Slipstream already supports several clients. Sunshine and Moonlight still assume one session. This
    is moving from differentiator to expectation, especially for work and family use.
11. **Clipboard sync, becoming table stakes.** Apollo, Zenith, Helios, and Nova have it. Sunshine does
    not. Once users try it, its absence is a regression.

### 2. What nobody does well yet, our differentiation opportunities

These are the places where the field is weak, contradictory, or absent:

1. **One client, several hosts.** Moonlight and Slipstream can save multiple hosts and switch between
   them. Nobody offers a single session view that aggregates hosts, shows their live state, and moves
   input and windows across them. This is the clearest greenfield.
2. **Host chaining with a block toggle.** No project lets host A stream to host B and then to a client,
   with a switch to block or bypass the chain. The closest partials are Wolf's one-host-many-users
   model, Vibepollo's Remote Monitor (an extra display on the same host), and Helios's display
   topology (again, same host). Host-to-host chaining is unowned.
3. **An editable display model with fake displays on top of a monitor representation.** Competitors
   create one synthetic monitor per client and let the OS arrange it. None expose a host-side
   representation of monitors where an operator can layer synthetic or virtual displays onto a
   physical monitor model, name them, and route sessions to them. Slipstream's virtual display policy
   and physical monitor pinning are the nearest foundation, but the abstraction itself does not exist
   elsewhere.
4. **Session recording.** Every host focuses on live delivery. None ships a first-class session
   recorder with per-stage stats, markers, or export. Slipstream has diagnostic capture recording, but
   not user-facing session recording. This is a real product gap for work, support, and content.
5. **Cross-vendor HDR plus 4:4:4 on virtual displays.** Apollo, Zenith, and Vibepollo each solve part
   of it on one OS or one driver. Linux virtual outputs are often 8-bit. Sunshine refuses 4:4:4 and
   HDR together on NVENC. A uniform capability across NVIDIA, AMD, and Intel, on Linux and Windows,
   would be a genuine step.
6. **Headless and VM operation without a dummy plug or a kernel module.** Zenith gets closest with
   borrowed connectors and EVDI fallback. Vibepollo uses a bundled driver. Nobody makes it uniform
   across compositors and hypervisors.
7. **File transfer as a first-class, policy-aware feature.** Only Zenith has a beta host-to-client
   push. Slipstream's clipboard policy can already permit or refuse files, but no shipping client asks
   for it. This is close to unowned.
8. **Per-client permissioned sessions, input-only clients, and USB or camera uplinks.** Helios and
   Nova are exploring this. Most hosts still treat a paired client as fully trusted. This is a
   meaningful security and product differentiator.

### 3. Our operator's specific asks, and what exists today

| Ask | Exists today? | Partial form |
| --- | --- | --- |
| One client connecting to multiple hosts | Partial | Moonlight saves several hosts and switches between them. Slipstream saves host records and can reconnect. Neither multiplexes hosts into one session or one live view. |
| Chaining hosts together with a block toggle | No | Wolf shares one host among users. Vibepollo Remote Monitor adds a display on the same host. Helios extends display topology on the same host. No project chains host A through host B to a client, and none offers a block/bypass toggle. |
| Adding fake displays on top of the monitor representation | No, as an abstraction | Virtual display drivers (SudoVDA, EVDI, KWin/GNOME/Sway headless outputs) add synthetic monitors to the OS, usually one per client. Zenith, Apollo, and Vibepollo remember per-client display identity. None expose an editable monitor model where fake displays are layered onto a physical monitor representation. Slipstream's virtual display presets and physical monitor pinning are the closest pieces, not the full abstraction. |

### 4. Where the ecosystem's centre of gravity is

The centre of gravity is **GameStream/Moonlight compatibility**, clearly and unambiguously.

Evidence:

- Every host reviewed here targets Moonlight: Sunshine, Zenith, Apollo, Vibepollo, Wolf, Helios, and
  Nova. Several are Sunshine forks.
- Every client is a Moonlight client or a Moonlight derivative (Artemis, VoidLink, Selene, Nebula).
- Even Helios, which adds negotiated QUIC extensions, falls back to the compatible Moonlight path for
  clients that do not advertise extensions.
- Slipstream itself ships an optional GameStream path for exactly this reason.

**Cost of staying GameStream-compatible.** You inherit the client install base, pairing model, and
codec ecosystem for free. You also inherit the protocol's ceilings: 4:2:0 for GameStream sessions in
Slipstream, weaker control-plane crypto, no native multi-host semantics, no native clipboard or file
transfer, no native display topology, and a client feature set that updates only when Moonlight or a
fork updates.

**Cost of a native protocol.** You control transport, security, multi-host, multi-client, display
topology, clipboard, file transfer, recording metadata, and latency instrumentation. You must build
and maintain clients on every platform you care about, and you lose the long tail of Moonlight ports.
Slipstream's `slipstream/1` path with QUIC control and UDP media is already this bet.

**The practical answer.** Do both, but do not let compatibility define the product. Use GameStream as
an interoperability plane for Moonlight devices. Put the differentiating features (multi-host,
chaining, display model, recording, permissions, clipboard and file transfer) on the native plane,
and expose them through native clients. That is the only path that reaches Moonlight's install base
without inheriting its limits.

---

## Five gaps worth owning

1. **One client, many hosts, one live view.** Moonlight and Slipstream can switch hosts but not
   aggregate them, so a unified host switcher and session dashboard would be immediately legible and
   has no direct competitor.
2. **Host chaining with a block toggle.** No project implements host A to host B to client streaming,
   which makes it a clean, defensible topology feature rather than a catch-up item.
3. **An editable monitor representation with layered fake displays.** Competitors add one synthetic
   monitor per client and stop there, so an operator-facing display model is genuinely new.
4. **First-class session recording with per-stage evidence.** No host ships user-facing recording, and
   Slipstream already has the diagnostic capture machinery to build on.
5. **Uniform cross-vendor HDR plus 4:4:4 on virtual displays.** Every competitor solves a slice on one
   OS or one driver, and Linux virtual outputs are often 8-bit, so a uniform capability would be
   visibly better rather than merely equal.

## Four things not worth building because someone else already owns it

1. **GameStream/Moonlight wire compatibility from scratch.** Sunshine and its forks own this, and
   Moonlight owns the clients. Interoperate with it, do not reimplement the protocol.
2. **Per-OS virtual display drivers.** SudoVDA, EVDI, KWin/GNOME/Sway headless outputs, and bundled
   drivers are commoditized and fragile. Integrate with them, and own the policy layer above them
   instead.
3. **Virtual gamepad and HID emulation.** ViGEm, Virtual HID, and uinput paths are solved across
   Sunshine, Apollo, and Vibepollo. Reuse or wrap them, do not build another driver.
4. **A basic web console, pairing flow, and game library.** Sunshine, Apollo, Zenith, Vibepollo,
   Helios, and Nova all have mature versions. Match the table stakes, then spend the effort on
   topology, orchestration, and recording instead.
