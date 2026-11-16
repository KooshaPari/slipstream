## For the operator: read this first

Today Slipstream hosts on Linux and Windows. On Linux it creates per-client virtual displays across
KWin, Mutter, gamescope, Sway, and Hyprland, and it can stream to several clients. On Windows, video
works, but through the mirror path: the `e9aaef9` session captured `\\.\DISPLAY1` after setting the
real head to 1920x1080, which is `WindowsMirrorDisplay`, not an IDD virtual display. Adding virtual
displays on Windows still requires the operator to install the RustDesk indirect display driver. The
native client surface is Android, Android TV, and Steam Deck, with an Apple app target for macOS,
iOS, iPadOS, and tvOS in the tree.

The first things a Parsec user would notice, in order: Windows cannot add a virtual display until a
driver is installed, so a laptop user does not get the second screen Parsec gives them. One client
cannot hold several hosts in one live view; it can save and reconnect, but every switch is a connect
ceremony. The in-session overlay, compact stats shape, and error language differ per client, so
muscle memory from Parsec does not transfer. Per-device capability and revocation are partial, so an
operator cannot see and remove a device with the confidence Parsec's device list gives.

Start with the client multi-session owner and host switcher: it is 3-5 days of client work, needs no
protocol or host change, and unblocks warm sessions, chaining UX, and display naming. Do not start
daisy chaining. It needs a seat arbiter, a client stack inside the host, and a relay authority model
that do not exist, and its latency cost is unmeasured.

# Product reference set

These documents are deferred reference material for after the streaming core is finished. They exist
so the operator's expectations, competitor gaps, and the shapes of the harder topology features are
written down once, while the architecture is fresh. Nothing here is a commitment to build, a
schedule, or a promise that any item is next. Read them as a shared vocabulary for later decisions,
not as a backlog that is allowed to pull focus from the core.

## The documents

| Document | What it answers | Who reads it |
| --- | --- | --- |
| [`competitor-gaps.md`](competitor-gaps.md) | What Sunshine, Moonlight, Zenith, Apollo, Vibepollo, Wolf, and Helios already solve, where they are weak, and which gaps are unowned. | Product and strategy readers deciding what is table stakes and what is genuinely differentiating. |
| [`parsec-parity.md`](parsec-parity.md) | A feature-by-feature inventory of what Parsec does, what our current state is, and which parity items matter to a power user. | Product and design readers scoping parity work, plus implementers who need the evidence-backed current-state value for a feature. |
| [`session-topology.md`](session-topology.md) | Design for one client to many hosts, daisy chaining with a block toggle, and Parsec-style virtual display management, including effort estimates and failure analysis. | Protocol and host engineers planning the topology features, and anyone who needs to know why chaining is sequenced last. |
| `macos-host-backend.md` (landed during this synthesis) | The macOS host backend. It landed while this index was being written and is not yet incorporated into the decision table or open questions below. | Platform engineers, once the document has been reviewed alongside this set. |

## Decision table

This table merges the highest-value items across the three completed documents. `Source` names the
document that owns the item. `Our current state` copies the evidence-backed value from the source. If
two documents disagree, the cell says so instead of choosing one. `Effort` is copied from
`session-topology.md` where that document gives a figure, otherwise it says `unknown`. Build order is
1 to 18, ordered by whether the item unblocks other work, whether it needs a protocol change, and
whether it needs a new host capability.

