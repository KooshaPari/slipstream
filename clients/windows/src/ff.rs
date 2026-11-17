//! Decode through `ffmpeg.exe` as a child process: Annex-B access units in on stdin, raw 32-bit
//! BGRA frames out on stdout (GDI's DIB byte order, so the presenter can copy straight to screen).
//!
//! This exists because the two "native" options on this box do not close the loop:
//!   * the Media Foundation HEVC MFT (see [`crate::mf`]) accepts every AU but produces no output —
//!     the Microsoft HEVC decoder is an asynchronous DXVA MFT and needs a D3D11 device manager
//!     before it decodes anything, which is more machinery than "pixels in a window" needs;
//!   * `ffmpeg-next`/`ffmpeg-sys` needs the FFmpeg **development** SDK, which this box does not
//!     have (only `ffmpeg.exe`, shipped with ImageMagick).
//!
//! So the child process is the thin, verifiable path: the exact same bitstream the session already
//! receives, decoded by the ffmpeg build that is already installed, delivered as tight BGRA (DIB order). The
//! pipe is a real dependency and it is named in the PR: resolution order is `SLIPSTREAM_FFMPEG`,
//! then `ffmpeg` on `PATH`, then the ImageMagick location.
#![allow(dead_code)]

use crate::present::{DecodedFrame, Decoder};
use anyhow::{anyhow, bail, Context, Result};
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::atomic::{AtomicUsize, Ordering};

const IMAGEMAGICK_FFMPEG: &str =
    r"C:\Program Files\ImageMagick-7.1.0-Q16-HDRI\ffmpeg.exe";

fn find_ffmpeg() -> Result<String> {
    if let Some(p) = std::env::var_os("SLIPSTREAM_FFMPEG") {
        return Ok(p.to_string_lossy().into_owned());
    }
    if Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return Ok("ffmpeg".to_string());
    }
    if std::path::Path::new(IMAGEMAGICK_FFMPEG).exists() {
        return Ok(IMAGEMAGICK_FFMPEG.to_string());
    }
    bail!("no ffmpeg found: set SLIPSTREAM_FFMPEG to ffmpeg.exe")
}

pub struct FfmpegDecoder {
    child: Child,
    stdin: Option<ChildStdin>,
    /// Frames produced by the reader thread. ffmpeg blocks writing a raw frame (~8 MB at 1080p)
    /// when nobody drains stdout, and we block writing AUs when ffmpeg stops reading stdin — so a
    /// dedicated reader thread is what keeps the pipe from deadlocking in both directions.
    frames: Receiver<Vec<u8>>,
    /// Live frames waiting to be presented, and the high-water mark since the session started.
    /// Bounded by [`QUEUE_BOUND`]: the reader thread blocks on `send` when the queue is full, which
    /// is the backpressure that stops ffmpeg from decoding further ahead.
    queued: std::sync::Arc<AtomicUsize>,
    pub max_queued: usize,
    /// Nanoseconds spent waiting for the queue to drain before an AU could be written.
    pub blocked_ns: u64,
    width: u32,
    height: u32,
    frame_index: u32,
    frame_bytes: usize,
    codec: u8,
    /// The hwaccel actually in use, `None` for software.
    hwaccel: Option<&'static str>,
    /// AUs fed since (re)spawn; used to detect a hwaccel that produces no frames.
    fed: u32,
    /// Set once we have fallen back to software, so we never retry hwaccel in a loop.
    fell_back: bool,
    /// AUs fed since the last (re)spawn, kept so a fallback can replay them into the new child —
    /// without the opening keyframe's parameter sets the replacement decodes nothing.
    replay: Vec<Vec<u8>>,
    pub decoded: u64,
    pub eof: bool,
}

/// Windows GPU decode candidates, cheapest first. Both decode on the GPU and still hand us a
/// CPU-mappable BGRA frame on stdout, which is what our blit needs.
const HWACCELS: [&str; 2] = ["d3d11va", "dxva2"];

/// How many AUs a hwaccel gets before we conclude it is producing nothing. The GPU path buffers
/// more than software (a d3d11va pipe run decodes fine, it just starts later), so this is generous:
/// ~6 s at the host's ~95 AUs/s.
const HWACCEL_GRACE_AUS: u32 = 600;

/// Frames of decode-ahead allowed. A small bound is the whole point: at ~11 ms/frame, 3 frames is
/// ~33 ms of standing latency instead of the ~0.8 s a 75-frame backlog implied.
const QUEUE_BOUND: usize = 3;

