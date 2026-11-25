//! Runtime for one Windows client streaming session.
//!
//! This module owns the handshake, admission and receive/decode/present loop. It never exits the
//! process: the CLI boundary in `main` decides what an outcome means for the process exit code.

use anyhow::{anyhow, bail, Context, Result};
use std::io::Write;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use slipstream_core::config::{Mode, Role};
use slipstream_core::input::InputEvent;
use slipstream_core::packet::FLAG_PROBE;
use slipstream_core::quic::{endpoint, io, Hello, RequestKeyframe, Start, Welcome};
use slipstream_core::quic::{CODEC_AV1, CODEC_H264, CODEC_HEVC};
use slipstream_core::transport::UdpTransport;
use slipstream_core::{Session, SlipstreamError};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::args::{hex, Args};
use crate::connect::{self, Machine};
use crate::identity;
use crate::input::{InputSender, InputSenderGuard};
use crate::present::{ConsolePresenter, Decoder, PassthroughDecoder, Presenter};
use crate::recovery::{KeyframeRequest, RecoveryEvent, RecoveryState};
use crate::{ff, mf, window, PipeStats};

/// How long the client tolerates post-handshake silence before it says WHY it is still waiting.
/// An unpaired device's first knock is parked host-side for delegated approval (host:
/// `PENDING_APPROVAL_WAIT = 180 s`, `crates/slipstream-host/src/protocol/slipstream1.rs:757`),
/// and the host holds the connection open and silent for that whole window before it sends
/// `Welcome`. Silence past this point is therefore expected for a first pairing, not a fault.
const FIRST_OFFER_WAIT: Duration = Duration::from_secs(15);
/// Extra wait after `FIRST_OFFER_WAIT`, bringing the total ceiling to 210 s. Strictly greater
/// than the host's 180 s park plus room for the approval round trip, so a human who sees the
/// console row and clicks Approve is admitted on the SAME connection with no reconnect. A
/// genuinely wedged capture/encode pipeline still fails, just later (the Mac client's shipped
/// equivalent is 185 s, `SessionModel.swift:365`).
const APPROVAL_WAIT: Duration = Duration::from_secs(195);

/// The `Hello` this client sends, built from the parsed arguments.
///
/// Extracted so the handshake shape, the per-session `monitor_id` in particular, is exercised against
/// the construction the client actually uses instead of a copy in a test that can drift from it.
pub(crate) fn hello_for(args: &Args) -> Hello {
    Hello {
        abi_version: slipstream_core::WIRE_VERSION,
        mode: args.mode,
        compositor: slipstream_core::CompositorPref::Auto,
        gamepad: slipstream_core::config::GamepadPref::Auto,
        bitrate_kbps: args.bitrate_kbps,
        name: Some(args.name.clone()),
        launch: None,
        // Capability bits stay 0 until a decoder here advertises 10-bit/4:4:4 that it can really
        // render; claiming them early would have the host pick a mode this client cannot show.
        video_caps: 0,
        audio_channels: args.audio_channels,
        // Advertise only what the decode backend that will actually run can decode: a host picks
        // from this set, so a bit the backend refuses negotiates a stream the client cannot decode
        // (AV1 on the default ffmpeg backend was exactly that).
        video_codecs: hello_codecs(),
        preferred_codec: args.preferred_codec,
        display_hdr: slipstream_core::client::display_hdr_env_override(),
        client_caps: 0,
        // E1: additive per-session monitor choice. `--monitor` names a head; absent keeps the
        // host's pinned monitor, which is the legacy behaviour. The host validates the id against
        // its own monitor list, so an unknown head fails there with a clear message.
        monitor_id: args.monitor.clone(),
    }
}

/// Inputs needed to run one client session. The CLI has already parsed and resolved these.
#[derive(Debug)]
pub struct SessionConfig {
    pub remote: SocketAddr,
    pub args: Args,
    /// Input route created by the CLI boundary. `None` keeps headless/record-only runs quiet.
    pub input: Option<InputSender>,
    /// Receiver paired with `input`; handed to the QUIC datagram pump once admitted.
    pub input_rx: Option<UnboundedReceiver<InputEvent>>,
}

/// How a session ended. Every normal termination has a variant; unexpected I/O and protocol errors
/// remain `Err` so the CLI still reports them as failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOutcome {
    /// The peer ended a session that delivered at least one video AU.
    Ended,
    /// QUIC handshake, hello/welcome exchange or admission failed.
    ConnectFailed,
    /// The session was admitted but published no video AUs.
    ZeroFrames,
    /// The user closed the presentation window.
    WindowClosed,
    /// No frames arrived for the receive timeout.
    TimedOut,
}