| # | Item | Source | Our current state | Effort | Build order |
| --- | --- | --- | --- | --- | --- |
| 1 | Client multi-session owner and explicit host switcher | session-topology §1, competitor-gaps gap 1 | No client-side notion of a second host inside one session process. Slipstream saves host records and reconnects, but does not multiplex hosts or present one live view. | 3-5 days for the smallest version | 1 |
| 2 | Named, persistently created virtual displays (reading 3a) | session-topology §3a, parsec-parity §9, competitor-gaps gap 3 | Virtual displays exist, but only as a side effect of a session. There is no explicit create, keep, name, or destroy API. Parsec-parity calls the explicit add-display action partial and `verify`. | 1.5-2 weeks on Linux, +1 week for Windows IDD | 2 |
| 3 | Per-device capability record, visible paired-device list, and immediate revocation | parsec-parity §1 and top 10 #1 | Paired device records exist. Revocation and re-pair are documented, but enforcement strength is `verify`. Per-feature permissions are partial: clipboard and microphone only. | unknown | 3 |
| 4 | Warm-idle multi-host with `MSG_SESSION_SUSPEND` and `MSG_SESSION_RESUME` | session-topology §1 | Only active, lingering, and cold tiers exist. No low-fps encode mode. Keep-alive already keeps a display after a session ends. | 1.5-2.5 weeks including admission tests | 4 |
| 5 | Canonical in-session overlay, compact stats mode, and one error/retry taxonomy | parsec-parity top 10 #3, #4, #5 | Overlay contents differ per client. Stats are richer than Parsec's but not in the compact fps/Mbps/ms/loss shape. Error surfaces vary by client. | unknown | 5 |
| 6 | Host status with connected client name, resolution, bitrate, and per-client stop | parsec-parity §7 and top 10 #7 | Sessions list exists. Per-client disconnect and a per-row stop control are `verify` or partial. | unknown | 6 |
| 7 | Hardware 4:4:4 path over wired LAN | parsec-parity top 10 #8, competitor-gaps gap 5 | Docs disagree. Parsec-parity says 4:4:4 is only available through PyroWave and that hardware HEVC/AV1 paths are 4:2:0. Competitor-gaps describes 4:4:4 as a table-stakes capability and cites HEVC as a carrier in the Slipstream support matrix. Treat the current state as contested. | unknown | 7 |
| 8 | Uniform cross-vendor HDR plus 4:4:4 on virtual displays | competitor-gaps gap 5 | HDR works on capable chains. 4:4:4 is partial. Linux virtual outputs are often 8-bit according to the source material. | unknown | 8 |
| 9 | User-facing session recording | competitor-gaps gap 4 | Diagnostic capture recording exists. There is no user-facing session recording with markers or export. | unknown | 9 |
| 10 | File transfer over the session transport, or a policy-aware clipboard file channel | competitor-gaps gap 7, parsec-parity §6 | Clipboard file transfer: resolved from code as text works end to end but files are wire-only; the blocker is host-side (`ss-clipboard` backend Phase 2), and a client implementation alone would not deliver it because no host backend advertises or serves file formats [crates/slipstream-core/src/net/quic/control.rs:569-578, 619, 671; crates/ss-clipboard/src/backend/host.rs:228-246; crates/ss-clipboard/src/lib.rs:28-47; clients/apple/Sources/SlipstreamKit/Clipboard/ClipboardSync.swift:16, 116; clients/android/app/src/main/kotlin/io/slipstream/ClipboardSync.kt:11]. The two documents disagreed because parsec-parity judged the shipping feature and wrote `none`, while competitor-gaps judged the policy bit and wrote that files are permitted but no client asks. | gated on host backend landing first | 10 |
| 11 | Windows host real-GPU validation and virtual display ownership | parsec-parity §11, session-topology §3 | Windows host virtual display maturity: resolved from code as requires a pre-installed driver (user install step), with the `e9aaef9` session bypassing IDD because virtual display is opt-in and the default is mirror, so `\\.\DISPLAY1` is the `WindowsMirrorDisplay` path [crates/ss-vdisplay/src/display/windows_idd.rs:11-13, 63-65, 106-108; crates/ss-vdisplay/src/lib.rs:492-496; crates/ss-vdisplay/src/display/windows.rs:98-110, 137-154]. The two documents disagreed because parsec-parity described the external driver step while session-topology described the host-side code that consumes an already-installed driver. | unknown | 11 |
| 12 | Desktop Linux client or a credible macOS client | parsec-parity top 10 #10 | Desktop Linux client or credible macOS client: resolved from code as a distinct macOS app target exists alongside iOS and tvOS and shares the SlipstreamKit session, decode, presenter, and fullscreen code, with macOS-specific AppKit fullscreen, Scene Stream menu, sheet presentation, and App Sandbox surfaces [clients/apple/Slipstream.xcodeproj/project.pbxproj:158-232, 421, 582, 655; clients/apple/Package.swift:7-10; clients/apple/Sources/SlipstreamClient/FullscreenController.swift:4-14; clients/apple/Sources/SlipstreamClient/ContentView.swift:14; clients/apple/README.md:38, 111-115]. iOS-only surfaces are touch/pointer input and `fullScreenCover`, while macOS uses sheets and AppKit fullscreen. The two documents disagreed because parsec-parity treated the Apple tree as iOS/iPadOS/tvOS only, while the package and Xcode project declare a macOS target. | unknown | 12 |
| 13 | macOS host backend | parsec-parity §11, `macos-host-backend.md` pending review | None. The macOS host document landed during this synthesis but is not yet incorporated here, so no effort or design value is copied. | unknown | 13 |
| 14 | Seat arbiter for local versus relayed input | session-topology §2 | None. The injector applies whatever input it receives, with no local-versus-relayed ownership model. | 1 week, and a prerequisite for chaining | 14 |
| 15 | Static relay chaining, no toggle | session-topology §2, competitor-gaps ask 2 | None. No reviewed project chains host A through host B to a client. | 2-3 weeks | 15 |
| 16 | Relay block toggle and local source switch | session-topology §2, competitor-gaps ask 2 | None. The block toggle and source switch are separate mechanisms in the design. | +3-5 days for the gate, +1-2 weeks for the local source switch | 16 |
| 17 | Derived head over a physical monitor (reading 3b) | session-topology §3b, competitor-gaps gap 3 | None. The mode-restore guard is single-owner and the ownership question is unresolved. | 2 weeks for one client, no slicing, +2-3 weeks for per-client slices | 17 |
| 18 | Multi-display for a single client, or a multi-window presenter | parsec-parity §3 and §8, competitor-gaps ask 3 | Single-client multi-display: resolved from code as not available, one host display per session [crates/slipstream-core/src/net/quic/handshake.rs:13-95; crates/slipstream-core/src/net/quic/control.rs:17-19; crates/ss-client-core/src/runtime/session.rs:586-596; crates/slipstream-core/src/runtime/client/mod.rs:661-672]. `Hello` carries a mode and capabilities but no display or monitor selector, `Reconfigure` carries only `Mode`, and the client's only mid-session switch is `request_mode(Mode)`. The two documents disagreed because parsec-parity had only marked the count `needs checking`, while competitor-gaps inferred not-yet-available from the docs; the protocol settles it. | unknown | 18 |