impl FfmpegDecoder {
    pub fn new(width: u32, height: u32, codec_u8: u8) -> Result<Self> {
        let requested = std::env::var("SLIPSTREAM_FFMPEG_HWACCEL").ok();
        let hw = match requested.as_deref() {
            None | Some("none") | Some("software") => None,
            Some(other) => HWACCELS.iter().copied().find(|h| *h == other),
        };
        Self::spawn(width, height, codec_u8, hw)
    }

    fn spawn(
        width: u32,
        height: u32,
        codec_u8: u8,
        hwaccel: Option<&'static str>,
    ) -> Result<Self> {
        use slipstream_core::quic::{CODEC_H264, CODEC_HEVC};
        let input_fmt = match codec_u8 {
            CODEC_HEVC => "hevc",
            CODEC_H264 => "h264",
            other => bail!("ffmpeg decoder: unsupported negotiated codec byte {other}"),
        };
        let exe = find_ffmpeg()?;
        let mut child = Command::new(&exe)
            .args({
                let mut a: Vec<String> = Vec::new();
                a.extend(["-hide_banner".into(), "-loglevel".into(), "error".into()]);
                if let Some(hw) = hwaccel {
                    a.extend(["-hwaccel".into(), hw.into()]);
                }
                // No `-fflags nobuffer` / `-flags low_delay` here on purpose: with them the raw
                // HEVC demuxer reads nothing from a pipe and exits with "Output file is empty".
                // Verified by hand on this box; the added latency is ffmpeg's normal probe buffer.
                //
                // `-probesize`/`-analyzeduration` are cut because the default probe buffer is 5 MB:
                // on a live stream at ~1.2 KB/AU that is ~4000 AUs (~40 s) before ffmpeg emits
                // anything, which looked exactly like "hwaccel produces no frames". We already know
                // the codec and the exact geometry, so there is nothing to probe for.
                a.extend([
                    "-probesize".into(),
                    "32k".into(),
                    "-analyzeduration".into(),
                    "0".into(),
                    "-f".into(),
                    input_fmt.into(),
                    "-i".into(),
                    "pipe:0".into(),
                    "-an".into(),
                    "-f".into(),
                    "rawvideo".into(),
                    // BGRA, not RGBA: it is what the GDI DIB wants, so nothing has to swizzle
                    // 2 M pixels per frame on the client thread (measured ~10-70 ms/frame here).
                    "-pix_fmt".into(),
                    "bgra".into(),
                    "pipe:1".into(),
                ]);
                a
            })
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawn {exe}"))?;
        let stdin = child.stdin.take();
        let mut stdout: ChildStdout = child.stdout.take().ok_or_else(|| anyhow!("ffmpeg stdout"))?;
        // Reader thread: one raw BGRA frame per message until ffmpeg's stdout closes.
        let (tx, frames) = mpsc::sync_channel::<Vec<u8>>(QUEUE_BOUND);
        let queued = std::sync::Arc::new(AtomicUsize::new(0));
        let queued_reader = queued.clone();
        let frame_bytes = width as usize * height as usize * 4;
        std::thread::Builder::new()
            .name("ffmpeg-frames".into())
            .spawn(move || loop {
                let mut frame = vec![0u8; frame_bytes];
                match read_exact_or_eof(&mut stdout, &mut frame) {
                    Ok(true) => {
                        // Blocks here when the queue is full — that stop is the flow control.
                        if tx.send(frame).is_err() {
                            break; // the client went away
                        }
                        let d = queued_reader.fetch_add(1, Ordering::Relaxed) + 1;
                        if d > QUEUE_BOUND + 1 {
                            // Only reachable if a sender raced ahead; keep the high-water honest.
                            tracing::debug!(depth = d, "frame queue over bound");
                        }
                    }
                    _ => break, // EOF or a broken pipe: ffmpeg is done
                }
            })
            .context("spawn ffmpeg reader thread")?;
        tracing::info!(
            exe = %exe,
            codec = input_fmt,
            width,
            height,
            hwaccel = hwaccel.unwrap_or("none (software)"),
            "ffmpeg decoder started"
        );
        Ok(Self {
            child,
            stdin,
            frames,
            queued,
            max_queued: 0,
            blocked_ns: 0,
            width,
            height,
            frame_index: 0,
            frame_bytes,
            codec: codec_u8,
            hwaccel,
            fed: 0,
            fell_back: false,
            replay: Vec::new(),
            decoded: 0,
            eof: false,
        })
    }

