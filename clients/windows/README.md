# slipstream-windows

The native Windows session client. **Not** StreamLight — that is a third-party Moonlight client
(with an entirely different protocol) and has nothing to do with this crate. This speaks
`slipstream/1` to `slipstream-host`.

## What works today

| Area | State |
|---|---|
| LAN discovery (`_slipstream._udp`, port 9777) | works — `--discover` |
| Manual host entry `ip:port` (port defaults to 9777) | works |
| Identity / pairing persistence (`%APPDATA%\slipstream\client-{cert,key}.pem`) | works, shared with the other clients |
| PIN pairing ceremony (`--pair PIN`) | works (same `NativeClient::pair` the probe uses) |
| QUIC connect, Hello/Welcome negotiation, `Start` | works |
| Video plane receive (every access unit, `--record FILE` writes a playable `.h265/.h264/.av1`) | works |
| Admission / connection state machine (`HostAdvert`, admission table, phases) | implemented and unit-tested |
| Win32 window, GDI present, keyboard/mouse capture, decode (Media Foundation/D3D11VA, ffmpeg fallback) | implemented — `src/window.rs`, `src/input.rs`, `src/mf.rs`, `src/ff.rs` |

The window and the decode backend attach at the two traits in `src/present.rs` (`Decoder`,
`Presenter`), so both can be exercised headlessly in tests as well as live.

## Build

Requires Rust 1.96 (pinned by `rust-toolchain.toml` in the repo root) and the MSVC toolchain.

`audiopus_sys` builds libopus through CMake, and two things trip it up on a machine that has only the
Visual Studio 18 Build Tools:

1. The bundled Opus `CMakeLists.txt` predates the CMake 4 policy floor, so CMake refuses it unless
   `CMAKE_POLICY_VERSION_MINIMUM=3.5` is set.
2. The CMake on `PATH` is often too old to know the `Visual Studio 18 2026` generator (4.0.3 is not
   enough). VS 18 Build Tools ships a newer CMake that does, so put that first. Forcing
   `CMAKE_GENERATOR=Visual Studio 17 2022` does **not** work on such a box: there is no VS 17
   instance and CMake fails with "could not find any instance of Visual Studio".

```bat
set "CMAKE_POLICY_VERSION_MINIMUM=3.5"
set "PATH=%ProgramFiles(x86)%\Microsoft Visual Studio\18\BuildTools\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin;%PATH%"
cargo build --release -p slipstream-windows
```

If the inherited environment carries `CARGO_TARGET_DIR` pointing at a drive that is not mounted,
cargo fails immediately with `The device is not ready. (os error 21)`; clear it for the build with
`set "CARGO_TARGET_DIR="`.

Binary: `target\release\slipstream-windows.exe`.

## Run

```bat
:: discover hosts on the LAN, then exit
slipstream-windows --discover
slipstream-windows --discover 8

:: pair once (reads the PIN the host prints when pairing is armed)
slipstream-windows 192.0.2.10:9777 --pair 1234
:: -> PAIRED — host fingerprint <hex>   (use it as --pin from now on)

:: stream for 12 s and record the elementary stream
slipstream-windows 192.0.2.10:9777 --pin <hex> --seconds 12 --record out.h265

:: unpinned: trust-on-first-use, the observed fingerprint is logged for pinning
slipstream-windows 192.0.2.10:9777 --seconds 30
```

Every example above uses `192.0.2.10`, which RFC 5737 reserves for documentation (TEST-NET-1),
so it names no real machine. The client has no default host: it only connects to the `HOST[:PORT]`
you pass as a positional or with `--connect` (a bare host defaults to port `9777`).

Flags: `--connect HOST[:PORT]` (or a bare positional), `--pin HEX`, `--pair PIN`,
`--discover [SECS]`, `--name LABEL`, `--mode WxHxFPS` (default `1920x1080x144`),
`--bitrate KBPS`, `--codec auto|h264|hevc|av1`, `--audio-channels 2|6|8`, `--seconds N`,
`--record FILE`, `--monitor BACKEND:NAME` (stream a specific head for this session; the host validates
the id). Exit code `3` means the session was admitted but the host published zero frames.

`RUST_LOG=debug` for the transport log.

## Test

```bat
cargo test -p slipstream-windows
```

68 tests cover argument parsing, `host:port`/IPv6 handling, TXT advert decoding (fail-closed
pairing policy), the admission table, every connection phase transition, and `--monitor` validation.

## Layout

- `src/args.rs` — flag parsing, `Target` (host/port), fingerprint hex.
- `src/connect.rs` — `HostAdvert`, `PairingPolicy`, `Admission`, `ConnectOutcome`, `Machine`.
- `src/discovery.rs` — mDNS browse + console rows.
- `src/identity.rs` — cert/key persistence.
- `src/present.rs` — `Decoder`/`Presenter` seam, test pattern, counting presenter.
- `src/main.rs` — dispatch, pair, connect, receive loop.
- `src/session.rs` — the connect path: Hello/Welcome, trust prompt, decode-and-present loop.
- `src/window.rs` — Win32 window and GDI blit. With `src/mf.rs` it holds the crate's only `unsafe`.
- `src/input.rs` — keyboard/mouse capture forwarded to the host.
- `src/mf.rs` — Media Foundation (D3D11VA) decoder.
- `src/ff.rs` — ffmpeg decoder fallback and the record/replay path.
