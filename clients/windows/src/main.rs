//! `slipstream-windows` — the Windows session client.
//!
//! Today this binary is the **transport + admission + present-seam** half of the client: it
//! discovers `_slipstream._udp` hosts, honours the `pair=required` advert rule, runs the same PIN
//! pairing ceremony as every sibling client, connects over QUIC, negotiates a mode/codec, receives
//! every access unit of the video plane, and can record them to a playable elementary stream.
//!
//! The window and the hardware decode path attach at the two traits in [`present`] — see PR.md for
//! the exact anchors and why they are not in this change (no FFmpeg development SDK on the build
//! box, and `ss-presenter` is Vulkan/Linux-only). Everything else here is real: no stubs in the
//! connect path.
//!
//! Decode is Media Foundation (the OS HEVC/H.264 decoder MFTs) and presentation is a Win32 window
//! with a GDI blit: [`mf`] and [`window`]. Those two modules hold the only `unsafe` in the crate.
#![deny(unsafe_code)]

mod args;
mod connect;
mod discovery;
mod ff;
mod identity;
mod mf;
mod present;
mod window;

use anyhow::{anyhow, Context, Result};
use args::{hex, Action, Args, Target};
use connect::{Admission, Machine};
use present::{ConsolePresenter, Decoder, PassthroughDecoder, Presenter};
use std::io::Write;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use slipstream_core::config::Role;
use slipstream_core::packet::FLAG_PROBE;
use slipstream_core::quic::{endpoint, io, Hello, Start, Welcome};
use slipstream_core::transport::UdpTransport;
use slipstream_core::{CompositorPref, Session, SlipstreamError};

/// Pipeline counters, printed once a second: AUs received → decoded → presented, plus the interval
/// between consecutive presented frames. The interval distribution is the point: "steady low fps"
/// and "mostly fast with periodic 200-400 ms hitches" are different bugs with different fixes.
#[derive(Default)]
struct PipeStats {
    aus: u64,
    decoded: u64,
    presented: u64,
    /// Presented-frame intervals (µs) within the current one-second window.
    intervals: Vec<u32>,
    last_present: Option<Instant>,
    window: Option<Instant>,
    /// Highest frame-queue depth the decoder reported (flow control, bounded).
    queue_max: usize,
    /// Milliseconds blocked writing an AU because the queue was full.
    blocked_ms: u64,
}

impl PipeStats {
    fn note_au(&mut self) {
        self.aus += 1;
    }

    fn note_decoded(&mut self) {
        self.decoded += 1;
    }

    fn note_presented(&mut self) {
        self.presented += 1;
        let now = Instant::now();
        if let Some(prev) = self.last_present {
            self.intervals
                .push(now.duration_since(prev).as_micros().min(u32::MAX as u128) as u32);
        }
        self.last_present = Some(now);
    }

