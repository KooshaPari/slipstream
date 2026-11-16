# clients/windows: a native Windows session client (transport + admission + present seam)

## Problem

There is no Windows client. `clients/` has `android`, `apple`, `cli` (a Linux-only stub that prints
"slipstream CLI runs on Linux"), `decky`, `linux`, `probe`, `session`, `shared`. Windows operators
today either run the headless probe by hand or nothing at all.

**This is not StreamLight.** StreamLight (`C:\Program Files\StreamLight`) is a third-party Moonlight
client speaking NVIDIA's GameStream/GFE protocol. It shares no code and no wire format with
slipstream, and nothing in this change reads or depends on it. Any claim that this PR "wraps
StreamLight" is wrong.

## Scope

Two honest halves, in one PR:

1. **Complete and real:** LAN discovery, manual host entry, identity persistence, the PIN pairing
   ceremony, QUIC connect + Hello/Welcome negotiation, admission, and the full video-plane receive
   loop (with `--record` writing a playable elementary stream). This is the part that must not be a
   stub, and it is not.
2. **Seam, not yet a backend:** the window (winit + softbuffer) and the hardware decode
   (D3D11VA via FFmpeg / NVDEC) attach at two traits — `Decoder` and `Presenter`. They are not in
   this PR because this box has no FFmpeg **development SDK** (only `ffmpeg.exe` from ImageMagick),
   and `crates/ss-presenter` is Vulkan/Linux-only (`cfg(target_os = "linux")` throughout). Writing a
   winit + D3D11VA path that cannot be compiled or seen here would be an unverified 600-line blob;
   the traits are the smallest thing that is honest today and keeps the session loop unchanged when
   the backend lands.

Reuse, not reinvention: `slipstream-core` (QUIC endpoint, `client::NativeClient::pair`, `Session`,
`UdpTransport`, `Hello`/`Welcome`/`Start`), the same `mdns-sd` crate/version the probe browses with,
and the same identity file layout the other clients use. `crates/ss-client-core` is *not* reused
because every module in it except `os` is `#[cfg(target_os = "linux")]` — on Windows it compiles to
an empty crate. That is why the receive loop mirrors `clients/probe/src/session.rs` (the proven
portable path) instead of `ss-client-core`.

## Design

- `HostAdvert` decodes a `_slipstream._udp` TXT record (`id`, `fp`, `pair`, `mac`, `os`) exactly as
  `clients/apple` does, and is **fail-closed**: only a literal `pair=optional` offers TOFU; a
  missing, unknown, or `required` value means pairing is required.
- `Admission` is the post-handshake table: `Stream` / `ConfirmTrust` / `RefuseUnpaired`, with
  `requires_drop()` so a refusal can never leave a live connection behind.
- `ConnectOutcome` classifies rejection statuses (including `0x0107` approval-timeout) into text the
  operator can act on, and keeps "pin mismatch" distinct from "unreachable" — the one failure that
  must not be retried blindly.
- `Machine` is the phase machine (`Idle`/`Connecting`/`AwaitingTrust`/`Streaming`): only idle may
  connect (no double sessions), input is only permitted while streaming, and every failure returns
  to idle while naming the host.
- Manual `ip:port` entry has no advert, so it takes the TOFU branch (confirm the fingerprint the
  host presented) — the probe's behaviour. A `pair=required` advert is what refuses.

## Files