Row count: 18.

## Not building

These are genuinely not worth building, with the reason. This is not the same as "deferred". Deferred
items stay in the decision table. These are rejected or should be treated as integration points
instead of product work.

- **GameStream/Moonlight wire compatibility from scratch.** Sunshine and its forks own the protocol,
  and Moonlight owns the clients. Interoperate with it. Do not reimplement it as a product strategy.
- **A new per-OS virtual display driver as the only path.** SudoVDA, EVDI, KWin/GNOME/Sway headless
  outputs, and the Windows IDD path are commoditized and fragile. Integrate with them and own the
  policy layer above them. The parsec-parity document reaches the same conclusion: keep the Windows
  driver optional and do not make a kernel-mode driver the only path.
- **Virtual gamepad and HID emulation.** ViGEm, Virtual HID, and uinput paths are solved across
  Sunshine, Apollo, and Vibepollo. Reuse or wrap them.
- **A basic web console, pairing flow, and game library.** Sunshine, Apollo, Zenith, Vibepollo, Helios,
  and Nova all have mature versions. Match the table stakes, then spend the effort on topology,
  orchestration, and recording.
- **An account as a precondition, a friends graph, and a party system.** Parsec's account enables
  central revocation but makes the product useless when the service is down. Pairing and guest tokens
  cover the actual need. The parsec-parity document lists these as items not to copy.
- **The Teams/enterprise permission matrix as a first-class surface.** Build the per-device capability
  primitive, not the org chart.
- **Paid gating of virtual displays or bitrate.** Never charge by degrading the stream. Charge for
  scale if charging at all.
- **A browser client as a parity checkbox.** WebCodecs gives up 4:4:4, low-latency pacing, and most
  gamepad fidelity. Reach is not worth claiming parity that cannot be delivered.