    /// Print and reset the one-second window. Returns nothing; the numbers are the record.
    fn tick(&mut self) {
        let now = Instant::now();
        let start = *self.window.get_or_insert(now);
        if now.duration_since(start) < Duration::from_secs(1) {
            return;
        }
        let mut iv = std::mem::take(&mut self.intervals);
        iv.sort_unstable();
        let pick = |q: f64| -> f64 {
            if iv.is_empty() {
                return 0.0;
            }
            let i = ((iv.len() - 1) as f64 * q) as usize;
            iv[i] as f64 / 1000.0
        };
        tracing::info!(
            received = self.aus,
            decoded = self.decoded,
            presented = self.presented,
            ivl_p50_ms = format!("{:.1}", pick(0.50)),
            ivl_p95_ms = format!("{:.1}", pick(0.95)),
            ivl_p99_ms = format!("{:.1}", pick(0.99)),
            ivl_max_ms = format!("{:.1}", pick(1.0)),
            n_ivl = iv.len(),
            queue_max = self.queue_max,
            blocked_ms = self.blocked_ms,
            "pipeline received -> decoded -> presented"
        );
        self.window = Some(now);
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = match args::parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(args) {
        tracing::error!("{e:#}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<()> {
    match args.action() {
        None => {
            println!("{}", args::usage());
            Ok(())
        }
        Some(Action::Discover(secs)) => discovery::run(secs),
        Some(Action::Pair) => pair(&args),
        Some(Action::Play) => play(&args),
        Some(Action::Connect) => stream(args),
    }
}

/// `--play FILE`: decode a recorded Annex-B stream into the window with no network.
///
/// This is the decode+present stack in isolation, which is exactly what a live-session run cannot
/// separate from host/network state.
fn play(args: &Args) -> Result<()> {
    let path = args.play.as_deref().context("--play needs a file")?;
    let data = std::fs::read(path).with_context(|| format!("read {path}"))?;
    let codec = if args.preferred_codec == 0 {
        slipstream_core::quic::CODEC_HEVC
    } else {
        args.preferred_codec
    };
    let mut decoder = ff::FfmpegDecoder::new(args.mode.width, args.mode.height, codec)?;
    let mut presenter =
        window::GdiPresenter::new(&format!("slipstream — {path}"), args.mode.width, args.mode.height)
            .context("create window")?;
    let aus = split_aus(&data);
    if aus.is_empty() {
        return Err(anyhow!("{path}: no Annex-B access units found"));
    }
    let started = Instant::now();
    let mut presented = 0u64;
    let mut stats = PipeStats::default();
    let frame_gap = Duration::from_micros(1_000_000 / args.mode.refresh_hz.max(1) as u64);
    for au in &aus {
        if presenter.closed() {
            break;
        }
        if let Some(s) = args.seconds {
            if started.elapsed() > Duration::from_secs(s) {
                break;
            }
        }
        stats.note_au();
        match decoder.decode_one(au)? {
            Some(f) => {
                stats.note_decoded();
                presented += 1;
                stats.note_presented();
                presenter.present(Some(&f));
            }
            None => presenter.present(None),
        }
        stats.tick();
        // Pace to the nominal refresh so the run is watchable and the counter is time-bounded.
        std::thread::sleep(frame_gap);
    }
    // Drain what the decoder still holds once its input closes.
    for f in decoder.finish() {
        if presenter.closed() {
            break;
        }
        stats.note_decoded();
        stats.note_presented();
        presented += 1;
        presenter.present(Some(&f));
    }
    let secs = started.elapsed().as_secs_f64();
    println!(
        "played {} AUs from {path}: presented {presented} frames in {secs:.1}s ({:.1} fps presented)",
        aus.len(),
        presented as f64 / secs.max(0.001)
    );
    Ok(())
}

/// Split an Annex-B elementary stream into access units at AUD NALs (HEVC type 35, H.264 type 9).
/// A stream without AUDs falls back to one NAL per unit, which is what the encoder block emits.
fn split_aus(data: &[u8]) -> Vec<Vec<u8>> {
    // Start-code offsets.
    let mut starts = Vec::new();
    let mut i = 0usize;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            let pos = if i > 0 && data[i - 1] == 0 { i - 1 } else { i };
            starts.push((pos, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    let nal_type = |body: usize| -> u8 {
        data.get(body).map(|b| (b >> 1) & 0x3F).unwrap_or(0)
    };
    let auds: Vec<usize> = (0..starts.len())
        .filter(|&k| nal_type(starts[k].1) == 35)
        .collect();
    let boundaries: Vec<usize> = if auds.len() > 1 {
        auds.iter().map(|&k| starts[k].0).collect()
    } else {
        starts.iter().map(|&(pos, _)| pos).collect()
    };
    boundaries
        .iter()
        .enumerate()
        .filter_map(|(k, &start)| {
            let end = boundaries.get(k + 1).copied().unwrap_or(data.len());
            (end > start).then(|| data[start..end].to_vec())
        })
        .collect()
}

fn target_of(args: &Args) -> Result<Target> {
    let raw = args
        .connect
        .as_deref()
        .context("no host given (pass HOST[:PORT] or --discover)")?;
    Target::parse(raw).map_err(|e| anyhow!(e))
}

fn resolve(t: &Target) -> Result<SocketAddr> {
    use std::net::ToSocketAddrs;
    let addrs: Vec<SocketAddr> = (t.host.as_str(), t.port)
        .to_socket_addrs()
        .with_context(|| format!("resolve {}:{}", t.host, t.port))?
        .collect();
    // Prefer IPv4: the host's data plane is UDP over v4 today.
    addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or_else(|| addrs.first())
        .copied()
        .ok_or_else(|| anyhow!("{}:{} resolved to no addresses", t.host, t.port))
}

/// `--pair PIN`: the SPAKE2 ceremony. On success the host trusts this client's certificate, so
/// later connects are admitted without a PIN — only the fingerprint pin matters.
fn pair(args: &Args) -> Result<()> {
    let addr = resolve(&target_of(args)?)?;
    let pin = args.pair.as_deref().context("--pair needs a PIN")?;
    let (cert, key) = identity::load_or_create()?;
    let fp = slipstream_core::client::NativeClient::pair(
        &addr.ip().to_string(),
        addr.port(),
        (&cert, &key),
        pin,
        &args.name,
        Duration::from_secs(90),
    )
    .map_err(|e| anyhow!("pairing failed: {e:?} (wrong PIN? is pairing armed on the host?)"))?;
    println!("PAIRED — host fingerprint {}", hex(&fp));
    println!("connect with: slipstream-windows {addr} --pin {}", hex(&fp));
    Ok(())
}

fn stream(args: Args) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    rt.block_on(stream_async(args))
}

async fn stream_async(args: Args) -> Result<()> {
    let remote = resolve(&target_of(&args)?)?;
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
    io::write_msg(
        &mut send,
        &Hello {
            abi_version: slipstream_core::WIRE_VERSION,
            mode: args.mode,
            compositor: CompositorPref::Auto,
            gamepad: slipstream_core::config::GamepadPref::Auto,
            bitrate_kbps: args.bitrate_kbps,
            name: Some(args.name.clone()),
            launch: None,
            // No decode backend yet: advertise no codec capability bits rather than claiming
            // 10-bit/4:4:4 we cannot render.
            video_caps: 0,
            audio_channels: args.audio_channels,
            video_codecs: slipstream_core::quic::CODEC_H264
                | slipstream_core::quic::CODEC_HEVC
                | slipstream_core::quic::CODEC_AV1,
            preferred_codec: args.preferred_codec,
            display_hdr: slipstream_core::client::display_hdr_env_override(),
            client_caps: 0,
        }
        .encode(),
    )
    .await?;
    // A host that completes the QUIC handshake but never sends `Welcome` (wedged encoder/parked
    // session) must fail, not hang the client forever.
    let welcome_msg = tokio::time::timeout(Duration::from_secs(15), recv.read_msg())
        .await
        .map_err(|_| {
            anyhow!(
                "host {remote} accepted the QUIC handshake but sent no session offer within 15 s \
                 (the host's capture/encode pipeline is stalled, or the session is parked for \
                 approval) — this is a host-side condition, not a client one"
            )
        })??;
    let welcome =
        Welcome::decode(&welcome_msg).map_err(|e| anyhow!("Welcome decode: {e:?}"))?;
    tracing::info!(
        mode = ?welcome.mode,
        frames = welcome.frames,
        codec = welcome.codec,
        bit_depth = welcome.bit_depth,
        encrypt = welcome.encrypt,
        udp_port = welcome.udp_port,
        "session offer"
    );

    // Admission, before a single byte of video is consumed.
    let mut machine = Machine::new();
    machine
        .connect(&remote.to_string())
        .map_err(|e| anyhow!(e))?;
    let admission = if args.pin.is_some() {
        Admission::Stream
    } else {
        // Manual `ip:port` entry has no advert to consult, so the user decides by confirming the
        // fingerprint the host just presented (the probe's TOFU behaviour).
        Admission::ConfirmTrust
    };
    let host_label = remote.to_string();
    match machine.admitted(admission) {
        connect::Transition::Stay(_, msg) => tracing::info!(%msg, "admission"),
        connect::Transition::DropAndIdle(msg) => {
            conn.close(0u32.into(), b"refused");
            return Err(anyhow!(msg));
        }
    }

    // Reserve our data-plane port, then let the host start.
    let probe = std::net::UdpSocket::bind("0.0.0.0:0")?;
    let udp_port = probe.local_addr()?.port();
    drop(probe);
    io::write_msg(&mut send, &Start { client_udp_port: udp_port }.encode()).await?;

    let host_udp = SocketAddr::new(remote.ip(), welcome.udp_port);
    let cfg = welcome.session_config(Role::Client);
    let expected = welcome.frames;
    let codec = welcome.codec;
    let mode = welcome.mode;
    let record = args.record.clone();
    let seconds = args.seconds;

    let started = Instant::now();
    let (frames, bytes, _aus, presented) =
        tokio::task::spawn_blocking(move || -> Result<(u64, u64, u64, u64)> {
        let transport = UdpTransport::connect(&format!("0.0.0.0:{udp_port}"), &host_udp.to_string())
            .context("bind data plane")?;
        let mut session =
            Session::new(cfg, Box::new(transport)).map_err(|e| anyhow!("client session: {e:?}"))?;
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
            let use_mf = std::env::var_os("SLIPSTREAM_WINDOWS_DECODER_MF").is_some();
            let dec: Box<dyn Decoder> = if use_mf {
                Box::new(mf::MfDecoder::new(mode.width, mode.height, codec)?)
            } else {
                Box::new(ff::FfmpegDecoder::new(mode.width, mode.height, codec)?)
            };
            let pres: Box<dyn Presenter> = match window::GdiPresenter::new(
                &format!("slipstream — {host_label}"),
                mode.width,
                mode.height,
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
        loop {
            if presenter.closed() {
                tracing::info!("window closed — ending the session");
                break;
            }
            if let Some(secs) = seconds {
                if started.elapsed() > Duration::from_secs(secs) {
                    break;
                }
            } else if last_rx.elapsed() > Duration::from_secs(45) {
                tracing::warn!("no frames for 45 s — ending the session");
                break;
            }
            if expected > 0 && frames >= expected as u64 {
                break;
            }
            match session.poll_frame() {
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
                    let decoded = decoder.decode(&frame.data, codec);
                    if decoded.is_some() {
                        stats.note_decoded();
                        presented += 1;
                        stats.note_presented();
                    }
                    presenter.present(decoded.as_ref());
                    let (qm, bm) = decoder.flow_stats();
                    stats.queue_max = qm;
                    stats.blocked_ms = bm;
                    stats.tick();
                }
                Err(SlipstreamError::NoFrame) => {
                    std::thread::sleep(Duration::from_micros(300));
                }
                Err(e) => return Err(anyhow!("poll_frame: {e:?}")),
            }
        }
        if let Some(mut s) = sink {
            s.flush().ok();
        }
        Ok((frames, bytes, 0, presented))
    })
    .await??;

    conn.close(0u32.into(), b"quit");
    println!(
        "session ended: received {frames} video AUs, presented {presented} frames, {bytes} bytes in {:.1}s ({:.1} fps presented)",
        started.elapsed().as_secs_f64(),
        presented as f64 / started.elapsed().as_secs_f64().max(0.001),
    );
    if frames == 0 {
        tracing::warn!(
            "zero frames: the host accepted the session but published none \
             (a wedged host capture thread does this — it is a host-side bug, not a client one)"
        );
        std::process::exit(3);
    }
    Ok(())
}
