# macOS as a Slipstream host: design and feasibility

Status: scoping document, no code written. Operator has approved pursuing this.
Scope: what it takes to make a Mac **serve** a session (capture, encode, virtual display,
input injection, service model, CI), not to improve the existing Apple client.

This document is deliberately pessimistic. Where a macOS API is documented I name it. Where I
could not confirm behaviour on a real machine I say so. Every such item is collected again in
the [unverified ledger](#11-unverified-ledger).

---

## 0. Verdict in one screen

| Area | Difficulty | Honest read |
|---|---|---|
| Screen capture (ScreenCaptureKit) | Low | Works, documented, fast. Small surprise surface. |
| Encode (VideoToolbox) | Low to medium | Works. We lose reference-frame invalidation and probably 4:4:4. |
| Zero-copy and colour | Low | Genuinely zero-copy on the capture to encode leg. |
| Input injection (keyboard, mouse) | Medium | Works for desktop control. Silently no-ops without Accessibility. |
| Input injection (relative mouse for games) | High | CGEvents are not raw HID. Games will not see them. |
| Gamepad | High | No userland virtual HID. Needs a DriverKit extension. |
| **Virtual display** | **Very high** | **No supported API exists. This is the product-defining risk.** |
| Session and permission model | High | Per-user LaunchAgent only. TCC is per signature. Login window is a wall. |
| Build and CI | Medium | Cannot build on the Windows desk. Needs a Mac and a Developer ID. |

Bottom line: a **whole-display remote desktop host** (no virtual display) is a realistic
2 to 4 week job. A **virtual-display host** is either a $10 HDMI dongle or a private API that
Apple can remove. There is no third option that I can find.

---

## 1. What exists today (verified by inspection)

The repository is at `agents/worktrees/slipstream/spec`. Facts checked directly, not assumed:

- `crates/ss-capture/src/platform/` contains `linux/`, `windows/`, and `mod.rs`. `mod.rs`
  declares only `#[cfg(target_os = "linux")] pub mod linux;` and
  `#[cfg(target_os = "windows")] pub mod windows;`. There is no macOS module, so on macOS there
  is no capturer at all.
- `crates/ss-capture/src/lib.rs` contains **zero** occurrences of the string `macos`. The
  platform split is expressed as `#[cfg(target_os = "linux")]` and `#[cfg(not(target_os = "linux"))]`
  pairs, so macOS currently falls into the "not Linux" arm of every decision.
- `crates/ss-vdisplay/src/display/` contains `linux/`, `windows.rs`, `windows_idd.rs`, and
  `backend.rs` (the `VirtualDisplay` trait). There is no macOS backend.
- `crates/ss-inject/src/lib.rs` defines `Backend::Unsupported` for
  `#[cfg(not(any(target_os = "linux", target_os = "windows")))]`, and `open()` bails with
  "no input-injection backend on this platform". That is what macOS gets today.
- `crates/slipstream-host/src/capture.rs` has **explicit** bail arms for
  `#[cfg(not(any(target_os = "linux", target_os = "windows")))]` on both
  `open_virtual_output_capture` and the desktop capturer. Even if a mac backend appeared in
  `ss-capture`, the host would refuse to call it until these arms change.
- `crates/slipstream-host/src/audio.rs` bails the same way for audio capture and the virtual
  mic ("no audio capture backend on this platform yet"). macOS sessions would be video-only.
- `crates/ss-encode/src/lib.rs` is closer than it looks. Its
  `#[cfg(not(target_os = "linux"))]` `open_video_backend` compiles on macOS, but its only
  hardware branch is `#[cfg(all(target_os = "windows", feature = "nvenc"))]`, so macOS resolves
  to `sw::OpenH264Encoder` and advertises H.264 only (`host_wire_caps` returns
  `CODEC_H264` on the non-Linux arm unless the Windows NVENC probe answers).
- `clients/apple` is a Swift Package plus Xcode project with `Sources`, `Tests`, and
  `Package.swift`. It is a client: decode, present, send input. It contains no capture, encode,
  or display code.

> Correction added after probing the Mac (see [§12](#12-mac-capability-facts-probed-2026-11-16)):
> a second checkout, `~/CodeProjects/slipstream-darwin-host`, already contains a
> `crates/ss-capture/src/platform/darwin/` scaffold and a `crates/ss-encode/src/backend/darwin/`
> directory. The statements above are true of **this** checkout (`spec`), which is the one the
> work has to land in, and false of the Mac's separate checkout.

The traits a Mac backend must satisfy already exist and are stable and well documented:

```rust
// crates/ss-encode/src/backend/codec.rs
pub trait Encoder: Send {
    fn submit(&mut self, frame: &CapturedFrame) -> Result<()>;
    fn submit_indexed(&mut self, frame: &CapturedFrame, wire_index: u32) -> Result<()> { ... }
    fn caps(&self) -> EncoderCaps { ... }
    fn request_keyframe(&mut self);
    fn poll(&mut self) -> Result<Option<EncodedFrame>>;
    fn flush(&mut self) -> Result<()>;
    // plus optional: reset, reconfigure_bitrate, applied_bitrate_bps,
    // set_pipelined, supports_chunked_poll, poll_chunk, set_input_ring_depth
}
```

`CapturedFrame` (`crates/ss-frame/src/core/frame.rs`) carries `width`, `height`, `pts_ns`,
`format: PixelFormat`, `payload: FramePayload`, `cursor: Option<CursorOverlay>`, and
`stage_ns`. `FramePayload` has `Cpu(Vec<u8>)` plus, on Linux only, `Cuda(DeviceBuffer)` and
`Dmabuf(DmabufFrame)`. A macOS payload variant is the obvious shape (`FramePayload::Cv` holding
an owned `CVPixelBuffer`), mirroring how `Cuda` was added.

`PixelFormat` already has `Bgra`, `Bgrx`, `Nv12`, and 10-bit variants, so the vocabulary does
not need to grow for the common cases.

`VirtualDisplay` (`crates/ss-vdisplay/src/display/backend.rs`) is
`create(&mut self, mode: Mode) -> Result<VirtualOutput>`, where `VirtualOutput` carries a
`node_id` (a PipeWire concept), `preferred_mode`, a `Box<dyn Send>` keepalive, and ownership
metadata. `node_id: u32` and `remote_fd` are Linux-shaped. A macOS backend would either have to
put a `CGDirectDisplayID` into `node_id` (ugly but workable) or the struct grows a mac field the
way it already grew `windows_head: Option<String>`. The Windows backend already chose the
second option, which is the precedent to follow.

`InputInjector` is a single method, `inject(&mut self, event: &InputEvent) -> Result<()>`, plus
`default_backend()`, `text_input_supported()`, `set_absolute_anchor()`. A `Backend::MacCgEvent`
variant fits cleanly.

So the shape is understood. The work is roughly four new platform lanes plus host wiring, not a
new architecture.

---

## 2. Screen capture: ScreenCaptureKit

### 2.1 The API we would use

ScreenCaptureKit (SCK) is the modern capture surface, introduced in **macOS 12.3**. The path:

1. `SCShareableContent.getExcludingDesktopWindows(_:onScreenWindowsOnly:completionHandler:)` or
   its async form, to enumerate `SCDisplay` and `SCWindow`.
2. Build an `SCContentFilter` with
   `init(display:excludingWindows:)` for a whole display, or
   `init(desktopIndependentWindow:)` for a single window, or
   `init(display:includingApplications:exceptingWindows:)` for an app-scoped capture.
3. Configure `SCStreamConfiguration`: `width`, `height`, `minimumFrameInterval` (a `CMTime`),
   `pixelFormat`, `showsCursor`, `capturesAudio`, `queueDepth`, `scalesToFit`.
   `captureResolution` (`SCCaptureResolutionType`: `.automatic`/`.best`/`.nominal`) is
   **macOS 15.0**, verified in the SDK headers (§12.2), not 14 as an earlier draft said.
   There is no `colorSpaceName` property in the ScreenCaptureKit headers of SDK 26.0, so any
   colour-space control lives somewhere I have not found (§12.2, §12.9).
4. Create `SCStream(filter:configuration:delegate:)` and add an output with
   `addStreamOutput(_:type:.screen sampleHandlerQueue:)`.
5. Implement `SCStreamOutput.stream(_:didOutputSampleBuffer:ofType:)` to receive
   `CMSampleBuffer`s, and `SCStreamDelegate.stream(_:didStopWithError:)` for teardown.

`SCDisplay.displayID` is a `CGDirectDisplayID`, which is the same identity space as
`CGGetActiveDisplayList`. That is the join key between "which head the host wants" and "which
SCK display to capture", and it is what makes both the dummy-HDMI and (if we ever use it)
`CGVirtualDisplay` routes workable.

### 2.2 Version floor

- SCK itself: **macOS 12.3**. Anything older has no supported path except the deprecated
  `CGDisplayStream`.
- The modern async `SCShareableContent` accessors and `SCContentSharingPicker`: macOS 14.0,
  confirmed from the SDK headers.
- Audio on a stream (`SCStreamOutputTypeAudio`): macOS 13.0, confirmed. Microphone type: 15.0.
- `SCStreamConfiguration.colorSpaceName`: **refuted.** No such symbol in the SDK 26.0
  ScreenCaptureKit headers.
- HDR capture properties named `captureDynamicRange`: **refuted as written.** No such symbol in
  the SDK headers, so the real HDR knob (if any) still has to be found.

Recommended floor: **macOS 14** to build against, **macOS 13** as the runtime floor if the
async API is avoided (the completion-handler form works from 12.3). Supporting 12.3 costs a
second code path and buys very little in 2026.

### 2.3 Permission model (Screen Recording TCC)

SCK capture requires the **Screen Recording** privacy permission. The relevant calls are
`CGPreflightScreenCaptureAccess()` and `CGRequestScreenCaptureAccess()` (both macOS 10.15+),
which query and request the same grant that SCK consumes.

Behaviour that matters:

- `CGRequestScreenCaptureAccess()` returns a Bool immediately. On a first request it shows the
  system prompt (or, depending on version, sends the user to System Settings). It does **not**
  block until the user decides.
- If the user denied, the function keeps returning false and the system will **not** show the
  prompt again. The only recovery is System Settings > Privacy & Security > Screen Recording,
  toggling the app on, and usually **quitting and relaunching** the app. macOS attaches the
  grant to the running process, so a live process often does not pick up a new grant.
- A denied SCK call surfaces as an `SCStreamError` / `SCStream` failure rather than a clean
  "permission denied" enum. Expect to classify by domain and code; the exact codes are
  unverified on current macOS.

The failure UX therefore cannot be a generic error toast. It must be:

1. On host start, and again before each first capture, evaluate
   `CGPreflightScreenCaptureAccess()`.
2. If false, do **not** start a session. Serve a management-API state
   `capture_permission: "denied"` with a human string.
3. In the tray/menu-bar app, present a pane that explains the toggle, with a button that opens
   `x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture`, plus a
   "I granted it, restart host" action that relaunches the agent.
4. In the management API, expose the same state so a headless console can show "Mac host needs
   Screen Recording permission" rather than a black stream.

The worst possible UX, and the one to design against, is a host that advertises itself, accepts
a connection, and streams black frames because capture silently produced nothing.

### 2.4 Why not CGDisplayStream

`CGDisplayStream` is Apple's older display-capture API. It is deprecated in favour of
ScreenCaptureKit (the current SDK marks it deprecated with "use ScreenCaptureKit instead"; the
exact deprecation version is unverified, I believe it landed in the macOS 13 or 14 SDK). Beyond
deprecation it is the wrong tool:

- It captures **whole displays only**. There is no per-window filter, so we cannot do
  window-scoped sessions, which our `VirtualDisplay`/capture vocabulary already models on other
  platforms.
- It has no HDR path and a weaker colour pipeline.
- It has no documented audio capture.
- Apple's investment, bug fixes, and the Screen Recording permission flow have all moved to SCK.
  Building a new host on a deprecated capture API is a maintenance trap with no upside.

Use SCK. Pay the 12.3 floor.

### 2.5 One behavioural trap that is not an API limitation

SCK is **change-driven**. It emits a sample buffer when the display content changes, not on a
fixed clock. On a static desktop, frames stop. Our host's frame-gap watchdog and the client's
loss logic interpret "no frames" as a dead link.

The design must therefore synthesise or tolerate idle frames: either configure the stream so it
ticks (a `minimumFrameInterval` is a floor, not a scheduler, so this is not guaranteed) or make
the host emit a keepalive and have the client treat "no new frame, stream healthy" as normal.
This is a real design decision with a correctness consequence on a static screen, and it is
cheap to get wrong.

---

## 3. Zero-copy and colour fidelity

### 3.1 How frames arrive

`SCStreamOutput` delivers `CMSampleBuffer`s whose image buffer is a `CVPixelBuffer` backed by
an `IOSurface`. That is the important fact: the pixels live in a GPU-shared surface, not in a
CPU buffer we own.

`pixelFormat` choices that matter:

| Requested format | FourCC | Notes |
|---|---|---|
| `kCVPixelFormatType_32BGRA` | `BGRA` | Default, matches `PixelFormat::Bgra`. Safest. |
| `kCVPixelFormatType_32BGRA` with `showsCursor` | `BGRA` | WindowServer composites the pointer into the surface. |
| `kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange` | `420v` | NV12, matches `PixelFormat::Nv12`, skips one conversion. |
| `kCVPixelFormatType_420YpCbCr8BiPlanarFullRange` | `420f` | NV12 full range. Chroma siting differs. |

### 3.2 Where the copies are

- **Inside WindowServer**: SCK composites the display (and the cursor, if asked) into a new
  IOSurface. That copy is unavoidable and not ours.
- **Capture to encode**: none, if we hand the `CVPixelBuffer` straight to VideoToolbox. VT's
  input is a pixel buffer, and it accepts BGRA or NV12 surfaces directly. Retain the buffer and
  pass it. This is the genuine zero-copy leg and it is better than the current Windows path,
  which uploads CPU frames into a D3D11 texture.
- **Unavoidable copy**: BGRA to NV12 chroma conversion if we insist on feeding VT packed BGRA.
  VT will do the conversion internally (no explicit copy in our code), but its scaler and chroma
  siting are not controllable and the quality is worse than letting it take NV12 from SCK
  directly. Recommendation: request `420v` from SCK when the negotiated session is 4:2:0, keep
  `BGRA` only for the cursor-blend path where we need to composite RGBA on CPU.

### 3.3 4:4:4

The `EncoderCaps.chroma_444` capability exists and NVENC sets it. VideoToolbox's HEVC encoder
historically does not expose a 4:4:4 (Range Extensions) profile on consumer hardware. I believe
this is still true on current macOS, but I have not confirmed it on a machine. Plan on
`chroma_444 = false` and `ChromaFormat::Yuv420` only, and treat any 4:4:4 support as a stretch
goal to be proven with a hardware test. Unverified, needs a Mac.

### 3.4 HDR

`PixelFormat` already carries 10-bit layouts (`Rgb10a2`, `P010`, `X2Rgb10`) and the NVENC path
uses them.

What is **verified** on macOS 26.0 (see §12.2, §12.9) is only this: the ScreenCaptureKit headers
contain no `colorSpaceName`, no `captureDynamicRange`, and no other colour-space or dynamic-range
property under any name I searched for. An earlier draft of this document invented those two
names; both are refuted. `SCCaptureResolutionType` exists at macOS 15.0 and is about resolution,
not HDR.

So the macOS HDR path is **unproven end to end**, not merely unimplemented. Before any HDR work,
someone has to find the real capture-side knob (or establish that there is none and that HDR
capture needs a different framework), then prove VideoToolbox takes a 10-bit surface and tags it
correctly. VideoToolbox HEVC Main10 exists as a profile; whether the whole chain carries BT.2020
and PQ from an SCK surface is untested.

Phase 1 and 2 are honest 8-bit SDR. The host must never label an SDR stream as HDR; the encode
crate already refuses to do that on Windows for exactly this reason.

### 3.4.1 Sleep is a hard requirement, not a nice-to-have

Measured on the operator's Mac (`pmset -g`, §12.7): `sleep 1`, i.e. one minute to system sleep,
currently held off only by seven `caffeinate` assertions and the running client processes.

A macOS host that does not hold its own power assertion will therefore sleep within a minute of
the last client disconnecting, which looks exactly like "the host crashed" to the next person
who tries to connect. This is a **hard requirement** and it is in the phase-1 definition of done
(§9): hold `IOPMAssertionCreateWithName(kIOPMAssertionTypeNoDisplaySleep, ...)` for the lifetime
of the host process (or at least for every live session plus a grace period), release it on
shutdown, and surface the assertion state in the management API so an operator can tell
"idle but awake" from "asleep".

### 3.5 Colour correctness risk

A Mac display may be P3, and SCK surfaces carry the display's colour space. If we feed those
pixels to VT and tag them as BT.709, saturated colours shift. If we do not tag them at all, the
client's decoder assumes BT.709 and the same shift happens. Getting this wrong produces the
classic "pink / neon / gamma wrong" stream that looks like a codec bug but is a colour tag bug.
Budget real time for a colour test card, and assert on decoded pixel values (see §8).

---

## 4. Encoding: VideoToolbox

### 4.1 The API we would use

```c
VTCompressionSessionCreate(allocator,
                           width, height,
                           kCMVideoCodecType_H264 /* or kCMVideoCodecType_HEVC */,
                           encoderSpecification,
                           sourceImageBufferAttributes,
                           NULL, outputCallback, refcon, &session);
```

`encoderSpecification` with `kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder`
set to true, and optionally
`kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder` for a hard requirement.
`VTCopyVideoEncoderList` enumerates what the machine actually has, with hardware and software
entries, which is the honest capability probe to feed `host_wire_caps`.

Per-session properties that map onto our latency policy:

| Our requirement | VideoToolbox property |
|---|---|
| Low-latency realtime | `kVTCompressionPropertyKey_RealTime = true` |
| No B-frames | `kVTCompressionPropertyKey_AllowFrameReordering = false` |
| Bound frame delay | `kVTCompressionPropertyKey_MaxFrameDelayCount = 1` |
| CBR-ish pacing | `kVTCompressionPropertyKey_AverageBitRate` + `kVTCompressionPropertyKey_DataRateLimits` (`[[bytes, seconds]]`) |
| Keyframe cadence | `kVTCompressionPropertyKey_MaxKeyFrameInterval` / `_MaxKeyFrameIntervalDuration` |
| Codec profile | `kVTCompressionPropertyKey_ProfileLevel` (`..._H264_High_AutoLevel`, `..._HEVC_Main_AutoLevel`) |
| Expected rate | `kVTCompressionPropertyKey_ExpectedFrameRate` |
| Power/thermal bias | `kVTCompressionPropertyKey_MaximizePowerEfficiency` (set false for streaming) |
| Force an IDR | `kVTEncodeFrameOptionKey_ForceKeyFrame` on the frame, not the session |

Encode with `VTCompressionSessionEncodeFrame(session, pixelBuffer, pts, duration, frameProps,
sourceFrameRefCon, &flagsOut)`. `VTSessionSetProperty` is also the path for
`reconfigure_bitrate`, since it may be legal to change `AverageBitRate` on a live session.
Unverified whether a live rate change takes effect without a keyframe.

Output arrives on the callback as a `CMSampleBuffer` whose data is a `CMBlockBuffer`.

### 4.2 The bitstream detail that bites

VideoToolbox emits **length-prefixed (AVCC/HVCC)** NAL units, with parameter sets carried in the
`CMFormatDescription`, not repeated in-band on every IDR.

Our wire contract, per `docs/embedding-the-c-abi.md`, is **Annex-B access units with in-band
parameter sets**, first AU is an IDR, and clients build their decoder from that AU. So a
VideoToolbox backend must:

1. Read VPS/SPS/PPS via `CMVideoFormatDescriptionGetHEVCParameterSetAtIndex` (or the H.264
   equivalent).
2. Rewrite 4-byte length prefixes to `00 00 00 01` start codes over every NAL in the sample.
3. Prepend the parameter sets on every IDR (and ideally on the first AU after a rebuild).

This is a few hundred lines and it is the single most common cause of "the client shows black
or a single frame". It also has a per-frame cost (a walk plus a memcpy of the AU). Worth doing
carefully, with unit tests over recorded real-world samples, and worth treating as its own
work item rather than part of "wire up VT".

### 4.3 What we lose versus NVENC

- **No reference-frame invalidation.** VideoToolbox exposes no RFI API. So
  `EncoderCaps.supports_rfi = false`, and every loss recovery becomes a full IDR through
  `request_keyframe`. The host already handles that (`invalidate_ref_frames` returning false
  falls back to a keyframe), so this is a recovery-quality regression, not a correctness break.
- **No intra-refresh wave** (`intra_refresh = false`).
- **No sub-frame / slice readback** (`subframe_output = false`), so `poll_chunk` degrades to
  whole-AU chunks, exactly like the libav and software backends.
- **No cursor blending** (`blends_cursor = false`). If a session negotiates the cursor channel,
  the host must composite the `CursorOverlay` on CPU before submitting, or ask SCK for an
  embedded cursor and forgo the channel. The existing negotiation already handles a
  non-blending backend by embedding the pointer, so this is a supported shape, just with the
  associated cost.

This is the same capability profile as the software backend, reached with hardware speed. That
is a fine trade for v1 and it should be stated in the UI/docs rather than discovered by a user
noticing that packet loss recovers worse on a Mac.

### 4.4 Hardware versus software

- H.264 hardware encode: present on essentially every Mac that can run macOS 12.3 (Intel Quick
  Sync in the media engine, Apple Silicon media engine). Safe to require.
- HEVC hardware encode: present on Apple Silicon and on Intel 6th generation and later with the
  media engine. Not universal on very old Intel. Must be probed via `VTCopyVideoEncoderList`,
  and the advertisement must be built from the probe, never from a static superset. This is the
  same failure the Linux code documents at length: advertising HEVC on a chip that cannot encode
  it gives the client fifteen seconds of blank video and a disconnect.
- AV1 encode: I do not believe VideoToolbox exposes AV1 encode on any shipping Mac. Unverified,
  needs a Mac, but plan on "no AV1" and advertise H.264 | HEVC only.

Software fallback: VT will fall back to a software encoder if hardware is not required. That is
useful for CI (a runner without a media engine still encodes) but it is not a streaming path.

### 4.5 Realtime session versus file session

A file-encode session (`RealTime = false`, reordering allowed, large VBV, multi-pass quality
optimisation) is a different configuration with different buffering. Ours must be the realtime
shape: `RealTime = true`, `AllowFrameReordering = false`, `MaxFrameDelayCount = 1`, a VBV of
about one frame interval (`bitrate / fps`, matching the house `SLIPSTREAM_VBV_FRAMES = 1.0`
default), and no quality tuning that adds latency. Even so, VT buffers a small number of frames
internally for pipelining; `poll()` may legitimately return `None` for a frame or two. The
`Encoder` contract already permits that ("`poll` may return `None` while an encode is in
flight"), so the host loop needs no change.

---

## 5. Virtual displays: the hard part

This is where the honest answer is "macOS does not have this".

Windows has Indirect Display Driver (IDD), which is a supported kernel-level mechanism for
creating a display that is not backed by hardware. Our Windows backend uses it. **macOS has no
equivalent.** There is no public, supported way for a user-space process to create a display
that the window server treats as a real monitor.

The candidate routes, and what each actually is:

### 5.1 `CGVirtualDisplay` (private CoreGraphics API)

`CGVirtualDisplay`, `CGVirtualDisplayDescriptor`, `CGVirtualDisplaySettings`, and
`CGVirtualDisplayMode` are classes inside CoreGraphics. They are **not in the public SDK
headers**. They are discovered by class-dump and are what third-party tools use to create
virtual monitors (BetterDisplay, and several of the display-link and "add a HiDPI mode" tools).

What this means in product terms, stated plainly:

- Using it means calling an undocumented private API from a shipping product.
- **App Store: effectively impossible.** Private API use is a documented rejection reason.
- **OS-update risk: real and permanent.** The class layout, the initialiser selectors, and the
  property names can change in any macOS release. A change typically manifests as a crash or,
  worse, as a display that appears and then is not capturable. When this breaks it breaks at
  the worst possible time: after the operator has already shipped and after an OS update the
  user chose to install.
- There is no entitlement to request. There is no support channel. There is no deprecation
  notice. It is not "grey area", it is unsupported.
- The display is owned by the creating process. If the host crashes, the display disappears,
  which is at least a clean failure mode.

If we use it: put it behind an explicit opt-in flag, isolate the calls in a small helper
process so a bad class crashes the helper and not the host, and be honest in the UI that this
feature may stop working after a macOS update. It is viable for a directly distributed,
self-hosted product. It is not viable for the App Store.

### 5.2 A window that pretends to be a display

Create a borderless window at the client's mode and call it a display. This does not work as a
product:

- It is not a display, so it cannot receive a full-screen game or a launched app's window.
- It has no monitor identity, so there is no per-display resolution, DPI, or arrangement
  semantics.
- WindowServer treats it as a window on some existing display, so the client sees that window
  plus whatever else is on the desktop, not an independent desktop.
- There is no focus or Space semantics that a game would respect.

Verdict: a demo trick. Do not ship it.

### 5.3 Sidecar and Continuity

Sidecar makes a Mac an extended display **for an iPad**. The direction is wrong: it does not
create a display that arbitrary remote clients can drive, and there is no public API to use it
programmatically. Universal Control is Apple-to-Apple and has no third-party surface.

Verdict: not usable.

### 5.4 A physical dummy HDMI plug

A $10 to $30 HDMI EDID emulator ("headless dongle", "display emulator"). macOS sees a real
display attached to real hardware. Everything downstream is ordinary, supported capture:
`SCDisplay` appears, we capture its `CGDirectDisplayID`, input lands on it, full-screen apps
work, Spaces work.

Costs and limits:

- It is hardware. One dongle per virtual display. A laptop user has to carry and plug one in.
- The mode set is fixed by the dongle's EDID unless the dongle supports programmable modes.
  A 1080p60 dongle cannot serve a 1440p client. Some dongles can be reprogrammed over USB,
  which is another integration surface.
- It occupies a port, and on Apple Silicon laptops it competes with the single external display
  port if one is already in use.
- On a Mac mini or a headless desktop it is the obvious, slightly ugly answer, and it works.

Verdict: **the only route I can find that is shippable today without private API.** This should
be the v1 virtual-display story: "plug in the dongle, the host will use it".

### 5.5 No virtual display at all

Capture the primary display and stream it. This is "remote control my Mac", which is what most
people actually mean. It needs no new API, no hardware, and no private surface.

Verdict: **phase 1 must be this.** It is the smallest thing that produces a real feature.

### 5.6 A driver-extension display

DriverKit has a HID family and a networking family. It has no display family. A virtual display
kext is a kernel extension, which modern macOS requires to be notarized, user-approved, and
which Apple has repeatedly signalled is on the way out. A Mac App Store product cannot ship one
at all.

Verdict: not an option.

### 5.7 Summary table

| Option | Works today | Shippable | Verdict |
|---|---|---|---|
| No virtual display (mirror primary) | Yes | Yes | Phase 1 |
| Physical HDMI dongle | Yes | Yes, with hardware | v1 supported path |
| `CGVirtualDisplay` (private) | Yes, today | Direct distribution only, fragile | Opt-in, flagged, isolated |
| Window-as-display | Partially | No | Hack, do not ship |
| Sidecar / Continuity | No (wrong direction) | No | Not applicable |
| DriverKit / kext display | No | No | Not an option |

**Answer to "are virtual displays viable at all":** not through a supported API. A physical
dongle is viable. `CGVirtualDisplay` is viable in the narrow sense that it works today and is
what the rest of the ecosystem uses, but it carries a permanent, uninsurable breakage risk and
rules out the App Store. There is nothing else.

---

## 6. Input injection

### 6.1 The API we would use

- Keyboard: `CGEventCreateKeyboardEvent(source, virtualKeyCode, keyDown)` then
  `CGEventPost(kCGHIDEventTap, event)`.
- Text: `CGEventKeyboardSetUnicodeString` on a keyboard event types committed text
  independent of layout, which maps onto our `HOST_CAP_TEXT_INPUT` capability.
- Mouse: `CGEventCreateMouseEvent` with `kCGEventMouseMoved`, `kCGEventLeftMouseDown`,
  `kCGEventLeftMouseDragged`, `kCGEventRightMouseDown`, `kCGEventOtherMouseDown` (with the
  button number in `mouseEventButtonNumber`), plus `CGEventCreateScrollWheelEvent`.
- Absolute positioning: `CGWarpMouseCursorPosition(point)`.
- Suppressing the synthetic-delta spike after a warp:
  `CGAssociateMouseAndMouseCursorPosition(false)` / `(true)` around it.

Note the API shape: macOS has **no absolute pointer event**. The pointer is a cursor at a
global coordinate. Absolute client coordinates therefore become `CGWarpMouseCursorPosition`
followed by (optionally) a synthetic move so apps that read deltas see motion, and relative
deltas become a mouse-moved event with `kCGMouseEventDeltaX` / `DeltaY` set. Both are
approximations of what the Windows `SendInput` and Linux libei paths do exactly.

### 6.2 Permissions

Posting events requires the **Accessibility** TCC grant (`kTCCServiceAccessibility`). Check with
`AXIsProcessTrustedWithOptions([kAXTrustedCheckOptionPrompt: true])`.

The critical property: **without the grant, event posting silently does nothing.** It does not
throw, it does not return an error. The stream works, the client sees the desktop, and clicks
and keystrokes are simply dropped. This is the worst failure mode in the whole project, and the
UI has to handle it: probe Accessibility at host start and before a session, expose it in the
management API, and refuse to advertise an input-capable session when it is false.

Reading events (needed for host cursor hiding and global hotkeys) is a separate grant, **Input
Monitoring** (`kTCCServiceListenToEventTap`). Do not conflate them.

### 6.3 Absolute versus relative mouse, and games

Relative mouse is the known pain point and it should not be softened.

- Our protocol sends relative deltas (`MOUSE_MOVE`) and absolute moves (`MOUSE_MOVE_ABS`).
- Desktop apps, the Finder, browsers, and most UI respond correctly to posted events.
- Games with mouse-look commonly call `CGAssociateMouseAndMouseCursorPosition(false)` and read
  raw deltas from `IOHIDSystem`. Posted `CGEvent`s are not raw HID, so the game either ignores
  them or applies them inconsistently, and the cursor can drift or stick. This is well
  documented in the community as the reason macOS game streaming and remote-play tools struggle
  with look control.
- The only robust fix is a **virtual HID device** (DriverKit, `HIDDriverKit`), which is a system
  extension with user approval, notarization, and its own entitlement. That is a separate
  multi-week project and it is the same class of work as our Linux `uinput` virtual pads.

Verdict: **desktop remote control works. Game mouse-look does not.** That is a positioning
decision, not a bug to be fixed later by tuning.

### 6.4 Gamepads

- Reading a physical controller: `GameController` framework (`GCController`), straightforward.
- Synthesising a controller that other apps see: no public API. `GCVirtualController` (macOS 12+)
  appears to be an on-screen controller for the creating app, not a system-visible HID device.
  Unverified whether any other app can see it, but I would not bet the feature on it.
- The supported route for a system-visible virtual pad is a DriverKit `HIDDriverKit` system
  extension, the same work item as the relative-mouse fix. Note that our Linux side had to use
  UHID, and Windows had to use ViGEmBus, for exactly this reason: every OS needs a driver-level
  virtual device, and macOS is no exception.

Verdict: phase 1 ships no gamepad. Treat virtual HID (gamepad plus proper relative mouse) as a
distinct, later, large project.

### 6.5 Injecting into a specific display

You cannot. `CGEventPost` targets the session's active event context, and the pointer lives at
global coordinates. There is no "post this event into display N". Consequently:

- Absolute coordinates must be mapped from the client's video rectangle into the global display
  coordinate rectangle of the target display.
- Events land on whichever **Space** is active. If the streamed display is showing a different
  Space than the user is on, or if a full-screen game is on another Space, posted events go
  somewhere the user is not looking. Multi-display + Spaces is a genuine limitation.
- This is exactly why our `set_absolute_anchor` exists on the injector side: the host must know
  which display rectangle the session's coordinates belong to. Reuse that mechanism.

---

## 7. Session and permissions model

### 7.1 launchd: daemon versus agent

A Mac host must be a **per-user LaunchAgent**, not a root LaunchDaemon.

- A LaunchDaemon runs as root, outside any GUI login session. TCC grants for Screen Recording
  and Accessibility are attached to a user session and a code signature, so a daemon cannot hold
  them, and ScreenCaptureKit cannot capture a desktop that the daemon cannot see.
- A LaunchAgent runs in the user's GUI session (Aqua), can hold TCC grants, and can drive SCK,
  CGEvent, and the menu-bar UI.

Practical shape: a menu-bar app plus a LaunchAgent that keeps the host process alive, or a
single agent that owns both the host and its status item. The tray crate
(`crates/slipstream-tray`) is the precedent for the UI, but it is Windows-shaped and would need
a macOS status-item path.

This is structurally the same limitation we already hit on Windows: the SCM service
(`crates/slipstream-host/src/service.rs`) runs in session 0, has no interactive desktop, and
cannot capture, so the streaming work has to happen in the interactive user process. macOS
makes that limitation a hard law rather than an incidental one.

### 7.2 Login session required

There is no way to serve the macOS **login window** or a locked screen through ScreenCaptureKit
from a user agent. Consequences to state up front:

- The Mac host only serves while a user is logged in and the session is unlocked.
- If the Mac is asleep, nothing is captured. The host **must** hold
  `IOPMAssertionCreateWithName(kIOPMAssertionTypeNoDisplaySleep, ...)` for its own lifetime: the
  measured `pmset` setting is `sleep 1` (one minute), and the box is only awake today because
  `caffeinate` and the current clients are holding it (see §3.4.1 and §12.7). This is the macOS
  analogue of `crates/slipstream-host/src/sleep_inhibit.rs` and it is load-bearing, not cosmetic.
- If the Mac is locked (screen lock, FileVault, fast user switching), capture stops or returns
  black. Unverified exactly which, needs a Mac, but either way it is a hard stop for the
  session and the management API should say "host locked" rather than "stream broken".
- "Remote unlock" is out of scope. It is Apple's own Screen Sharing / VNC territory, and the
  security model around it (SecureToken, FileVault, `fdesetup`) is not something to reimplement.

### 7.3 TCC and code signature

TCC keys grants on the bundle identifier and the code signature. A rebuild with a different
signature is a different app as far as TCC is concerned, so it re-prompts or silently loses the
grant. This has two consequences:

- **Developer loop tax.** Ad-hoc signed debug builds will lose Screen Recording and
  Accessibility on every rebuild. Use a stable Developer ID signature (or at least a stable
  ad-hoc identity plus consistent bundle id) from the first day, or every test cycle starts with
  two System Settings visits.
- **CI is hard.** CI cannot grant TCC. `tccutil` can only reset, not grant. A CI machine needs a
  pre-granted, signed helper and an auto-logged-in user. That means the operator's Mac or a
  dedicated Mac mini runner, not a fresh cloud VM.

### 7.4 Management API and pairing

These are platform-neutral and should not need redesign: the management API lives in
`crates/slipstream-host/src/mgmt/`, pairing in `native_pairing` plus `slipstream-core`'s SPAKE2
PIN ceremony. macOS work here is limited to:

- `ss-paths` needs a macOS branch (`~/Library/Application Support/...`, `~/Library/Logs/...`).
- Identity persistence should use the Keychain rather than a bare file.
- Logging should go to unified logging (`os_log`) as well as a file, because launchd agents
  have no terminal and stderr goes nowhere by default.
- The management API should grow a `platform: "macos"` and a permissions block
  (`screen_recording`, `accessibility`, `login_session`) so the console can explain a broken
  host.

---

## 8. Build and CI reality

### 8.1 Can this be built on the Windows desk?

No.

- Rust can target `aarch64-apple-darwin` from a non-Mac host only with the Apple SDK, which is
  licensed for Apple hardware and is not distributable. The linker (`ld64`/`clang`) and the
  frameworks come from Xcode or the Command Line Tools.
- SCK and VT are Objective-C frameworks. The practical bindings are the `objc2` family
  (`objc2`, `objc2-foundation`, `objc2-core-media`, `objc2-core-video`,
  `objc2-video-toolbox`, `objc2-screen-capture-kit`, subject to whether that last one exists
  and is current). If a framework's bindings are missing or incomplete, a small
  Swift or Objective-C shim compiled by `build.rs` is the fallback. Either way, the SDK must be
  present at compile time, so the build must happen on macOS.
- Signing and notarization (`codesign`, `notarytool`) are macOS tools.

So: the Mac in the loop is not optional, and not just for testing. It is required to compile.

### 8.2 What the Mac side needs

- macOS 14 or later to build (15 recommended for the current SDK), 13 as a runtime floor.
- Xcode 15+ or the Command Line Tools, plus `xcodebuild` for the packaging step.
- Rust via rustup, with both `aarch64-apple-darwin` and `x86_64-apple-darwin` if we ship
  universal.
- A Developer ID Application certificate for TCC persistence and distribution, plus
  notarization credentials for a shipped build.
- For CI: a self-hosted runner (Mac mini or MacStadium) with auto-login enabled and TCC
  pre-granted to a signed helper. GitHub's hosted `macos-14`/`macos-15` runners can compile and
  run unit tests but cannot grant Screen Recording, so they cannot prove live capture.

### 8.3 What CI can and cannot prove

Can be proven on a hosted macOS runner, no human:

- The crates compile and link against the macOS SDK.
- The Annex-B conversion is correct: feed known AVCC samples, assert start codes and parameter
  sets, compare against a reference.
- The VideoToolbox encoder works on a **synthetic** pixel buffer (a generated test pattern), with
  no capture at all. Encode, then decode with `VTDecompressionSession`, compare pixel values.
  This validates the whole encode path including colour tags.
- The `Encoder` trait contract: `submit` then `poll` yields AUs, `caps()` is honest, a forced
  keyframe produces an IDR.

Cannot be proven on a hosted runner:

- That ScreenCaptureKit captures a real screen. That needs a TCC grant and a GUI session.

### 8.4 Proving capture without a human watching

The trick is to make the assertion about pixels, not about a screenshot. A fully automated
loop on a self-hosted Mac:

1. Launch a tiny signed test app that draws a known pattern (a solid colour that changes every
   second, plus a frame counter) in a window at a known position.
2. Capture that window (or the whole display) with SCK.
3. Encode with VT.
4. Decode with `VTDecompressionSession` back to a pixel buffer.
5. Assert: at second N the average colour equals the colour the test app drew, and the decoded
   frame counter is monotonic and advancing at roughly the requested rate.

If that passes, capture works. It is a closed loop with no human in it, and it also catches
colour-tag bugs, scaling bugs, and stale-frame bugs, which a "I can see my desktop" check does
not.

Add a soak artifact: JSON lines per second with capture fps, encoded fps, average encode ms,
dropped frames. CI asserts fps within 5 percent of requested for 60 seconds and no gap over
250 ms.

---

## 9. Milestones

### Phase 0: prove capture and encode exist (spike, 1 to 2 days)

Throwaway binary: SCK capture of the primary display, VT H.264 encode, write Annex-B to a file.

- Provable: `ffprobe` reports the expected resolution, frame rate, and codec, and `ffmpeg -v
  error` decodes the file with no errors. The file plays.
- Can go wrong: TCC prompt does not appear for an unsigned binary. VT refuses because the pixel
  format is unsupported. The Annex-B rewrite is wrong and playback is black.

### Phase 1: a real host, whole-display, SDR (2 to 4 weeks)

This is the definition of done, and it is the only phase that should be committed to before the
virtual-display question is answered.

Work items:

- `ss-frame`: add a macOS `FramePayload` variant (owned `CVPixelBuffer`), gated on
  `cfg(target_os = "macos")`.
- `ss-capture`: `src/platform/macos/sck.rs` implementing `Capturer`, plus the `mod.rs` arm and
  the `not(linux)` host bails in `crates/slipstream-host/src/capture.rs`.
- `ss-encode`: a `backend/macos/videotoolbox.rs` `Encoder` plus the Annex-B conversion, plus a
  macOS `host_wire_caps` arm that probes `VTCopyVideoEncoderList` and advertises H.264 | HEVC
  only when the machine can encode them.
- `ss-inject`: `Backend::MacCgEvent`, `default_backend()`, `text_input_supported() == true`,
  and Accessibility preflight.
- `slipstream-host`: macOS capture arms, a permission state in the management API, a macOS
  sleep-inhibit, a launchd plist, and `ss-paths` support.
- Audio: either defer (video-only, which the host already degrades to) or add SCK
  `capturesAudio` (macOS 13+), which avoids needing a virtual audio device entirely. That is
  nicer than the Windows WASAPI loopback story and is worth doing in phase 1 if time allows.

**Phase 1 definition of done**, all checkable:

1. An existing first-party client (the Apple client, or the Windows client) connects to the Mac
   host and shows the live desktop at 1080p60, SDR, H.264 or HEVC.
2. Host-side stats prove encode plus send under 80 ms at 1080p60, and the client reports no
   sustained gap over 250 ms during a 10-minute soak.
3. Typing on the client appears in a TextEdit window on the Mac. A mouse click opens a menu.
   Both verified from the client, not by watching the Mac.
4. `cargo test -p ss-capture -p ss-encode -p ss-inject` is green on macOS, including the
   synthetic-pixel encode test and the Annex-B tests.
5. An artifact is attached: the `.h264`/`.hevc` file plus the JSON stats, both produced by the
   automated harness in §8.4.
6. The management API reports `platform: "macos"` with a permissions block, and a host with
   Screen Recording denied refuses the session with a human-readable reason instead of
   streaming black.
7. **The host survives idle.** With no client connected for ten minutes, the host is still
   answerable on its management API and a fresh connection succeeds without a human waking the
   machine. This requires the host to hold its own `IOPMAssertion` (§3.4.1): the measured
   `pmset` setting is one minute to sleep, so without it the host dies the moment the current
   clients disconnect. This item is not optional and not cosmetic.

What can go wrong: TCC never grants for the dev signature. HEVC open fails on an old Intel Mac
(falls back to H.264, acceptable). Colour is wrong because of a P3 to BT.709 mismatch. Retina
scaling gives a 2x point-to-pixel mismatch. A static screen starves the encoder (see §2.5) and
the client declares the link dead. All of these are solvable, none is architectural.

### Phase 2: virtual display (the fork in the road)

**2a, HDMI dongle (3 to 5 days).** Treat the dongle as the display source: enumerate `SCDisplay`
by `CGDirectDisplayID`, capture it, assign the client's mode (or clamp to the dongle's EDID),
route input there, and make "add a display" a user action that says "plug the dongle in".

- Provable: `system_profiler SPDisplaysDataType` lists the head, SCK enumerates it, capture of
  it succeeds, input lands on it, and a full-screen app launched into it is visible to the
  client.
- Can go wrong: dongle EDID fixes the mode below what the client wants. The dongle is not
  detected on a hub. The client asks for a resolution the dongle will not do.

**2b, `CGVirtualDisplay` (1 to 2 weeks, optional, opt-in).** Isolate the private calls in a
helper process, gate behind a flag, warn in the UI, and accept the permanent risk.

- Provable: `CGGetActiveDisplayList` grows by one, `SCDisplay` appears, an app can be moved to
  it, and it is capturable.
- Can go wrong: the class was renamed or restructured in the current OS. The display appears but
  SCK will not capture it (a known caveat class for virtual displays on some versions,
  unverified for the current one). It works until an OS update, then silently breaks.

### Phase 3: parity and polish (3 to 6 weeks, scope-dependent)

- HDR, if the 15+ capture properties and Main10 encode hold up on real hardware.
- 4:4:4, very likely not possible, verify before promising.
- Live bitrate reconfiguration, cursor channel (CPU composite), multi-display.
- Audio, if deferred from phase 1.
- Gamepad and proper relative mouse, which is the DriverKit virtual-HID project and should be
  scoped separately. Expect 4 to 8 weeks on its own.

---

## 10. Effort and risk

### Sizes

| Phase | Estimate | Confidence |
|---|---|---|
| 0 | 1 to 2 days | High, assuming a Mac with Xcode |
| 1 | 2 to 4 weeks, plus 3 to 5 days of signing and CI friction | Medium |
| 2a (dongle) | 3 to 5 days | High |
| 2b (private API) | 1 to 2 weeks, plus permanent maintenance | Low, by nature |
| 3 | 3 to 6 weeks, depending on HDR and audio | Low |
| Gamepad / relative mouse (DriverKit) | 4 to 8 weeks, separate project | Low |

The build, signing, and CI friction is not padding. Every macOS project I have seen
underestimates it, because TCC re-prompts on signature change and launchd gives no terminal.

### Top three risks, ranked

1. **Virtual display.** There is no supported API. The dongle works but is hardware and fixed
   mode. The private API works today but can vanish and rules out the App Store. This decides
   whether "macOS host" is a feature or a hack, and it is the one thing that cannot be
   engineered around. Resolve it (decide dongle versus private API) before committing to
   anything beyond phase 1.
2. **Permissions, signature, and session coupling.** Screen Recording and Accessibility are
   per-user-session and per-code-signature. A headless, unsigned, or daemon-hosted build
   silently captures nothing or injects nothing, with no error. This will consume days and it
   makes CI expensive (a self-hosted Mac with a pre-granted signed helper).
3. **Relative mouse for games.** Without a DriverKit virtual HID, the Mac host is a remote
   desktop product, not a game-streaming host. That is a product-positioning decision, and it
   should be made explicitly rather than discovered after phase 1 ships and a user tries to play
   a shooter.

### What I would tell the operator before they commit

This is worth doing, but it is worth doing as a **remote desktop host for a logged-in Mac**,
not as a game-streaming peer to the Linux and Windows hosts. Capture, encode, and absolute
input are all straightforward and the zero-copy story on macOS is actually better than on
Windows. The two things that make it hard are not in the code: a virtual display needs either
a $10 dongle or a private API Apple can break, and game-grade relative mouse needs a DriverKit
extension that is its own multi-week project. Budget four to six weeks to a credible host with
the dongle path, and do not commit to the private API until you have decided you are fine
shipping something that does not go on the App Store. The Mac in CI is a hard prerequisite, not
a nice-to-have, because TCC cannot be granted remotely.

---

## 11. Unverified ledger

Everything below is a claim I could not confirm from this Windows desk. Each needs a real Mac.

| Item | What needs confirming |
|---|---|
| Exact `CGDisplayStream` deprecation version | Which macOS SDK marked it deprecated |
| `SCStreamConfiguration.colorSpaceName` availability | 14.0 versus 13.x |
| HDR capture properties (`captureDynamicRange`) | Name and availability, believed 15.0 |
| SCK behaviour when Screen Recording is denied | Which error domain and codes surface |
| SCK on a locked screen | Black frames, an error, or a stopped stream |
| Whether `CGVirtualDisplay` works on the current macOS | Class shape and whether SCK captures its display |
| VideoToolbox HEVC 4:4:4 encode | Whether any shipping Mac exposes Range Extensions |
| VideoToolbox AV1 encode | Believed absent, needs confirmation |
| Live `AverageBitRate` change on an open VT session | Whether it applies without a keyframe |
| `GCVirtualController` visibility | Whether other apps can see a controller it creates |
| Whether any supported virtual-display API was added recently | Search the current SDK headers |
| SCK frame emission on a static desktop | Whether it ticks or genuinely stops |
| HEVC hardware encode on Intel Macs before 6th gen | Probe results per machine class |
| `objc2-screen-capture-kit` binding completeness | Whether a Swift/ObjC shim is needed |

---

## 12. Mac capability facts (probed 2026-11-16)

Read-only SSH over Tailscale (`kooshapari@100.112.14.98`, Tailscale `100.112.14.98`). No
installs, no permission changes, no dialogs opened, no System Settings, no remote session
started. Output below is literal, trimmed only where noted.

### 12.1 Host identity and version

```
ProductName:		macOS
ProductVersion:		27.0
BuildVersion:		26A5353q
arm64
```

macOS 27.0 on Apple Silicon, SDK `MacOSX26.0.sdk`. **The ScreenCaptureKit floor (12.3) is
cleared by fourteen major versions.** Every API in §2 is present on this machine except the ones
the SDK says arrived later (12.2 below).

### 12.2 Toolchain: present and current

```
-- xcode-select -p
/Applications/Xcode.app/Contents/Developer
rc=0
-- xcodebuild -version
Xcode 26.0
rc=0
-- xcrun sdk   (--show-sdk-version)
26.0
rc=0
-- swift --version
swift-driver version: 1.127.14.1 Apple Swift version 6.2 (swiftlang-6.2.0.19.9 clang-1700.3.19.1)
Target: arm64-apple-macosx28.0
rc=0
-- clang --version
Apple clang version 17.0.0 (clang-1700.3.19.1)
Target: arm64-apple-darwin27.0.0
rc=0
```

A full Xcode (not just Command Line Tools), Swift 6.2, clang 17. **This machine can compile a
Rust macOS host**, which is the single biggest prerequisite and the reason the "build cannot
happen on the Windows desk" finding is now an inconvenience rather than a blocker.

Version-floor facts read directly out of the SDK headers:

```
SCStream / SCShareableContent : API_AVAILABLE(macos(12.3), macCatalyst(18.2))
SCStreamOutputTypeAudio       : API_AVAILABLE(macos(13.0))
SCStreamOutputTypeMicrophone  : API_AVAILABLE(macos(15.0))
SCContentSharingPicker        : API_AVAILABLE(macos(14.0))
SCCaptureResolutionType       : (block) API_AVAILABLE(macos(15.0))
includeMenuBar                : API_AVAILABLE(macos(14.2))
```

So: SCK is 12.3 (confirmed), audio capture on a stream is 13.0 (confirmed), the picker is 14.0
(confirmed), `captureResolution` is **15.0, not 14** (the document guessed 14 and was wrong).
`SCStreamConfiguration.h` does not exist as a separate header in this SDK, and neither
`colorSpaceName` nor any `captureDynamicRange`/HDR property matched anywhere in
`ScreenCaptureKit.framework/Headers`. Treat §2's and §3.4's HDR and colour-space property names
as **unconfirmed and probably wrong**; do not design around them until the exact symbols are
found in the current SDK.

### 12.3 The Apple client builds on this machine today

```
$ cd ~/CodeProjects/slipstream/clients/apple && swift build -c debug
[0/1] Planning build
Building for debugging...
[0/3] Write swift-version--58304C5D6DBC2206.txt
Build complete! (0.36s)
exit code 0
```

Source lives at `~/CodeProjects/slipstream/clients/apple` (plus `App`, `Config`,
`SlipstreamCore.xcframework`, `Slipstream.xcodeproj`, `Sources`, `Tests`, `tools`). The build
completed from the package cache in 0.36 s, so this proves the tree is installed and consistent,
not that a clean build is fast. Nothing was installed to make it work.

### 12.4 A second checkout already exists, with Darwin scaffolds

`ls ~/CodeProjects` shows three entries: `slipstream`, `slipstream-darwin-host`, `temp`.
`~/CodeProjects/slipstream-darwin-host` is a full Rust workspace (Cargo.toml, `crates`, `api`,
`clients`, `ci`, `compliance`) and it is **not** the same as this `spec` checkout:

```
-- macos platform dirs
crates/ss-capture/src/platform/darwin
crates/ss-capture/src/platform/linux
crates/ss-capture/src/platform/mod.rs
crates/ss-capture/src/platform/windows

-- mod.rs tail
/// macOS: ScreenCaptureKit. Scaffold — see `darwin/mod.rs`.
#[cfg(target_os = "macos")]
pub mod darwin;

-- crates/ss-capture/src/platform/darwin
ffi.rs   mod.rs   mode.rs      (all 2026-11-16 03:00, ~4-5 KB each)

-- ss-encode
crates/ss-encode/src/backend/darwin/          (exists)
crates/ss-encode/src/backend/macos/           (does not exist)
#[cfg(target_os = "macos")]                   lib.rs:1840   (one occurrence)
```

So some phase-1 scaffolding for capture and encode already exists on the Mac, self-labelled
"Scaffold". Nothing yet for `ss-vdisplay` (no darwin backend: `admission.rs`, `backend.rs`,
`identity.rs`, `layout.rs`, `lifecycle.rs`, `linux/`, `mirror.rs`, `monitors.rs`, `policy.rs`,
`proc.rs`, `registry.rs`, `routing.rs`, `session.rs`, `windows.rs`, `windows_idd.rs`), nothing
for `ss-inject` (`hidout_dedup.rs`, `keymap.rs`, `linux/`, `pad_gate.rs`, `pad_slots.rs`,
`proto/`, `service.rs`, `uhid_manager.rs`, `windows/`), and no macOS payload in
`ss-frame/src/core/frame.rs` (zero matches for `macos`).

This changes the plan in one concrete way: **phase 1 is not a from-scratch start.** It is
"finish and verify the darwin scaffold, and port it into `spec`", which is materially cheaper
than the estimate in §10 for the capture and encode slices. It does not change the
virtual-display or input findings at all.

### 12.5 GPU and encoder reality

```
    Apple M1 Pro:
      Chipset Model: Apple M1 Pro
      Type: GPU
      Bus: Built-In
      Total Number of Cores: 16
      Vendor: Apple (0x106b)
      Metal Support: Metal 4
```

Accelerator class present in the IORegistry (`AGXAcceleratorG13`, `AGXAccelerator`,
`AGXAcceleratorG13X`). M1 Pro is an Apple Silicon SoC with the Media Engine.

- H.264 hardware encode: **inferred, not confirmed.** Apple Silicon media engines have encoded
  H.264 since M1. I did not call `VTCopyVideoEncoderList`, so this is chip-derived, not observed.
- HEVC hardware encode: **inferred, not confirmed.** Same reasoning. M1 Pro has Media Engine
  HEVC encode. To confirm, run `VTCopyVideoEncoderList` on the machine and look for an entry
  whose `kVTVideoEncoderList_IsHardwareAccelerated` is true for `kCMVideoCodecType_HEVC`.
- AV1: `kCMVideoCodecType_AV1 = 'av01'` exists in `CMFormatDescription.h`, so the *format* is
  known to CoreMedia. That says nothing about an encoder. `VTCompressionProperties.h` has no AV1
  or 4:4:4 property hits. Still **needs interactive confirmation** (a runtime encoder-list
  probe), and the plan should continue to assume H.264 | HEVC only.

The one thing this does settle: this is the class of machine the doc assumed (Apple Silicon,
current OS), so the encoder profile in §4.4 is realistic.

### 12.6 Displays: exactly one, and it is the built-in panel

```
      Displays:
        Color LCD:
          Display Type: Built-in Liquid Retina XDR Display
          Resolution: 3024 x 1964 Retina
          Main Display: Yes
          Mirror: Off
          Online: Yes
          Connection Type: Internal
```

**One display, no external, no dongle attached.**

Consequences, and they are decision-relevant:

- Phase 1 as specified (mirror the primary display) is testable on this machine today with no
  hardware.
- Phase 2a (HDMI dongle) is **not testable** until a dongle is physically plugged in. Someone
  has to buy one and connect it. That procurement is now on the critical path for the virtual
  display work, and it cannot be done remotely.
- The built-in panel is 3024 x 1964 Retina, i.e. a 2x backing scale. That is a good stress case
  for the point-to-pixel mismatch risk noted in §9, and it means a "1080p60" phase-1 mode is a
  downscale of a non-integer ratio, which is the version most likely to expose scaling bugs.

### 12.7 Session and permission shape

```
-- who
kooshapari       console      Nov 15 22:38
kooshapari       ttys000      Nov 15 22:39
kooshapari       ttys001      Nov 15 22:39
kooshapari       ttys012      Nov 16 00:42
kooshapari       ttys013      Nov 16 00:42
-- console owner
kooshapari
-- uid
501
-- launchctl print gui/501
gui/501 = {
	type = login
	handle = 100023
	active count = 525
```

- A **GUI login session exists** and is owned by `kooshapari` on the console, with uid 501.
  `launchctl print gui/501` resolves and reports `type = login`. This is the exact precondition
  §7.1 needs: a per-user LaunchAgent has a session to run in. A LaunchDaemon would still be
  wrong, for the reasons in §7.1.
- The session has been up since Nov 15 22:38, which matches the "awake, logged-in Mac" premise.

```
-- pmset -g (excerpt)
 SleepDisabled		0
 sleep                1 (sleep prevented by caffeinate, caffeinate, caffeinate, powerd,
                          runningboardd, jcode, jcode, jcode, ...)
 ttyskeepawake        1
```

System sleep is set to 1 minute but is currently held off by seven `caffeinate` assertions plus
the running Slipstream client (`jcode`). Two readings, both worth recording: (a) the box is awake
right now only because something is holding it awake, so a Mac host **must** take its own
`IOPMAssertion` (§7.2) or it will sleep the moment the current clients go away, and (b) the
`ttyskeepawake 1` plus a console session means screen lock, not sleep, is the realistic failure
mode to design against.

### 12.8 TCC state: could not be read without prompting

```
-- tccutil help
tccutil: Usage: tccutil reset SERVICE [BUNDLE_ID]
-- TCC.db readable?
ls: /Users/kooshapari/Library/Application Support/com.apple.TCC/TCC.db: No such file or directory
-- sqlite read
Error: unable to open database ".../TCC.db": unable to open database file
-- system_profiler SPPrivacyDataType
(no output)
```

`tccutil` has no status verb, the per-user TCC database is not present at the documented path
(SIP-protected, and on current macOS it is not readable by an ordinary process), and
`system_profiler SPPrivacyDataType` produced nothing on this machine. There is **no read-only
way to learn whether Screen Recording or Accessibility is currently granted** from this seat.
Calling `CGPreflightScreenCaptureAccess()` would require running a binary, and a first
`CGRequestScreenCaptureAccess()` or the first SCK call can raise a prompt, which is out of bounds
for this probe. Both TCC states therefore stay **unverified**, and the ledger entries for them
stay open.

### 12.9 Virtual-display symbol probe: attempted, inconclusive

```
-- public CoreGraphics headers matching virtual|display
CGDirectDisplay.h
CGDirectDisplayMetal.h
CGDisplayConfiguration.h
CGDisplayFade.h
CGDisplayStream.h
-- CGDisplayStream.h exists
-rw-r--r-- ... MacOSX26.0.sdk/.../CoreGraphics.framework/Headers/CGDisplayStream.h  16850 bytes
-- strings count for CGVirtualDisplay in CoreGraphics binary
0
-- SkyLight / CoreDisplay / DisplayServices
.../SkyLight.framework/Versions/A/SkyLight : MISSING
.../CoreDisplay.framework/Versions/A/CoreDisplay : MISSING
.../DisplayServices.framework/Versions/A/DisplayServices : MISSING
```

Two honest conclusions and one warning about my own method:

1. **No public virtual-display header exists** in the macOS 26.0 SDK's CoreGraphics framework.
   The only display-creation-adjacent public headers are `CGDirectDisplay`, `CGDisplayConfiguration`,
   `CGDisplayStream`, and their relatives. That matches §5: there is still no supported API.
2. The `CGVirtualDisplay` string count of 0 is **not evidence that the class is gone.** These
   frameworks live in the dyld shared cache on modern macOS, so `strings` over an on-disk path
   returns 0 even for a binary that is present (as the three `MISSING` lines show). The probe was
   inconclusive, and I am not going to claim otherwise. Whether `CGVirtualDisplay` still exists on
   macOS 27 **needs interactive confirmation** (a small program that `NSClassFromString`s it, run
   by someone willing to accept the prompt/crash risk).
3. `CGDisplayStream.h` is present in the SDK, but no `API_DEPRECATED` or `API_AVAILABLE` line
   matched inside it, so its formal deprecation version remains unconfirmed.

### 12.10 Ledger resolution

| # | Ledger entry | Verdict | Settled by |
|---|---|---|---|
| 1 | Exact `CGDisplayStream` deprecation version | **needs interactive confirmation** | Header present but no availability/deprecation macro matched (§12.9) |
| 2 | `SCStreamConfiguration.colorSpaceName` availability | **refuted as written** | No `colorSpaceName` symbol anywhere in `ScreenCaptureKit.framework/Headers` on SDK 26.0 (§12.2) |
| 3 | HDR capture properties (`captureDynamicRange`) | **refuted as written, replacement unknown** | No such symbol in the SDK headers (§12.2); that HDR capture exists at all is now unconfirmed |
| 4 | SCK behaviour when Screen Recording is denied | **needs interactive confirmation** | TCC state unreadable read-only (§12.8) |
| 5 | SCK on a locked screen | **needs interactive confirmation** | Would require locking the user's live session |
| 6 | `CGVirtualDisplay` works on current macOS | **needs interactive confirmation** | Symbol probe inconclusive (§12.9) |
| 7 | VideoToolbox HEVC 4:4:4 encode | **needs interactive confirmation** | No 4:4:4 property in `VTCompressionProperties.h` (§12.5); runtime encoder list not run |
| 8 | VideoToolbox AV1 encode | **needs interactive confirmation** | `kCMVideoCodecType_AV1` exists (format only); no encode property (§12.5) |
| 9 | Live `AverageBitRate` change on an open VT session | **needs interactive confirmation** | Requires running code |
| 10 | `GCVirtualController` visibility to other apps | **needs interactive confirmation** | Requires running code |
| 11 | Any supported virtual-display API added recently | **confirmed as absent** | Only public `CGDirectDisplay*` headers in the 26.0 SDK (§12.9) |
| 12 | SCK frame emission on a static desktop | **needs interactive confirmation** | Requires a running capture |
| 13 | HEVC hardware encode on Intel Macs before 6th gen | **not applicable here; inferred for this machine** | This box is an M1 Pro, so the Intel question is moot for it (§12.5) |
| 14 | `objc2-screen-capture-kit` binding completeness | **needs interactive confirmation** | Not probed: needs a crates.io lookup and a compile |

Closed or definitively moved: 4 of 14 (entries 2, 3, 11 refuted/confirmed as written, plus 13
disposed of as moot). Everything that needs a prompt, a locked screen, or executed code stayed
open, deliberately.

### 12.11 What this changes in the plan

1. **§8.1 overstates the cost slightly and §10's phase-1 estimate improves.** A Mac with full
   Xcode 26 and Swift 6.2 exists, is reachable over SSH, and already builds the Apple client. The
   compile side of §8 is a solved problem, not a risk.
2. **Phase 1 is smaller than estimated**, because `ss-capture/platform/darwin` and
   `ss-encode/backend/darwin` already exist on the Mac as scaffolds. The work becomes verify,
   finish, and port into `spec`. Re-estimate before committing.
3. **Phase 2a is blocked on hardware procurement.** One built-in display, no external. A dongle
   has to be bought and physically attached by a human; that is now the gating item for the only
   shippable virtual-display route.
4. **The HDR line in §3.4 must be treated as speculative.** The property names in the document
   were wrong for this SDK. Find the real symbols before writing any HDR code.
5. **TCC remains the open risk it was.** It cannot be read, cannot be granted remotely, and
   cannot be tested here without prompting. The ledger entries that matter most for "is phase 1
   startable" are 4 and 5, and both still need a human at the keyboard.

---

## 13. Scaffold audit (2026-11-16, same session as §12)

Subject: `~/CodeProjects/slipstream-darwin-host` on the Mac. Read-only except `cargo check`
inside that checkout. Nothing installed, no prompts, no stream started, no display changes.

Short answer up front: the scaffold is **real, compatible, and honestly labelled, and it
implements none of the two hard things.** It is FFI declarations plus an empty capture loop plus
an empty encoder, with genuinely useful pure logic around both. It is not a half-working
pipeline, and it is not an abandoned experiment either.

### 13.1 File-by-file inventory

640 lines across five files. `wc -l`:

```
110  crates/ss-capture/src/platform/darwin/ffi.rs
111  crates/ss-capture/src/platform/darwin/mod.rs
127  crates/ss-capture/src/platform/darwin/mode.rs
149  crates/ss-encode/src/backend/darwin/mod.rs
143  crates/ss-encode/src/backend/darwin/props.rs
640  total
```

| File | Lines | Implemented | Stub / placeholder |
|---|---|---|---|
| `ss-capture/.../darwin/mod.rs` | 111 | `ScreenCaptureKitCapturer` struct; `Capturer` impl; TCC-gated `open` (44-54); `set_active`, `backend_name`, `telemetry`; one test | **`next_frame` returns `Err`** at 66-70: `"darwin capture scaffold: SCStream/SCContentFilter not wired yet"`. `is_alive` hard-returns `false` (77-79) so a pool can never re-admit it. Zero SCK objects exist. |
| `ss-capture/.../darwin/ffi.rs` | 110 | Four real `extern "C"` symbols (`CGPreflightScreenCaptureAccess`, `CGRequestScreenCaptureAccess`, `CGPreflightPostEventAccess`, `AXIsProcessTrusted`), `TccState`, `can_capture`/`can_inject`/`missing_panes`, 3 tests | None. This file is done and usable as-is. `#![allow(dead_code)]` only because nothing calls it yet. |
| `ss-capture/.../darwin/mode.rs` | 127 | `DarwinCaptureMode` with `sanitized()` (even, 320..=7680, 1..=120 fps), `frame_interval()`, `from_logical(scale)`, `PIXEL_FORMAT_NV12_VIDEO_RANGE = '420v'` / `BGRA`, `is_native_capture_format`, 6 tests | None. Pure logic, no FFI, fully tested. |
| `ss-encode/.../darwin/mod.rs` | 149 | `VideoToolboxEncoder`; `Encoder` impl; `open` rejecting odd/zero dims; conservative `EncoderCaps` (82-84); `request_keyframe` arming a flag; 4 tests | **`submit` returns `Err`** at 74-78: `"darwin encode scaffold: VTCompressionSession not wired yet"`. `poll` always `Ok(None)` (93-95). No `VTCompressionSession` exists. |
| `ss-encode/.../darwin/props.rs` | 143 | `VtPropertyPlan` (real_time, `low_latency_rate_control`, `max_frame_delay`, `allow_frame_reordering`, keyframe interval, `reference_buffer_count`), `bitrate_for`, `keyframe_interval`, `rfi_window_frames` | None. Pure data and arithmetic; the decision is made, the session is not. |

The self-description is accurate. `mod.rs:3-5` says "**STATUS: SCAFFOLD** ... `next_frame` is a
labelled `Err`: no `SCStream` is opened yet", and `ss-encode/.../mod.rs:3-6` says the same for
`submit`. Both cite a `PR.md` follow-up plan that **does not exist at the repo root** (`ls:
PR.md: No such file or directory`), so the cited plan is missing; the code comments are the only
surviving record of the intended ordering.

Two design decisions in the scaffold are worth keeping because they are the expensive ones to
get wrong and they are already right:

- `ffi.rs:3-7` records *why* the bindings are hand-written `extern "C"` rather than `objc2-*`:
  the workspace `Cargo.lock` carries only transitive `core-foundation`/`core-foundation-sys`,
  so `objc2-screen-capture-kit` and `objc2-video-toolbox` are **not** available and adding them
  means new network resolution. That answers ledger entry 14's premise directly.
- `mode.rs:61-66` picks `420v` (NV12 video range) as the capture format precisely so the
  capture to VideoToolbox leg needs no host-side colour conversion, with BGRA demoted to a
  debug/fallback format. That is the right call and it matches §3.2.

### 13.2 Does it compile?

```
cargo check -p ss-capture   -> exit 101
cargo check -p ss-encode    -> exit 101
```

Both fail, and **neither failure is in the darwin code**. The build dies in a dependency's build
script:

```
running: cd ".../target/debug/build/audiopus_sys-.../out/build" && cmake ... opus ...
thread 'main' panicked at cmake-0.1.58/src/lib.rs:1132:5:
  failed to execute command: No such file or directory (os error 2)
  is `cmake` not installed?
build script failed, must exit now
```

`command -v cmake` -> `cmake: ABSENT`. `command -v brew` -> `brew: ABSENT`. Xcode 26 does not
ship cmake and there is no Homebrew on this machine to install it with, so this was not a
five-second fix and installing anything was out of bounds.

Compounding evidence that the scaffold has never been compiled here:

```
ls target/debug/deps | grep -E "ss_capture|ss_encode"   -> no output
ls -l target/debug/deps/libss_capture*                   -> no matches found
```

There is no prior artifact for either crate in the target directory. So the honest status of
"does it compile" is **unknown, and untested even at authoring time.** It is not "no": the
`#[cfg(target_os = "macos")]` arms are correctly wired (`platform/mod.rs` `pub mod darwin;` and
`ss-encode/src/lib.rs:1840` `#[path = "backend/darwin/mod.rs"] mod darwin;`), and the code uses
the current `Capturer`/`Encoder`/`CapturedFrame` shapes including the 7-field struct, so a type
mismatch is unlikely. But unlikely is not checked.

To settle it: install cmake (the whole blocker), then re-run both. That is a short task that is
blocked only by the "no installs" constraint, and it should be item one of the next session
rather than an open question in a document. `--no-default-features` was tried as a sidestep and
does not work, because `opus` arrives through `slipstream-core`'s `quic` feature which these
crates request themselves; the attempt and its exit codes are in §14.4.

### 13.3 Shortest honest path to "frames arrive at a client"

| # | Workstream | Status | Evidence |
|---|---|---|---|
| 1 | SCK stream setup and frame delivery (`SCStream`, `SCContentFilter`, `SCStreamConfiguration`, `SCStreamOutput` delegate) | **Absent** | `next_frame` is an `Err`, `mod.rs:66-70`. No ObjC message-send surface exists at all: `ffi.rs` only declares four C functions. |
| 2 | IOSurface/CVPixelBuffer handling and a macOS `FramePayload` | **Absent** | `ss-frame/src/core/frame.rs` has zero `macos` matches; `mod.rs:9-15` explicitly states `FramePayload` has no macOS variant yet. The intended shape (retain + hand to VT) is documented, not built. |
| 3 | VideoToolbox session, callback, Annex-B rewrite | **Absent, plan exists** | `submit` is an `Err` (`mod.rs:74-78`); the property plan is real (`props.rs`, 143 lines) but no `VTCompressionSessionCreate` call exists anywhere. Annex-B conversion is entirely unstarted. |
| 4 | Encoder dispatch (`open_video` macOS arm, `host_wire_caps` macOS arm) | **Absent** | The module is declared (`lib.rs:1840-1842`) but the `not(linux)` `open_video_backend` arm still resolves macOS to `sw::OpenH264Encoder`: it has no macOS branch. |
| 5 | Host/session wiring (capture arm in `slipstream-host/src/capture.rs`, audio, `ss-paths`) | **Absent** | `grep -n "darwin\|macos" crates/slipstream-host/src/capture.rs` -> no output; `ss-paths` -> no `macos`; `audio.rs` still bails on non-Linux/Windows. |
| 6 | Input injection (`Backend::MacCgEvent`, `CGEventPost` FFI, Accessibility gate) | **Absent**, gate half-built | `ss-inject/src/input/` has no darwin entry and `lib.rs` has no `macos` arm. But `ffi.rs` already samples Accessibility and post-event access and names the panes, so the policy half exists in the wrong crate. |

So the remaining work is the two items that matter: **an ObjC bridge into ScreenCaptureKit** and
**a real VideoToolbox session with an Annex-B rewrite.** Everything around them (mode clamping,
TCC gating, property decisions, capability honesty) is done.

### 13.4 Revised phase-1 estimate

Original: 2 to 4 weeks plus 3 to 5 days of signing/CI friction (§10). Revised: **2.5 to 3.5
weeks plus the same signing/CI friction**, and the reason is narrower than "the scaffold helps".

Cheaper because:

- Mode clamping, Retina scale handling, and pixel-format choice: done, tested, no work left
  except wiring (`mode.rs`).
- TCC preflight, the exact failure message, and the pane list: done (`ffi.rs`).
- The VideoToolbox property plan and the bitrate model: done (`props.rs`). Those are the
  decisions that usually cost two days of reading and one of arguing.
- Capability honesty (`caps()` returning no RFI, no cursor blend, no sub-frame) is already
  correct, which saves the class of bug where a backend advertises a capability it cannot honour.

Untouched, i.e. the estimate does not move on these:

- ObjC interop for ScreenCaptureKit. This is the single largest unknown and the scaffold does
  nothing to reduce it. Hand-written `objc_msgSend` shims or a new `objc2-*` dependency, plus
  delegate callbacks landing on a Rust queue. **Unchanged: the dominant slice.**
- `FramePayload` variant and buffer lifetime. Untouched.
- Annex-B conversion and parameter-set extraction. Untouched, and it is the classic cause of a
  black client.
- Session wiring, audio, `ss-paths`, launchd, signing, TCC-per-signature loop, CI. Untouched.
- Input injection. Untouched.

Explicitly **not** cheaper: the risk profile. The scaffold removes design uncertainty and
removes zero implementation uncertainty.

### 13.5 Is the scaffold usable or misleading?

**Usable.** Reasons, specifically:

- It is written against the current types: the encode test at `ss-encode/.../mod.rs:136-145`
  builds a `CapturedFrame` with the current seven fields (`width`, `height`, `pts_ns`, `format`,
  `payload`, `cursor`, `stage_ns`), and the capture test at `ss-capture/.../mod.rs:98` calls
  `ScreenCaptureKitCapturer::open` with a `DarwinCaptureMode`. Both would fail to compile against
  an older shape, so the code is not stale.
- It declares its own incompleteness in the first five lines of both modules and returns labelled
  errors instead of pretending (`next_frame`, `submit`). A scaffold that returns `Err("not wired
  yet")` cannot be mistaken for working in a log.
- It is not a dead branch: `platform/mod.rs` and `lib.rs:1840-1842` both exist behind
  `cfg(target_os = "macos")`, so the moment the workspace builds on a Mac these files are in the
  build graph.

The two things that make it *look* like more progress than it is, and that a reader should be
told:

1. 640 lines reads like a lot. Roughly 250 of them are tests and ~150 are pure data/logic, so
   the actual capture+encode implementation is **zero lines**.
2. It lives in a different checkout (`slipstream-darwin-host`) from the one the work is being
   planned in (`spec`), and `spec` has none of it. Anything ported has to be re-reviewed against
   `spec`'s current types rather than assumed identical. Two divergent copies of a platform
   backend is a merge problem waiting to happen; decide now which checkout is canonical before
   more is written.

Neither of those is a reason to distrust it. Both are reasons not to call phase 1 "half done".

---

## 14. Defects found while auditing the scaffold

Both are recorded, neither is fixed. Line numbers are from
`~/CodeProjects/slipstream-darwin-host` at `f3cdbdb`.

### 14.1 Defect A: the encoder dispatch can never reach VideoToolbox

`crates/ss-encode/src/lib.rs:604` declares the only non-Linux encoder factory:

```rust
/// Open the non-Linux encoder backend. Windows tries direct-SDK NVENC when the selected
/// adapter is NVIDIA (or the operator pinned `nvenc`) and the driver probe succeeded;
/// everything else — including a failed NVENC open on an `auto` H.264 session — uses
/// openh264. Same validation contract as the Linux arm.
#[cfg(not(target_os = "linux"))]          // :604
#[allow(clippy::too_many_arguments)]
fn open_video_backend(                    // :606
    codec: Codec, format: PixelFormat, width: u32, height: u32, fps: u32,
    bitrate_bps: u64, cuda: bool, bit_depth: u8, chroma: ChromaFormat,
    cursor_blend: bool, max_slices: u32,
) -> Result<(Box<dyn Encoder>, &'static str)> {
```

Its only hardware branch is Windows-and-NVENC only:

```rust
    #[cfg(all(target_os = "windows", feature = "nvenc"))]   // :623
    {
        ... nvenc_d3d11::NvencD3d11Encoder::open(...) ...    // :642
    }
    #[cfg(not(all(target_os = "windows", feature = "nvenc")))]
    let _ = (cuda, bit_depth, chroma, cursor_blend, max_slices);  // :664-665
```

and then, unconditionally:

```rust
    if codec != Codec::H264 {                               // :666
        anyhow::bail!(
            "the software encoder emits H.264 only; the session negotiated {codec:?} \
             (a client must advertise CODEC_H264 to reach a software host)"
        );
    }
    sw::OpenH264Encoder::open(format, width, height, fps, bitrate_bps.min(SW_BITRATE_CEIL))
        .map(|e| (Box::new(e) as Box<dyn Encoder>, "software"))  // :672-673
}
```

On macOS the Windows block compiles out, `let _ = (...)` absorbs the arguments, and the tail is
unconditional. So **every macOS session resolves to `"software"` and openh264, H.264 only**, no
matter which GPU is present. `host_wire_caps` on the same `not(target_os = "linux")` cfg returns
`CODEC_H264` for the same reason, so a Mac host would advertise H.264 and nothing else even
though its media engine encodes HEVC.

**Is the darwin backend reachable at all from that dispatch? No.** The module is compiled:

```rust
// lib.rs:1840-1842
#[cfg(target_os = "macos")]
#[path = "backend/darwin/mod.rs"]
mod darwin;
```

but nothing in the file references `darwin::` outside that declaration, which is exactly why
`backend/darwin/mod.rs:19` carries `#![allow(dead_code)] // scaffold: open_video does not dispatch
here yet`. It is in the build graph and unreachable from it.

**What the correct arm looks like** (not written, for the record): a `#[cfg(target_os = "macos")]`
block before the software tail that constructs
`darwin::VideoToolboxEncoder::open(darwin::props::VtPropertyPlan::for_stream(w, h, fps, bps),
darwin::VtPath::Hardware)` and returns the label `"videotoolbox-hevc"`; a macOS `host_wire_caps`
arm that probes `VTCopyVideoEncoderList` instead of assuming H.264; and narrowing this function's
attribute so the openh264 tail is an explicit fallback for a named platform rather than the
default for every platform that is not Linux. Until then, "VideoToolbox support" means "a module
nobody calls".

### 14.2 Defect B: the injection permission policy lives in the capture crate

`crates/ss-capture/src/platform/darwin/ffi.rs` declares and exposes three grants:

```rust
#[link(name = "ApplicationServices", kind = "framework")]     // :20
extern "C" {
    pub fn AXIsProcessTrusted() -> u8;                        // :24
}
pub fn can_inject(&self) -> bool { self.accessibility && self.post_events }   // :60-62
pub fn missing_panes(&self) -> Vec<&'static str> { ... "Privacy & Security > Accessibility" ...
                                                       ... "Privacy & Security > Input Monitoring" }  // :65-77
```

Capture needs exactly one of those three (`CGPreflightScreenCaptureAccess`). The other two are
input-injection gates, and `ss-inject` is the crate that owns `InputInjector`,
`default_backend()`, and `text_input_supported()`. Today `ss-inject` has **no macOS arm at all**
(no darwin entry under `crates/ss-inject/src/input/`, no `macos` match in `lib.rs`), so the policy
for a subsystem that does not exist yet is sitting in a crate that will never call it.

**Are the TCC symbols crate-appropriate today? Half of them are.** The screen-recording half is
exactly right where it is. The event-posting half is not, and it is mildly harmful rather than
merely untidy: `TccState` bundles all three, so `missing_panes()` on a screen-recording failure
also reports the Accessibility and Input Monitoring panes. A capture-only error will therefore
tell an operator to visit two panes that cannot fix it, and `ss-capture` links
`ApplicationServices` (an input framework) to compute a verdict it must not act on.

**What moving them costs: almost nothing, and that is the argument for doing it.**
`grep` across the whole workspace finds exactly one consumer:

```
crates/ss-capture/src/platform/darwin/mod.rs:45:        let tcc = ffi::tcc_state();
```

Nothing else imports `ffi::`, `TccState`, `tcc_state`, or the AX/post-event symbols. So the split
is: keep `CGPreflightScreenCaptureAccess` and the screen-recording half of `TccState` in
`ss-capture`; move `AXIsProcessTrusted`, `CGPreflightPostEventAccess`, `can_inject()`, the
injection half of `missing_panes()`, and the two injection tests into
`ss-inject/src/input/darwin/tcc.rs`. That is roughly 40 moved lines, one call site in the capture
`open()` narrowed to the screen-recording check, and one `#[link]` block dropped from
`ss-capture`. No new crate dependency is needed in either direction: the two crates do not depend
on each other, so the cost is duplicating two `extern "C"` lines, not adding an edge in the
dependency graph. If a third consumer ever appears, promote it to a small shared crate then,
not now.

### 14.3 Open decision (operator's call): which checkout is canonical

I am not deciding this. Facts, so the decision is cheap to make:

```
$ git rev-parse --abbrev-ref HEAD   -> feat/darwin-host
$ git rev-list --count HEAD         -> 2
$ git log --oneline -5
f3cdbdb feat(macos-host): scaffold ScreenCaptureKit capture + VideoToolbox HEVC encode
93f083d Merge pull request #75 from KooshaPari/codex/gamestream-fec-budget
$ git remote -v
origin  https://github.com/vindeckyy/slipstream (fetch) [blob:none]
origin  https://github.com/vindeckyy/slipstream (push)
$ git status --porcelain | wc -l    -> 0
```

So it is a real repository on branch `feat/darwin-host` with **two commits**, a clean working
tree, and a blobless partial clone of `vindeckyy/slipstream` as origin. The scaffold itself is one
commit touching exactly the expected files:

```
f3cdbdb crates/ss-capture/src/platform/darwin/ffi.rs      | 110 +
       crates/ss-capture/src/platform/darwin/mod.rs      | 111 +
       crates/ss-capture/src/platform/darwin/mode.rs     | 127 +
       crates/ss-capture/src/platform/mod.rs             |   4 +
       crates/ss-encode/src/backend/darwin/mod.rs        | 149 +
       crates/ss-encode/src/backend/darwin/props.rs      | 143 +
       crates/ss-encode/src/lib.rs                       |   5 +
       7 files changed, 649 insertions(+)
```

How it reaches the canonical repo, given that: it is committed, so the scaffold is **fetchable as
a commit** (add the remote, fetch `f3cdbdb`, cherry-pick) or exportable as a patch
(`git format-patch -1 f3cdbdb`). No file copying is required. But the base commit is
`93f083d` "Merge pull request #75" on a repo whose remote is `vindeckyy/slipstream`, so whether
that base matches `spec`'s history has not been checked and would need confirming before a
cherry-pick is assumed clean.

The three options, with their costs, for the operator:

1. **Port `f3cdbdb` into `spec` and retire the checkout.** One review commit, one patch, then
   delete `slipstream-darwin-host`. Cheapest long term, requires the base-commit comparison.
2. **Make `darwin-host` canonical for the macOS host and `spec` canonical for Linux/Windows.**
   Buys a Mac-native tree, costs permanent two-repo drift in four crates and a merge problem the
   first time a shared type changes.
3. **Treat `darwin-host` as a scratch fork**, port the useful parts, never merge it. Costs
   nothing now, loses the tests if nobody ports them.

My read, offered as input and not as a decision: option 1, because the scaffold is small,
committed, cleanly scoped, and already correct against the current types. But it is the
operator's call, and nothing was pushed, fetched, or cherry-picked from that machine.

### 14.4 Compile status after trying to sidestep cmake

Attempted, in that checkout, nothing installed:

```
cargo check -p slipstream-core --no-default-features        -> exit 0   (SUCCESS)
cargo check -p ss-capture  --no-default-features            -> exit 101
cargo check -p ss-encode   --no-default-features            -> exit 101
```

`slipstream-core` with no default features typechecks, so the core itself is fine and `cmake` is
only reachable through one path. It cannot be turned off from these two crates:

- `slipstream-core/Cargo.toml:26` puts `dep:opus` inside the **`quic`** feature
  (`quic = ["tls", "dep:quinn", "dep:tokio", "dep:rcgen", "dep:hmac", "dep:spake2", "dep:opus"]`).
- `ss-capture` and `ss-encode` each request `slipstream-core = { features = ["quic"] }` in their
  **own** dependency declaration, and `ss-capture` has no `[features]` section at all, so
  `--no-default-features` is a no-op for them.
- `cargo tree -p ss-capture -i audiopus_sys` confirms the path:
  `audiopus_sys -> opus -> slipstream-core (ss-capture, ss-frame)`.

So **the honest status of the darwin modules remains UNKNOWN.** They still have never been
compiled. The only ways out are to install `cmake`, to build `opus` from a vendored/pregenerated
path, or to typecheck the darwin files against a stripped stub crate. All three were out of
scope here, and none was attempted.

### 14.5 Port plan (canonical checkout decided: `spec`)

Decision, made by the coordinator: **`spec` (this checkout) is canonical.** The side checkout
`~/CodeProjects/slipstream-darwin-host` is retired once its scaffold is ported, because a platform
backend living in a side clone is exactly how two divergent implementations happen, and the commit
is small enough (7 files, +649 lines) that moving it now costs far less than not moving it.

**This document supersedes the `PR.md`** that both darwin modules cite as their follow-up plan.
That file does not exist at the repo root of that checkout, so the staged ordering it promises is
recorded here instead.

Ordered port items:

1. **Cherry-pick `f3cdbdb` into `spec`.** Patch exported from that checkout as
   `darwin-scaffold-f3cdbdb.patch`, 29,295 bytes, sha256
   `de7e10753e061132e71ce901a72e1ba7b61e15d04ffae6d46dd709808a858780`. It creates 5 files and
   modifies 2 (`crates/ss-capture/src/platform/mod.rs`, `crates/ss-encode/src/lib.rs`). Prefer
   `git am` over `git apply` so the author and message survive. Verify the trailer claim
   `tx-validated: cargo-check` independently: no build artifact for either crate exists in that
   checkout's `target/`, and neither crate can build there without `cmake` (§13.2, §14.4), so the
   trailer is **not supported by evidence**. Do not carry that claim forward as if it were true.
2. **Fix defect B during the port** (§14.2). Move `AXIsProcessTrusted`,
   `CGPreflightPostEventAccess`, `can_inject()`, the injection half of `missing_panes()`, and the
   two injection tests out of `ss-capture/src/platform/darwin/ffi.rs` into
   `ss-inject/src/input/darwin/tcc.rs`. Keep `CGPreflightScreenCaptureAccess` and the
   screen-recording half where they are, narrow the single call site (`darwin/mod.rs:45`) to the
   screen-recording check, and drop the `#[link(name = "ApplicationServices")]` block from
   `ss-capture`. Cost: ~40 moved lines, one narrowed call site, one dropped framework link, no new
   crate edge. Do it in the same port commit or immediately after, so the crate boundary is correct
   before anything is built on top of it.
3. **Do NOT fix defect A during the port** (§14.1). A macOS session resolving to openh264 is
   deliberately left alone, because the fix belongs with the VideoToolbox backend work in phase 1:
   wiring the dispatch to a backend whose `submit` is still an `Err` would only move the failure
   from "wrong backend, H.264 only" to "open fails", with no user-visible improvement. When the
   darwin module gains its first real caller (the macOS arm of `open_video`), remove
   `#![allow(dead_code)]` from both `ss-encode/src/backend/darwin/mod.rs:19` and
   `ss-capture/src/platform/darwin/mod.rs:16` in that same commit, so a future regression to
   "nobody calls it" is a warning rather than silence.