    /// Feed one access unit and take the newest frame ffmpeg has produced so far.
    ///
    /// Never blocks: ffmpeg buffers a few frames before its first output (HEVC reordering), so a
    /// blocking "one AU in, one frame out" loop would deadlock on that delay. Feeding continues and
    /// frames are collected as they appear; only the newest is presented, so a decoder slower than
    /// the stream cannot grow an unbounded queue of 8 MB frames.
    pub fn decode_one(&mut self, au: &[u8]) -> Result<Option<DecodedFrame>> {
        // Pop before writing: with a bounded channel this is what frees the reader thread so
        // ffmpeg can advance. Writing first would block us on a full stdin pipe while the frame we
        // need to pop sits in the queue — the classic two-thread pipe deadlock.
        let mut frame = self.try_next();
        if let Some(si) = self.stdin.as_mut() {
            // A broken pipe means ffmpeg exited (bad input); surface it instead of looping.
            let t0 = std::time::Instant::now();
            let res = si.write_all(au).context("write AU to ffmpeg");
            self.blocked_ns += t0.elapsed().as_nanos() as u64;
            res?;
            si.flush().ok();
        }
        self.fed += 1;
        if self.replay.len() < HWACCEL_GRACE_AUS as usize * 2 {
            self.replay.push(au.to_vec());
        }
        // A hwaccel that never produces a frame (unsupported profile, no free GPU session) must not
        // silently look like a dead stream: fall back to software once and keep going.
        if self.decoded == 0 && self.fed > HWACCEL_GRACE_AUS && self.hwaccel.is_some() && !self.fell_back
        {
            let hw = self.hwaccel;
            self.fell_back = true;
            let next = HWACCELS
                .iter()
                .copied()
                .skip_while(|h| Some(*h) != hw)
                .nth(1);
            tracing::warn!(
                from = hw.unwrap_or("none"),
                to = next.unwrap_or("software"),
                aus = self.fed,
                "hwaccel produced no frames — switching and replaying"
            );
            if let Ok(mut d) = Self::spawn(self.width, self.height, self.codec, next) {
                d.fell_back = next.is_none();
                d.hwaccel = next;
                // Replay the buffered AUs so the replacement sees the opening parameter sets.
                let buffered = std::mem::take(&mut self.replay);
                for au in &buffered {
                    if let Some(si) = d.stdin.as_mut() {
                        if si.write_all(au).is_err() {
                            break;
                        }
                    }
                }
                d.fed = buffered.len() as u32;
                d.replay = buffered;
                *self = d;
            }
            return Ok(None);
        }
        // One frame per AU, never newest-wins: newest-wins silently discards frames ffmpeg already
        // decoded, which reads as "the decoder cannot keep up" in the counters. If more than one
        // frame is ready here it stays queued (bounded by QUEUE_BOUND) and is taken next call.
        if frame.is_none() {
            frame = self.try_next();
        }
        Ok(frame)
    }

    /// Close the input pipe and drain the frames ffmpeg still holds (end of a `--play` file).
    pub fn finish(&mut self) -> Vec<DecodedFrame> {
        self.stdin = None;
        let mut out = Vec::new();
        while let Ok(frame) = self.frames.recv() {
            self.frame_index = self.frame_index.wrapping_add(1);
            self.decoded += 1;
            out.push(DecodedFrame {
                width: self.width,
                height: self.height,
                pixels: frame,
                frame_index: self.frame_index,
                pts_ns: 0,
            });
        }
        out
    }

    /// Non-blocking check for a decoded frame (used when the caller drives its own pacing).
    pub fn try_next(&mut self) -> Option<DecodedFrame> {
        match self.frames.try_recv() {
            Ok(frame) => {
                let d = self.queued.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
                self.max_queued = self.max_queued.max(d + 1);
                self.frame_index = self.frame_index.wrapping_add(1);
                self.decoded += 1;
                Some(DecodedFrame {
                    width: self.width,
                    height: self.height,
                    pixels: frame,
                    frame_index: self.frame_index,
                    pts_ns: 0,
                })
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.eof = true;
                None
            }
        }
    }

    pub fn frame_bytes(&self) -> usize {
        self.frame_bytes
    }

    /// Frames decoded but not yet presented (the bounded decode-ahead queue).
    pub fn backlog(&self) -> usize {
        self.queued.load(Ordering::Relaxed)
    }
}

fn read_exact_or_eof<R: Read>(r: &mut R, buf: &mut [u8]) -> std::io::Result<bool> {
    let mut filled = 0usize;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => return Ok(false),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

impl Drop for FfmpegDecoder {
    fn drop(&mut self) {
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Decoder for FfmpegDecoder {
    fn decode(&mut self, au: &[u8], _codec: u8) -> Option<DecodedFrame> {
        match self.decode_one(au) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(error = %e, "ffmpeg decode");
                None
            }
        }
    }

    fn flow_stats(&self) -> (usize, u64) {
        (self.max_queued, self.blocked_ns / 1_000_000)
    }
}