impl SessionOutcome {
    /// Classify the result of a completed stream attempt.
    pub fn from_result(connected: bool, stop: SessionStop, frames: u64) -> Self {
        if !connected {
            return Self::ConnectFailed;
        }
        if frames > 0 {
            return match stop {
                SessionStop::Ended => Self::Ended,
                SessionStop::WindowClosed => Self::WindowClosed,
                SessionStop::TimedOut => Self::TimedOut,
            };
        }
        match stop {
            SessionStop::Ended => Self::ZeroFrames,
            SessionStop::WindowClosed => Self::WindowClosed,
            SessionStop::TimedOut => Self::TimedOut,
        }
    }
}

/// Why the receive loop stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStop {
    Ended,
    WindowClosed,
    TimedOut,
}

/// Show the host fingerprint and require an explicit `yes` before anything is streamed.
///
/// Fails closed. A non-interactive stdin is an error rather than an automatic accept, and a read
/// error, EOF, a bare newline or any answer other than `y`/`yes` is a refusal. There is no
/// "press enter to continue": a stray newline must never be enough to hand a host live keyboard
/// and mouse input.
fn confirm_fingerprint(host: &str, fp_hex: &str) -> Result<bool> {
    use std::io::{IsTerminal, Write};
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        return Err(anyhow!(
            "cannot confirm {host}'s fingerprint: stdin is not a terminal. Verify {fp_hex} out of \
             band and re-run with --trust {fp_hex}."
        ));
    }
    eprintln!("\n{host} presents certificate fingerprint:\n  {fp_hex}\n");
    eprint!("Trust this host and send it your keyboard and mouse? [y/N] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if stdin.read_line(&mut line).is_err() {
        return Ok(false);
    }
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

// Peer-controlled geometry is validated below before it sizes a decoder or window.
/// The largest geometry the wire contract allows. These mirror `ss-encode`'s
/// `Codec::max_dimension` (4096 for H.264, 8192 for HEVC/AV1) — the host refuses anything above it,
/// so the client accepts exactly that range. Kept as a local copy because the Windows client must
/// not depend on the encoder stack; if the host limits change, this is the line to update.
const MAX_DIM_H264: u32 = 4096;
const MAX_DIM_HEVC_AV1: u32 = 8192;
/// No current display drives more than this; a larger `refresh_hz` in an offer is corruption.
const MAX_REFRESH_HZ: u32 = 1000;

fn max_dimension(codec: u8) -> u32 {
    if codec == CODEC_H264 {
        MAX_DIM_H264
    } else {
        MAX_DIM_HEVC_AV1
    }
}

fn known_codec(codec: u8) -> bool {
    matches!(codec, CODEC_H264 | CODEC_HEVC | CODEC_AV1)
}

/// The decode backend this session will use, from the same env var `run_stream` reads.
///
/// Kept next to the advertisement so the two cannot drift: a codec bit in `Hello` is a promise the
/// client can decode that stream, and the promise is only good for the backend that will run.
fn uses_mf_backend() -> bool {
    std::env::var_os("SLIPSTREAM_WINDOWS_DECODER_MF").is_some()
}

/// Video codecs to advertise in `Hello`, derived from what the selected backend can decode.
///
/// This mirrors how the crate already decides codec support: each backend hard-codes its own
/// `match` over the negotiated byte (`ff` accepts H.264/HEVC, `mf` adds AV1), so the advertisement
/// asks the backend rather than assuming a fixed set. AV1 is special because its Media Foundation
/// decoder is an optional Windows extension: `av1_probe` reports whether this install has one, so
/// a machine without the AV1 Video Extension does not negotiate a stream it cannot decode.
fn advertised_codecs(mf: bool, av1_probe: impl Fn() -> bool) -> u8 {
    if !mf {
        // Ask the backend, not a copied constant: `ff::supported_codecs` is the same list its
        // `spawn` will accept, so the two cannot drift apart.
        return ff::supported_codecs();
    }
    let mut codecs = CODEC_H264 | CODEC_HEVC;
    if av1_probe() {
        codecs |= CODEC_AV1;
    }
    codecs
}

/// The advertisement for this process: the default `ffmpeg.exe` backend has no AV1 demuxer
/// mapping, so AV1 is not advertised unless the Media Foundation backend (probed) is selected.
fn hello_codecs() -> u8 {
    if uses_mf_backend() {
        advertised_codecs(true, mf::supports_av1)
    } else {
        advertised_codecs(false, || false)
    }
}