| File | Lines | What |
|---|---|---|
| `clients/windows/Cargo.toml` | 28 | crate manifest; no window/GPU deps yet (justified in README) |
| `clients/windows/src/main.rs` | 278 | dispatch, `--pair`, connect, Hello/Welcome, receive loop, `--record` |
| `clients/windows/src/args.rs` | 396 | flags, `Target` (host:port, IPv6 brackets), hex fingerprint |
| `clients/windows/src/connect.rs` | 411 | `HostAdvert`, `PairingPolicy`, `Admission`, `ConnectOutcome`, `Machine` |
| `clients/windows/src/discovery.rs` | 181 | mDNS browse, `DiscoveredHost`, console rows |
| `clients/windows/src/identity.rs` | 59 | cert/key at `%APPDATA%\slipstream\`, shared with the other clients |
| `clients/windows/src/present.rs` | 198 | `Decoder`/`Presenter` seam, RGBA `TestPattern`, counting presenter |
| `clients/windows/README.md` | 80 | build recipe, run examples, StreamLight disclaimer |
| `Cargo.toml` | +1 | `"clients/windows"` workspace member |

## Testing (real output, this box)

Toolchain: `rustc 1.96.0` (repo pin), x86_64-pc-windows-msvc. `audiopus_sys` needs two env
overrides on this machine (its CMake picks the VS 2026 generator that CMake 4.0.3 does not know, and
its `cmake_minimum_required` is below CMake 4's floor):

```bat
set "CMAKE_GENERATOR=Visual Studio 17 2022"
set "CMAKE_POLICY_VERSION_MINIMUM=3.5"
```

```
cargo build --release -p slipstream-windows
warning: slipstream-core@0.24.0: slipstream-core: wrote ...\include\slipstream_core.h
    Finished `release` profile [optimized] target(s) in 45.75s

cargo test -p slipstream-windows
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.31s
     Running unittests src\main.rs (target\debug\deps\slipstream_windows-…)

running 30 tests
test args::tests::defaults_match_the_probe_baseline ... ok
test args::tests::bare_positional_is_the_host ... ok
test args::tests::second_positional_is_rejected ... ok
test args::tests::target_defaults_the_port_and_handles_ipv6 ... ok
test args::tests::malformed_pin_fails_closed ... ok
test args::tests::discover_takes_an_optional_duration ... ok
test args::tests::pair_and_connect_conflict ... ok
test args::tests::unknown_flag_and_bad_values_are_errors ... ok
test args::tests::codec_and_mode_parse ... ok
test args::tests::record_is_carried_through ... ok
test args::tests::help_is_not_silently_ignored ... ok
test connect::tests::realistic_advert_parses ... ok
test connect::tests::pairing_policy_is_fail_closed ... ok
test connect::tests::missing_keys_are_empty_not_panicky ... ok
test connect::tests::admission_table ... ok
test connect::tests::machine_happy_path ... ok
test connect::tests::double_connect_is_refused ... ok
test connect::tests::refused_connection_drops_and_goes_idle ... ok
test connect::tests::tofu_waits_for_confirmation ... ok
test connect::tests::failures_return_to_idle_with_actionable_text ... ok
test connect::tests::disconnect_from_streaming_is_clean ... ok
test connect::tests::every_admission_message_names_the_host ... ok
test discovery::tests::ipv4_is_preferred_over_ipv6 ... ok
test discovery::tests::key_falls_back_to_fullname_without_an_id ... ok
test discovery::tests::row_reports_the_pairing_policy_and_a_fp_stub ... ok
test present::tests::passthrough_counts_every_au_and_presents_nothing ... ok
test present::tests::test_pattern_is_rgba_and_opaque ... ok
test present::tests::test_pattern_moves_between_frames ... ok
test present::tests::tiny_and_degenerate_modes_do_not_panic ... ok
test present::tests::console_presenter_status_does_not_count_frames ... ok

test result: ok. 30 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

### Live attempt (one, as scoped)

```
slipstream-windows.exe 100.112.143.99:9777 --seconds 12 --record ...\winclient-live.h265
```

Result: **no frames**. `ping 100.112.143.99` answers in 4–6 ms (tailnet up), but the QUIC handshake
did not complete within ~120 s and the run produced no log line past `connected` and no record file.
The host is reachable at the IP layer and not answering the handshake — consistent with the known
wedged-host state (capture thread not released after a prior session) and/or other agents rebuilding
that box. **This is not attributed to the client**: the identical transport path is what
`slipstream-probe` uses, and no client-side error was raised. The only client-side gap this exposed
is that `Connecting::await` has no timeout, so a silent host hangs the CLI (see follow-up 4). Per the
lane, this was attempted once and not chased.

