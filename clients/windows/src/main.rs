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
#![forbid(unsafe_code)]

mod args;
mod connect;
mod discovery;
mod identity;
mod present;

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
        Some(Action::Connect) => stream(args),
    }
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
    let welcome =
        Welcome::decode(&recv.read_msg().await?).map_err(|e| anyhow!("Welcome decode: {e:?}"))?;
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
    let record = args.record.clone();
    let seconds = args.seconds;

    let started = Instant::now();
    let (frames, bytes, aus) = tokio::task::spawn_blocking(move || -> Result<(u64, u64, u64)> {
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
        let mut decoder = PassthroughDecoder::default();
        let mut presenter = ConsolePresenter::default();
        presenter.status(&format!("receiving from {host_label}"));
        let (mut frames, mut bytes) = (0u64, 0u64);
        let mut last_rx = Instant::now();
        loop {
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
                    bytes += frame.data.len() as u64;
                    if let Some(s) = sink.as_mut() {
                        s.write_all(&frame.data).context("write AU")?;
                    }
                    let decoded = decoder.decode(&frame.data, codec);
                    presenter.present(decoded.as_ref());
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
        Ok((frames, bytes, decoder.aus))
    })
    .await??;

    conn.close(0u32.into(), b"quit");
    println!(
        "session ended: received {frames} video AUs ({aus} through the decoder), {bytes} bytes in {:.1}s",
        started.elapsed().as_secs_f64()
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