/// Reject a mode that cannot be a real encoded stream *before* anything is sized from it.
///
/// `Welcome.mode` and `Reconfigured.mode` are decoded from bytes the peer controls: a corrupt or
/// hostile host could otherwise make the client allocate a decoder and a window for a 0x0, odd, or
/// absurd geometry. The dims are `u32` on the wire, so there is no non-finite value to reject; the
/// failure modes are zero, odd (4:2:0 chroma is subsampled by two on both axes), and out of range.
/// The message names the source (`Welcome` vs `Reconfigured`) so an operator can tell a broken host
/// from a corrupt packet, and the codec byte is validated here too because it selects the limit.
fn validate_mode(mode: &Mode, codec: u8, what: &str) -> Result<()> {
    if !known_codec(codec) {
        bail!("{what}: unknown negotiated codec byte {codec} (expected H.264, HEVC or AV1)");
    }
    if mode.width == 0 || mode.height == 0 {
        bail!(
            "{what}: mode {}x{}@{} has a zero dimension",
            mode.width,
            mode.height,
            mode.refresh_hz
        );
    }
    if !mode.width.is_multiple_of(2) || !mode.height.is_multiple_of(2) {
        bail!(
            "{what}: mode {}x{} is odd; 4:2:0 chroma needs even dimensions",
            mode.width,
            mode.height
        );
    }
    let max = max_dimension(codec);
    if mode.width > max || mode.height > max {
        bail!(
            "{what}: mode {}x{} exceeds the {max}px limit for codec byte {codec}",
            mode.width,
            mode.height
        );
    }
    if mode.refresh_hz == 0 || mode.refresh_hz > MAX_REFRESH_HZ {
        bail!(
            "{what}: refresh {} Hz is outside 1..={MAX_REFRESH_HZ}",
            mode.refresh_hz
        );
    }
    Ok(())
}

/// Feed one decode-loop event to the recovery policy.
///
/// Events alone never make a request due: each arm resets the timer its own request would fire
/// from, so the value is intentionally dropped and requests are polled from the no-frame arm.
fn note_recovery(recovery: &mut RecoveryState, event: RecoveryEvent) {
    let due = recovery.advance(Some(event), Instant::now());
    debug_assert!(
        due.is_none(),
        "recovery events never make a keyframe request due on their own"
    );
}