- **Fire TV and other fragmented TV platforms.** Android TV covers the same users with one codebase.
- **Parsec's raw quality sliders and silent mode changes.** Presets and profile intent are better than
  six numbers. Failing loudly beats silently giving a different resolution.
- **Relay-mediated connectivity by default.** It is why Parsec needs an account and a service.
  Explicit LAN and VPN connectivity is the honest trade.

**Host chaining is deliberately not on this list.** It is unowned by every reviewed competitor, the
operator explicitly asked for it, and `session-topology.md` provides a design with effort estimates.
It belongs in the decision table at orders 14 to 16, behind the seat arbiter, because it is expensive
and conditional, not because it is a bad idea. It would move to this not-building list only if we
decide the relay authority model, the host-side client stack, or the measured latency cost is
unacceptable. That decision has not been made.

## Dependency graph in prose for the top five

**1. Client multi-session owner and host switcher.** Nothing must exist before it on the host side.
It needs a client front-end that owns a map of host ID to cancel handle and session-event channel,
instead of the single child it owns today, and it needs explicit audio muting for non-active children.
It has no protocol change and no host change. It should come first because it unblocks warm-idle
sessions, host chaining UX, and the vocabulary of "which host and which display am I switching to".

**2. Named, persistently created virtual displays.** It depends on the existing registry, policy
layout, and display-state API, which already carry identity slots, positions, and lifecycle state. It
needs three management routes plus console work, and a name alias over the identity slot rather than a
second identity. It does not need the host switcher technically, but it is easier to explain once a
switcher can name what it switches to. It precedes derived heads and single-client multi-display.

**3. Per-device capability record and revocation.** It depends on the paired-device records that
already exist and on a management API and console surface for per-device capabilities. It needs an
enforcement decision for connected sessions, not just future connections. It should precede
permissioned sessions and relay authority, because a relay or a view-only guest is only meaningful if
the host can name and revoke the device behind it.

**4. Warm-idle multi-host.** It depends on item 1, because a front-end that cannot own several
children cannot keep several hosts warm. It also needs the host-side `MSG_SESSION_SUSPEND` and
`MSG_SESSION_RESUME` pair, a dropped and re-acquired session permit, and a `LiveGuard` that means
"owns" rather than "streaming". Without those, four suspended hosts would occupy every session slot.
It should follow item 1 and should not be attempted before the admission test matrix is updated.

**5. Canonical overlay, compact stats, and error taxonomy.** It depends on a stable client UI shell,
not on the host or the protocol. It can run in parallel with item 4. It should precede chaining
because a relay needs explicit `blocked` and `upstream down` states, and those are error and session
states before they are video states. The compact stats shape also gives the operator a common
vocabulary for latency and loss before any new topology work starts.

## Open questions

Every `verify`, `unverified`, `?`, or `unknown` from the three documents, grouped by source. Where the
same uncertainty appears repeatedly, it is listed once under the tightest label.

### From `parsec-parity.md`

**Parsec behavior that must be checked before building to it**

- Account and access: direct IP without an account, the new-device approval path, guest-link expiry
  and approval options, per-feature permission set on personal accounts, whether a view-only mode
  exists outside Teams, whether console approval works without a PIN, friends-request behavior, party
  feature existence, Teams feature set and permission matrix, account recovery and device limits, and
  current personal-account paid and telemetry limits.
- Session lifecycle: reconnect hold-window length, Wake-on-LAN path, Windows lock-screen
  streamability, the exact "host is busy" wording, host-side idle timeouts, and whether the host app
  tabs and labels match the description.
- Display: whether the client or host owns display selection, Parsec VDD paid gating, the exact VDD
  name, VDD persistence across reboot, per-resolution and per-refresh limits, personal versus Teams
  display-count limits, exact client fit/stretch options, cursor naming, mid-session hot-plug
  behavior, whether Parsec blanks real monitors, and whether a client can request an added display.
- Video: current encoder list and labels, whether AV1 is absent, the personal bitrate ceiling, whether
  a quality slider exists, whether any experimental HDR path exists, exact vsync and pacing toggles,
  the exact stats-overlay fields, high-refresh per-mode limits, color-range and chroma-siting
  behavior, and whether any 4:4:4 or explicit low-latency input toggle exists.