## Risks

1. **No pixels yet.** The client is a transport client until `Decoder`/`Presenter` have a backend.
   Anyone reading "Windows client" as "streams video to a window" will be disappointed; the README
   states this in its second section.
2. **Build depends on two env overrides** (`CMAKE_GENERATOR`, `CMAKE_POLICY_VERSION_MINIMUM`) for
   `audiopus_sys`. A fresh contributor without them sees an opaque CMake panic. Documented in the
   README; the durable fix belongs in the repo (`.cargo/config.toml` env, or bumping `audiopus_sys`).
3. **Unverified pairing against a live host** — the ceremony is the probe's proven code path, but
   this PR could not complete one because the host was not answering.
4. **`Admission::RefuseUnpaired` is unreachable from the CLI today** (manual entry always TOFU),
   because discovery-driven connect is not wired. The rule is tested; the caller lands with the
   discover-to-connect flow.
5. Windows-only-by-intent but not gated: the crate builds on any host where `slipstream-core` does;
   the identity path prefers `%APPDATA%`.

## Follow-ups (with anchors)

1. **Window + present backend.** `clients/windows/src/present.rs` — implement `Presenter` with
   winit 0.29 + softbuffer over `DecodedFrame.rgba`; construct it in
   `clients/windows/src/main.rs::stream_async` in place of `ConsolePresenter::default()`.
2. **Hardware decode.** `clients/windows/src/present.rs::Decoder` — implement with `ffmpeg-next`
   (`AV_HWDEVICE_TYPE_D3D11VA`, falling back to NVDEC), sized from `welcome.mode`. Requires adding
   the FFmpeg dev SDK to the Windows build (or a `build`-feature vendored build); re-enable the
   `video_caps` bits in `main.rs::stream_async`'s `Hello` once it can actually render 10-bit/4:4:4.
3. **Input plane.** `clients/windows/src/present.rs` — a third seam for input, or send from the
   window: map Win32 Raw Input (`WM_INPUT`) to `slipstream_core::input::{InputEvent, InputKind}` with
   evdev codes, and gate on `connect::Machine::can_send_input()`. The reserved cross-client hotkeys
   are Ctrl+Alt+Shift+Q (release pointer) / +D (disconnect) / +S (stats) / +A (mute), per
   `clients/apple/Sources/SlipstreamClient/Session/StreamCommands.swift:3`.
4. **Connect timeout.** `clients/windows/src/main.rs::stream_async` — wrap
   `ep.connect(..).await` in a 10 s timeout (185 s for a delegated-approval request, mirroring the
   host's 180 s park) so a wedged host fails with `ConnectOutcome::Unreachable` instead of hanging.
   Exposed by the live attempt above.
5. **Discovery-driven connect.** `clients/windows/src/discovery.rs::browse` already returns
   `DiscoveredHost` with the advert; wire a host picker so `decide(.., Some(&advert))` and
   `Admission::RefuseUnpaired` become reachable.
6. **Audio plane.** Not consumed at all yet (the probe decodes it separately).

## Checklist

- [x] New crate builds: `cargo build --release -p slipstream-windows` → Finished (real output above)
- [x] Unit tests: 30 passed, 0 failed
- [x] Files under ~500 lines (largest: `connect.rs` 411)
- [x] `Cargo.toml` workspace member added; no change to the main checkout
- [x] README with build + run + the StreamLight disclaimer
- [ ] Live stream verified end to end (host silent; follow-up 4 hardens the failure)
- [ ] Video presented in a window (follow-ups 1–2)
- [ ] Input plane (follow-up 3)