/// Run one streaming session to completion, from handshake to final counters.
pub async fn run_stream(cfg: SessionConfig) -> Result<SessionOutcome> {
    let SessionConfig {
        remote,
        args,
        input,
        input_rx,
    } = cfg;
    let (cert, key) = identity::load_or_create()?;
    let (ep, observed) =
        endpoint::client_pinned_with_identity(args.pin, Some((cert.as_str(), key.as_str())));
    let ep = ep.map_err(|e| anyhow!("QUIC client endpoint: {e}"))?;
    let conn = ep
        .connect(remote, "slipstream")
        .context("connect")?
        .await
        .context("QUIC handshake (a pin mismatch fails here)")?;
    match (args.pin, *observed.lock().unwrap()) {
        (Some(_), _) => tracing::info!(%remote, "connected — host fingerprint pinned"),
        (None, Some(fp)) => tracing::info!(
            %remote,
            fingerprint = %hex(&fp),
            "connected (trust-on-first-use) — pass --pin to verify this host from now on"
        ),
        (None, None) => tracing::info!(%remote, "connected"),
    }

    let (mut send, recv) = conn.open_bi().await.context("open control stream")?;
    let mut recv = io::MsgReader::new(recv);
    io::write_msg(&mut send, &hello_for(&args).encode()).await?;
    // Tell the truth while waiting: 15 s of silence usually means the host parked this device for
    // approval, so name that likely cause and keep waiting past the host's own park window. A
    // genuinely stalled capture/encode pipeline still surfaces the same diagnosis, just later.
    let welcome_msg = match tokio::time::timeout(FIRST_OFFER_WAIT, recv.read_msg()).await {
        Ok(msg) => msg?,
        Err(_) => {
            tracing::warn!(
                host = %remote,
                wait_secs = APPROVAL_WAIT.as_secs(),
                "no session offer after 15 s - the host has parked this device for approval; \
                 approve it in the host console - still waiting"
            );
            tokio::time::timeout(APPROVAL_WAIT, recv.read_msg())
                .await
                .map_err(|_| {
                    anyhow!(
                        "host {remote} accepted the QUIC handshake but sent no session offer \
                         within 210 s (the host's capture/encode pipeline is stalled, or this \
                         device's pairing request was never approved in the host console)"
                    )
                })??
        }
    };
    let welcome = Welcome::decode(&welcome_msg).map_err(|e| anyhow!("Welcome decode: {e:?}"))?;
    // The offer is peer-controlled bytes: reject an unusable codec byte or geometry before any
    // decoder, window or frame buffer is sized from it.
    validate_mode(&welcome.mode, welcome.codec, "host Welcome")?;
    tracing::info!(
        mode = ?welcome.mode,
        frames = welcome.frames,
        codec = welcome.codec,
        bit_depth = welcome.bit_depth,
        encrypt = welcome.encrypt,
        udp_port = welcome.udp_port,
        "session offer"
    );

    // Admission, before a single byte of video is consumed. `connect::decide` is the only authority
    // for this: this CLI takes a manual `ip:port`, which has no advert, so the certificate the host
    // presents is the sole anchor and it must pass the confirmation card below.
    let mut machine = Machine::new();
    machine
        .connect(&remote.to_string())
        .map_err(|e| anyhow!(e))?;
    let observed_fp = *observed.lock().unwrap();
    let admission = connect::decide(args.pin.is_some(), None, true);
    let host_label = remote.to_string();
    match machine.admitted(admission) {
        connect::Transition::Stay(_, msg) => tracing::info!(%msg, "admission"),
        connect::Transition::DropAndIdle(msg) => {
            conn.close(0u32.into(), b"refused");
            return Err(anyhow!(msg));
        }
    }

    // The trust gate. With no pin, core accepts *any* host certificate by design, so this is the
    // only place the decision is enforced: nothing streams, and `can_send_input()` stays false,
    // until the operator confirms the fingerprint they were shown.
    if machine.awaiting_trust() {
        let fp = observed_fp.ok_or_else(|| {
            anyhow!("cannot confirm trust: the host presented no certificate fingerprint")
        })?;
        let fp_hex = hex(&fp);
        let confirmed = match args.trust {
            Some(want) => {
                if want != fp {
                    tracing::warn!(
                        %remote,
                        expected = %hex(&want),
                        presented = %fp_hex,
                        "--trust fingerprint does not match the host"
                    );
                }
                want == fp
            }
            None => confirm_fingerprint(&host_label, &fp_hex)?,
        };
        if !confirmed {
            conn.close(0u32.into(), b"trust declined");
            return Err(anyhow!(
                "{host_label} presents fingerprint {fp_hex}, which was not confirmed - refusing to \
                 stream. Verify it out of band, then re-run with --trust {fp_hex} (or --pin {fp_hex} \
                 to pin it permanently)."
            ));
        }
        machine.trusted().map_err(|e| anyhow!(e))?;
        tracing::info!(%remote, fingerprint = %fp_hex, "host fingerprint confirmed");
    }

    // Admission is the first gate: input events before this point are never queued. The second
    // gate is window focus, maintained by WM_ACTIVATE/WM_SETFOCUS/WM_KILLFOCUS in `window.rs`.
    let _input_guard = input.as_ref().map(|s| {
        s.set_enabled(true);
        // Held-input releases go straight to the connection from here on. The pump task is
        // cancelled when the session ends, which is exactly when a stuck key has to be freed, so a
        // release must not depend on it.
        s.attach_release_route(conn.clone());
        InputSenderGuard::new(s.clone())
    });
    if let (Some(sender), Some(rx)) = (&input, input_rx) {
        let pump_conn = conn.clone();
        let pump_sender = sender.clone();
        tokio::spawn(async move { pump_sender.pump(rx, pump_conn).await });
    }

    // Reserve our data-plane port, then let the host start.
    let probe = std::net::UdpSocket::bind("0.0.0.0:0")?;
    let udp_port = probe.local_addr()?.port();
    drop(probe);
    io::write_msg(
        &mut send,
        &Start {
            client_udp_port: udp_port,
        }
        .encode(),
    )
    .await?;

    // The host encodes an infinite GOP, so a lost opening IDR (or a restarted decoder replaying
    // only deltas) leaves the picture black forever. `RequestKeyframe` is the protocol's recovery
    // message; the policy in [`crate::recovery`] decides when it is due. The write itself happens
    // on this control-stream task, never inside the blocking frame loop: `Handle::block_on` from
    // the loop panics when the thread is already driving a future (the bug 5352687c fixes).
    let (keyframe_tx, mut keyframe_rx) = tokio::sync::mpsc::unbounded_channel::<KeyframeRequest>();
    // Written-request acks, drained by the frame loop so `mark_sent` counts real writes only.
    let (keyframe_sent_tx, keyframe_sent_rx) = std::sync::mpsc::channel::<()>();
    let keyframe_writer = {
        let mut send = send;
        tokio::spawn(async move {
            while let Some(request) = keyframe_rx.recv().await {
                match io::write_msg(&mut send, &RequestKeyframe.encode()).await {
                    Ok(()) => {
                        tracing::warn!(
                            reason = request.reason.as_str(),
                            attempted = request.attempted,
                            "requested a keyframe — no decodable picture"
                        );
                        let _ = keyframe_sent_tx.send(());
                    }
                    Err(e) => tracing::warn!(
                        reason = request.reason.as_str(),
                        attempted = request.attempted,
                        error = %e,
                        "keyframe request failed"
                    ),
                }
            }
        })
    };

    let host_udp = SocketAddr::new(remote.ip(), welcome.udp_port);
    let session_cfg = welcome.session_config(Role::Client);
    let expected = welcome.frames;
    let codec = welcome.codec;
    let mode = welcome.mode;
    let record = args.record.clone();
    let seconds = args.seconds;

    // The host may refuse the mode we asked for and send `Reconfigured` before the first AU.
    // Keep reading the control stream so that correction is not lost.
    let delivered: std::sync::Arc<std::sync::Mutex<Option<slipstream_core::config::Mode>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    {
        let delivered = delivered.clone();
        let mut ctl = recv;
        tokio::spawn(async move {
            while let Ok(msg) = ctl.read_msg().await {
                match slipstream_core::quic::Reconfigured::decode(&msg) {
                    Ok(rc) => {
                        tracing::info!(
                            accepted = rc.accepted,
                            width = rc.mode.width,
                            height = rc.mode.height,
                            refresh_hz = rc.mode.refresh_hz,
                            "host corrected the session mode"
                        );
                        match validate_mode(&rc.mode, codec, "host Reconfigured") {
                            Ok(()) => *delivered.lock().unwrap() = Some(rc.mode),
                            Err(e) => {
                                tracing::warn!(error = %e, "ignoring a Reconfigured with an unusable mode")
                            }
                        }
                    }
                    Err(_) => tracing::debug!(len = msg.len(), "control message (not a mode ack)"),
                }
            }
        });
    }

    let started = Instant::now();
    let (frames, bytes, presented, stop) =
        tokio::task::spawn_blocking(move || -> Result<(u64, u64, u64, SessionStop)> {
        let transport = UdpTransport::connect(&format!("0.0.0.0:{udp_port}"), &host_udp.to_string())
            .context("bind data plane")?;
        let mut session =
            Session::new(session_cfg, Box::new(transport)).map_err(|e| anyhow!("client session: {e:?}"))?;
        let mut sink = match &record {
            Some(p) => Some(std::io::BufWriter::new(
                std::fs::File::create(p).with_context(|| format!("create {p}"))?,
            )),
            None => None,
        };

        // Decode + present. `SLIPSTREAM_WINDOWS_HEADLESS=1` keeps the old counting path (CI, and
        // `--record`-only runs). Otherwise: Media Foundation → Win32 window. A window that cannot
        // be created is reported and the session still runs headless — never a silent black frame.
        let headless = std::env::var_os("SLIPSTREAM_WINDOWS_HEADLESS").is_some();
        let (mut decoder, mut presenter): (Box<dyn Decoder>, Box<dyn Presenter>) = if headless {
            (
                Box::new(PassthroughDecoder::default()),
                Box::new(ConsolePresenter::default()),
            )
        } else {
            // Default decode is the `ffmpeg.exe` child (see [`ff`]); the Media Foundation MFT is
            // opt-in because the Microsoft HEVC decoder is an async DXVA MFT that needs a D3D11
            // device manager before it emits anything.
            let use_mf = uses_mf_backend();
            let dec: Box<dyn Decoder> = if use_mf {
                Box::new(mf::MfDecoder::new(mode.width, mode.height, codec)?)
            } else {
                Box::new(ff::FfmpegDecoder::new(mode.width, mode.height, codec)?)
            };
            let pres: Box<dyn Presenter> = match window::GdiPresenter::new(
                &format!("slipstream — {host_label}"),
                mode.width,
                mode.height,
                input.clone(),
            ) {
                Ok(p) => Box::new(p),
                Err(e) => {
                    tracing::warn!(error = %format!("{e:#}"), "no window — presenting headless");
                    Box::new(ConsolePresenter::default())
                }
            };
            (dec, pres)
        };
        presenter.status(&format!("receiving from {host_label}"));
        let (mut frames, mut bytes) = (0u64, 0u64);
        let mut presented = 0u64;
        let mut stats = PipeStats::default();
        let mut last_rx = Instant::now();
        // The geometry we are currently sized for; starts at what the Welcome promised and moves
        // to whatever the host says it is actually delivering.
        let mut sized = (mode.width, mode.height);
        let mut recovery = RecoveryState::default();
        let stop;
        loop {
            // A due request counts as sent only once the control-stream task wrote it.
            while keyframe_sent_rx.try_recv().is_ok() {
                recovery.mark_sent();
            }
            if presenter.closed() {
                tracing::info!("window closed — ending the session");
                stop = SessionStop::WindowClosed;
                break;
            }
            if let Some(m) = delivered.lock().unwrap().take() {
                if let Err(e) = validate_mode(&m, codec, "delivered mode") {
                    tracing::warn!(error = %e, "not adopting an invalid delivered mode");
                } else if (m.width, m.height) != sized {
                    tracing::info!(
                        from_w = sized.0,
                        from_h = sized.1,
                        to_w = m.width,
                        to_h = m.height,
                        refresh_hz = m.refresh_hz,
                        "adopting the delivered mode (requested mode was refused)"
                    );
                    sized = (m.width, m.height);
                    decoder.resize(m.width, m.height);
                    presenter.resize(m.width, m.height);
                    // The replacement decoder has no reference picture of its own, even if the
                    // old one had produced frames: it needs a fresh IDR to produce anything.
                    note_recovery(&mut recovery, RecoveryEvent::DecoderRestart);
                }
            }
            if let Some(secs) = seconds {
                if started.elapsed() > Duration::from_secs(secs) {
                    stop = SessionStop::Ended;
                    break;
                }
            } else if last_rx.elapsed() > Duration::from_secs(45) {
                tracing::warn!("no frames for 45 s — ending the session");
                stop = SessionStop::TimedOut;
                break;
            }
            if expected > 0 && frames >= expected as u64 {
                stop = SessionStop::Ended;
                break;
            }
            let t_poll = Instant::now();
            let polled = session.poll_frame();
            let poll_ns = t_poll.elapsed().as_nanos() as u64;
            let mut spin_ns = 0u64;
            match polled {
                Ok(frame) => {
                    last_rx = Instant::now();
                    if frame.flags & FLAG_PROBE as u32 != 0 {
                        continue; // speed-test filler, not video
                    }
                    frames += 1;
                    stats.note_au();
                    bytes += frame.data.len() as u64;
                    if let Some(s) = sink.as_mut() {
                        s.write_all(&frame.data).context("write AU")?;
                    }
                    let t_dec = Instant::now();
                    let decoded = decoder.decode(&frame.data, codec);
                    let decode_ns = t_dec.elapsed().as_nanos() as u64;
                    if decoded.is_some() {
                        stats.note_decoded();
                        presented += 1;
                        stats.note_presented();
                        note_recovery(&mut recovery, RecoveryEvent::Output);
                    } else {
                        // An AU swallowed while the decoder is still unanchored is a stale
                        // reference or decode error; the policy re-asks after its own patience.
                        note_recovery(&mut recovery, RecoveryEvent::DecodeError);
                    }
                    let t_pres = Instant::now();
                    presenter.present(decoded.as_ref());
                    let present_ns = t_pres.elapsed().as_nanos() as u64;
                    let (qm, bm) = decoder.flow_stats();
                    stats.queue_max = qm;
                    stats.blocked_ms = bm;
                    stats.timed += 1;
                    stats.note_costs(
                        poll_ns,
                        spin_ns,
                        decode_ns,
                        present_ns,
                        presenter.stage_ns(),
                    );
                    stats.tick();
                }
                Err(SlipstreamError::NoFrame) => {
                    // No picture and nothing to decode: ask for a fresh IDR if the policy is due.
                    // Sending is non-blocking; the writer task reports the write back above.
                    if let Some(request) = recovery.advance(None, Instant::now()) {
                        tracing::debug!(
                            reason = request.reason.as_str(),
                            attempted = recovery.attempted(),
                            sent = recovery.sent(),
                            "keyframe recovery is due — queued for the control stream"
                        );
                        let _ = keyframe_tx.send(request);
                    }
                    let t_spin = Instant::now();
                    std::thread::sleep(Duration::from_micros(300));
                    spin_ns = t_spin.elapsed().as_nanos() as u64;
                    stats.spins += 1;
                    // Poll+spin without a frame still costs the same wall time; attribute it to
                    // the next frame's window so the split stays per-frame.
                    stats.poll_ns += poll_ns;
                    stats.spin_ns += spin_ns;
                }
                Err(e) => return Err(anyhow!("poll_frame: {e:?}")),
            }
        }
        if let Some(mut s) = sink {
            s.flush().ok();
        }
        Ok((frames, bytes, presented, stop))
    })
    .await??;

    // The frame loop ended and dropped its sender; let the control-stream task finish so its
    // `send` handle is released before the connection closes.
    let _ = keyframe_writer.await;

    conn.close(0u32.into(), b"quit");
    println!(
        "session ended: received {frames} video AUs, presented {presented} frames, {bytes} bytes in {:.1}s ({:.1} fps presented)",
        started.elapsed().as_secs_f64(),
        presented as f64 / started.elapsed().as_secs_f64().max(0.001),
    );
    Ok(SessionOutcome::from_result(true, stop, frames))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors the host's park window (`PENDING_APPROVAL_WAIT`,
    /// `crates/slipstream-host/src/protocol/slipstream1.rs:757`). If the host widens its park, the
    /// client ceiling must move with it or a first pairing becomes unapprovable by hand again.
    const HOST_PENDING_APPROVAL_WAIT: Duration = Duration::from_secs(180);
    /// An unpaired device's first knock is held silent host-side, so the client must keep the
    /// connection past the park instead of failing at the first 15 s of silence.
    #[test]
    fn offer_wait_outlasts_the_hosts_pairing_park() {
        let total = FIRST_OFFER_WAIT + APPROVAL_WAIT;
        assert!(
            total > HOST_PENDING_APPROVAL_WAIT + Duration::from_secs(15),
            "client gives up at {total:?}, not comfortably past the host's {HOST_PENDING_APPROVAL_WAIT:?} park"
        );
        assert!(
            FIRST_OFFER_WAIT < APPROVAL_WAIT,
            "the narrated wait must be shorter than the extra grace it announces"
        );
    }

    /// The bug this guards: `Hello` used to advertise AV1 unconditionally while the default
    /// `ffmpeg.exe` backend refuses it, so a host could pick a codec the client cannot decode.
    #[test]
    fn the_default_ffmpeg_backend_does_not_advertise_av1() {
        let caps = advertised_codecs(false, || true);
        assert_eq!(
            caps & CODEC_AV1,
            0,
            "AV1 must not be advertised when the ffmpeg backend cannot decode it"
        );
        assert_eq!(caps & CODEC_H264, CODEC_H264);
        assert_eq!(caps & CODEC_HEVC, CODEC_HEVC);
        assert_eq!(
            caps,
            ff::supported_codecs(),
            "the advertisement must be exactly what the backend accepts"
        );
    }

    /// Media Foundation can decode AV1 only when the AV1 Video Extension is installed, so the
    /// advertisement follows the probe instead of assuming the codec exists.
    #[test]
    fn the_mf_backend_advertises_av1_only_when_it_is_installed() {
        assert_eq!(
            advertised_codecs(true, || false) & CODEC_AV1,
            0,
            "no AV1 decoder means no AV1 in the offer"
        );
        assert_ne!(
            advertised_codecs(true, || true) & CODEC_AV1,
            0,
            "an installed AV1 decoder means AV1 is offered"
        );
        for caps in [
            advertised_codecs(true, || false),
            advertised_codecs(true, || true),
        ] {
            assert_eq!(caps & CODEC_H264, CODEC_H264);
            assert_eq!(caps & CODEC_HEVC, CODEC_HEVC);
        }
    }

    #[test]
    fn zero_frames_maps_to_zero_frames() {
        assert_eq!(
            SessionOutcome::from_result(true, SessionStop::Ended, 0),
            SessionOutcome::ZeroFrames
        );
    }

    #[test]
    fn clean_end_with_frames_maps_to_ended() {
        assert_eq!(
            SessionOutcome::from_result(true, SessionStop::Ended, 1),
            SessionOutcome::Ended
        );
    }

    #[test]
    fn external_stops_keep_their_cause() {
        assert_eq!(
            SessionOutcome::from_result(true, SessionStop::WindowClosed, 0),
            SessionOutcome::WindowClosed
        );
        assert_eq!(
            SessionOutcome::from_result(true, SessionStop::TimedOut, 10),
            SessionOutcome::TimedOut
        );
        assert_eq!(
            SessionOutcome::from_result(true, SessionStop::WindowClosed, 10),
            SessionOutcome::WindowClosed
        );
    }

    /// The client's real handshake message, not a copy of it: `--monitor` must survive the same codec
    /// the host decodes with. A real Windows head is `gdi:\\.\DISPLAY1`, so the backslashes are the
    /// interesting part: they must arrive intact rather than being stripped or shifted.
    #[test]
    fn monitor_id_reaches_the_wire_and_survives_the_codec() {
        let p = |argv: &[&str]| {
            crate::args::parse(argv.iter().map(|s| s.to_string())).expect("args parse")
        };

        const HEAD: &str = r"gdi:\\.\DISPLAY1";
        let hello = hello_for(&p(&["--connect", "h", "--monitor", HEAD]));
        assert_eq!(hello.monitor_id.as_deref(), Some(HEAD));

        // Round-trip through the shared codec the host uses to decode this message.
        let bytes = hello.encode();
        let decoded = Hello::decode(&bytes).expect("hello decodes");
        assert_eq!(
            decoded.monitor_id.as_deref(),
            Some(HEAD),
            "the head id must arrive intact (wire {} bytes, caps {}, hdr {})",
            bytes.len(),
            decoded.client_caps,
            decoded.display_hdr.is_some()
        );

        // Absent stays absent, which is what keeps the legacy wire shape for existing callers.
        let plain = hello_for(&p(&["--connect", "h"]));
        assert_eq!(plain.monitor_id, None);
        assert_eq!(
            Hello::decode(&plain.encode())
                .expect("hello decodes")
                .monitor_id,
            None
        );
    }

    fn mode(w: u32, h: u32, hz: u32) -> Mode {
        Mode {
            width: w,
            height: h,
            refresh_hz: hz,
        }
    }

    #[test]
    fn a_normal_welcome_mode_is_accepted() {
        assert!(validate_mode(&mode(2560, 1440, 240), CODEC_HEVC, "Welcome").is_ok());
        assert!(validate_mode(&mode(1920, 1080, 144), CODEC_H264, "Welcome").is_ok());
        // 8192 is the HEVC/AV1 limit and is inclusive; 4K is fine for H.264.
        assert!(validate_mode(&mode(8192, 8192, 60), CODEC_AV1, "Welcome").is_ok());
        assert!(validate_mode(&mode(4096, 2160, 60), CODEC_H264, "Welcome").is_ok());
    }

    #[test]
    fn zero_geometry_is_rejected() {
        for (w, h) in [(0, 1080), (1920, 0), (0, 0)] {
            let err = validate_mode(&mode(w, h, 60), CODEC_HEVC, "Welcome").unwrap_err();
            assert!(err.to_string().contains("zero dimension"), "{err}");
            assert!(err.to_string().contains("Welcome"), "{err}");
        }
    }

    #[test]
    fn odd_geometry_is_rejected() {
        for (w, h) in [(1921, 1080), (1920, 1081)] {
            let err = validate_mode(&mode(w, h, 60), CODEC_HEVC, "Welcome").unwrap_err();
            assert!(err.to_string().contains("odd"), "{err}");
        }
    }

    #[test]
    fn absurd_geometry_is_rejected_per_codec_limit() {
        // HEVC/AV1 accept up to 8192, H.264 only to 4096.
        assert!(validate_mode(&mode(8193, 2160, 60), CODEC_HEVC, "Welcome").is_err());
        let err = validate_mode(&mode(8192, 2160, 60), CODEC_H264, "Welcome").unwrap_err();
        assert!(err.to_string().contains("4096"), "{err}");
        // u32::MAX is decoded from four peer-controlled bytes; it must not size a buffer.
        assert!(validate_mode(&mode(u32::MAX, u32::MAX, 60), CODEC_AV1, "Welcome").is_err());
    }

    #[test]
    fn out_of_range_refresh_is_rejected() {
        assert!(validate_mode(&mode(1920, 1080, 0), CODEC_HEVC, "Welcome").is_err());
        assert!(
            validate_mode(&mode(1920, 1080, MAX_REFRESH_HZ + 1), CODEC_HEVC, "Welcome").is_err()
        );
        assert!(validate_mode(&mode(1920, 1080, MAX_REFRESH_HZ), CODEC_HEVC, "Welcome").is_ok());
    }

    #[test]
    fn unknown_codec_byte_is_rejected() {
        let err = validate_mode(&mode(1920, 1080, 60), 0x7f, "Welcome").unwrap_err();
        assert!(
            err.to_string().contains("unknown negotiated codec byte"),
            "{err}"
        );
    }

    #[test]
    fn reconfigured_mode_rejection_names_its_source() {
        let err = validate_mode(&mode(0, 0, 0), CODEC_HEVC, "host Reconfigured").unwrap_err();
        assert!(err.to_string().contains("host Reconfigured"), "{err}");
    }

    #[test]
    fn connect_failed_maps_to_connect_failed() {
        assert_eq!(
            SessionOutcome::from_result(false, SessionStop::Ended, 0),
            SessionOutcome::ConnectFailed
        );
    }
}