- Audio: whether non-default host devices are selectable, client output-device selection per
  platform, whether host volume is remotely controlled, audio-latency controls, surround support per
  client, and A/V sync behavior.
- Input: the reserved keyboard-shortcut matrix, DirectInput support, the controller limit, gyro,
  touchpad and trigger passthrough, hotkey remapping, clipboard image and rich-format support, whether
  Teams has a file path, pen pressure support, and the exact touch-mode set.
- Host and client UI: home-screen thumbnails, window-mode set, whether a multi-window display view
  exists, exact overlay items and order, whether edge-hover opens the overlay, connection-quality
  thresholds, speed-test absence, game-library absence, add-display location and button flow, display
  naming and default mode, display cleanup behavior, and whether in-app diagnostics are as thin as
  described.
- Platforms: macOS virtual display support, exact macOS permission set, current official Linux
  support, Linux client packaging, Chromebook or web client state, tvOS absence, Raspberry Pi client
  state, Fire TV state, and any Windows lock-screen unlock path.

**Our current state that the document marks for verification**

- Per-device revocation enforcement strength.
- Host lock-screen behavior and per-client stop behavior.
- Mirror mid-session mode change.
- Host audio device selection and Android/Deck client audio-device pickers.
- Multi-controller support.
- Explicit fit/stretch/presentation modes.
- Explicit color-range signaling.
- Explicit A/V offset.
- Tray behavior on Windows and macOS.
- Per-client disconnect and a simple connection-quality badge.
- High-refresh stability per compositor.
- Join latency p50 and click-to-first-frame measurement.
- The explicit add-display action and its naming.
- Display cleanup after a crash.
- Clipboard file-transfer state: resolved in the decision table. Wire support exists, but the host backend and client UI do not.

### From `session-topology.md`

- Whether the native pairing store can carry a small relay attestation without introducing a new
  trust root.
- Whether re-encoding at relay host A keeps interactive latency acceptable, and what the measured
  cost is on a LAN.
- Whether a low-fps warm encode produces a measured latency win over warm-idle, which is explicitly
  left unmeasured before any data-plane cadence change.
- The optional `backend_preference?` field in the display creation API is a design placeholder, not a
  settled contract.

### From `competitor-gaps.md`

- Zenith: HDR on virtual displays is unverified. Simultaneous multi-client sessions are unverified.
- Vibepollo: clipboard sync is inherited from Apollo but not listed in its README, so it is
  unverified. File transfer is not documented.
- Wolf: HDR is unverified. 4:4:4 is unverified. Audio device selection UI is unverified. A first-party
  web UI is unverified.
- Helios: 4:4:4 is unverified. Audio device selection is unverified. Gamepad support is assumed
  through the Moonlight path rather than documented.
- Sunshine: HDR status on macOS is not established here.
- Apollo and Helios 4:4:4 support is inherited or unverified rather than separately claimed.
- Nova is described from its README only and is not independently verified.
- The long tail of forks, and any claim about Parsec or Rainway beyond the reviewed material, is out
  of scope.

### Contradictions resolved from code

1. Windows virtual display maturity: resolved from code as requires a pre-installed driver (user
   install step) [crates/ss-vdisplay/src/display/windows_idd.rs:11-13, 106-108;
   crates/ss-vdisplay/src/lib.rs:492-496]. The two documents disagreed because parsec-parity described
   the external driver step while session-topology described the host-side code that consumes it.
2. Clipboard file transfer: resolved from code as text works end to end but files are wire-only
   [crates/slipstream-core/src/net/quic/control.rs:569-578, 619, 671;
   crates/ss-clipboard/src/backend/host.rs:228-246]. The two documents disagreed because parsec-parity
   judged the shipping feature while competitor-gaps judged the host policy bit.
3. Single-client multi-display: resolved from code as not available, one host display per session
   [crates/slipstream-core/src/net/quic/handshake.rs:13-95;
   crates/slipstream-core/src/net/quic/control.rs:17-19]. The two documents disagreed because
   parsec-parity marked it `needs checking` while competitor-gaps inferred not-yet-available.
