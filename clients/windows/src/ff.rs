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
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

const IMAGEMAGICK_FFMPEG: &str = r"C:\Program Files\ImageMagick-7.1.0-Q16-HDRI\ffmpeg.exe";

/// Codec bytes this backend can decode.
///
/// AV1 is deliberately absent: [`FfmpegDecoder::spawn`] maps only HEVC and H.264 to demuxers and
/// bails on anything else, so advertising AV1 would let a host negotiate a stream this process
/// then refuses at decoder creation. Keep this in step with the `match` in `spawn`.
pub fn supported_codecs() -> u8 {
    use slipstream_core::quic::{CODEC_H264, CODEC_HEVC};
    CODEC_H264 | CODEC_HEVC
}

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
    /// AUs go to a dedicated writer thread instead of being written from the session thread.
    /// Writing inline deadlocks: the session thread blocks in `write_all` when ffmpeg stops reading
    /// stdin (it is blocked writing an output frame whose consumer is the session thread, which is
    /// blocked), and nothing can break the cycle. Measured twice on this box — `--play` stalled at
    /// 41 AUs / 3 frames, and a live 2560x1440 session stalled at 0 frames for 107 s after a mode
    /// correction respawned the decoder. The channel is unbounded so the session thread never
    /// blocks; [`AU_BACKLOG_MAX`] bounds memory instead.
    au_tx: Option<std::sync::mpsc::Sender<Vec<u8>>>,
    writer: Option<std::thread::JoinHandle<()>>,
    /// Bytes handed to the writer and not yet written to ffmpeg's stdin.
    outstanding: std::sync::Arc<AtomicUsize>,
    /// AUs discarded because ffmpeg stopped draining (a wedged decoder must not grow memory).
    pub dropped_aus: u64,
    /// Set by the writer thread when the pipe broke (ffmpeg exited on bad input).
    pipe_broken: std::sync::Arc<std::sync::atomic::AtomicBool>,
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
    /// Bytes currently held in `replay`, kept alongside it so the cap is O(1) per AU.
    replay_bytes: usize,
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

/// AUs outstanding toward ffmpeg before new ones are dropped. At 1440p a live AU is ~14 KB, so this
/// is ~2 s of stream: far beyond any healthy backlog, and small enough that a wedged decoder cannot
/// eat memory. Dropping is the correct failure mode — ffmpeg needs a keyframe to resync, and the
/// host's recovery path supplies one.
const AU_BACKLOG_MAX: usize = 32 << 20;

/// The protocol's largest legal mode is 8192 px per axis (`ss-encode`'s `Codec::max_dimension` for
/// HEVC/AV1; 4096 for H.264), so the largest BGRA frame a legal host can send is 8192² × 4 bytes.
/// A geometry above this is corrupt or hostile: `width * height * 4` is peer-influenced and must
/// not be used as an allocation size without checking it.
const MAX_PROTOCOL_DIMENSION: u32 = 8192;
const MAX_FRAME_BYTES: usize =
    MAX_PROTOCOL_DIMENSION as usize * MAX_PROTOCOL_DIMENSION as usize * 4;

/// One access unit above this is not video. A live 1440p AU is ~14 KB (see [`AU_BACKLOG_MAX`]) and
/// a 4K IDR is a few hundred KB; 16 MiB is ~1000× a typical AU, far above any legitimate frame, and
/// small enough that a hostile length cannot make the writer channel buffer forever.
const MAX_AU_BYTES: usize = 16 << 20;

/// Bytes retained for the hwaccel-fallback replay. Same budget as the input backlog, so the
/// decoder's total buffered input — outstanding plus replay — stays bounded at 64 MiB.
const MAX_REPLAY_BYTES: usize = AU_BACKLOG_MAX;

/// Ceiling on the frames one end-of-input [`FfmpegDecoder::finish`] flush may accumulate. The flush
/// carries only what ffmpeg already decoded at EOF (its reorder tail, well under a second of video),
/// and a local `--play` file is drained frame by frame beforehand, so reaching this means a wedged
/// or hostile decoder. 256 MiB is ~32 1080p BGRA frames.
const MAX_FINISH_BYTES: usize = 256 << 20;

/// BGRA bytes for `width`x`height`, rejecting a geometry the protocol cannot produce. The products
/// are checked, so a wrapped pair of `u32`s is an error rather than an under-sized allocation.
fn checked_frame_bytes(width: u32, height: u32) -> Result<usize> {
    let bytes = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
        .with_context(|| format!("ffmpeg decoder: {width}x{height} BGRA size overflows usize"))?;
    if bytes == 0 {
        bail!("ffmpeg decoder: {width}x{height} has a zero dimension");
    }
    if bytes > MAX_FRAME_BYTES {
        bail!(
            "ffmpeg decoder: {width}x{height} BGRA is {bytes} bytes, above the {MAX_FRAME_BYTES} \
             byte frame cap (largest legal mode is {MAX_PROTOCOL_DIMENSION}px per axis)"
        );
    }
    Ok(bytes)
}

/// Refuse an access unit larger than [`MAX_AU_BYTES`] before it is cloned into the writer channel.
fn check_au_size(len: usize) -> Result<()> {
    if len > MAX_AU_BYTES {
        bail!("ffmpeg decoder: access unit of {len} bytes exceeds the {MAX_AU_BYTES} byte AU cap");
    }
    Ok(())
}

/// Whether an AU of `len` bytes still fits the replay budget with `have` bytes already buffered.
fn replay_fits(have: usize, len: usize) -> bool {
    have.saturating_add(len) <= MAX_REPLAY_BYTES
}

impl FfmpegDecoder {
    pub fn new(width: u32, height: u32, codec_u8: u8) -> Result<Self> {
        let requested = std::env::var("SLIPSTREAM_FFMPEG_HWACCEL").ok();
        let hw = match requested.as_deref() {
            None | Some("none") | Some("software") => None,
            Some(other) => HWACCELS.iter().copied().find(|h| *h == other),
        };
        Self::spawn(width, height, codec_u8, hw)
    }

    fn spawn(width: u32, height: u32, codec_u8: u8, hwaccel: Option<&'static str>) -> Result<Self> {
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
        let mut stdout: ChildStdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("ffmpeg stdout"))?;
        // Reader thread: one raw BGRA frame per message until ffmpeg's stdout closes.
        let (tx, frames) = mpsc::sync_channel::<Vec<u8>>(QUEUE_BOUND);
        let queued = std::sync::Arc::new(AtomicUsize::new(0));
        let queued_reader = queued.clone();
        let frame_bytes = checked_frame_bytes(width, height)?;
        // Writer thread: owns stdin, drains AUs at whatever rate ffmpeg accepts them, and accounts
        // for the bytes still outstanding so the session thread can stay non-blocking.
        let (au_tx, au_rx) = mpsc::channel::<Vec<u8>>();
        let outstanding = std::sync::Arc::new(AtomicUsize::new(0));
        let outstanding_writer = outstanding.clone();
        let pipe_broken = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pipe_broken_writer = pipe_broken.clone();
        let mut stdin_w = stdin;
        let writer = std::thread::Builder::new()
            .name("ffmpeg-aus".into())
            .spawn(move || {
                while let Ok(au) = au_rx.recv() {
                    let n = au.len();
                    let Some(si) = stdin_w.as_mut() else {
                        outstanding_writer.fetch_sub(n, Ordering::Relaxed);
                        break;
                    };
                    if si.write_all(&au).is_err() {
                        outstanding_writer.fetch_sub(n, Ordering::Relaxed);
                        pipe_broken_writer.store(true, Ordering::Relaxed);
                        break;
                    }
                    si.flush().ok();
                    outstanding_writer.fetch_sub(n, Ordering::Relaxed);
                }
                // Dropping stdin closes the pipe, which is what makes ffmpeg flush and exit.
                drop(stdin_w.take());
            })
            .context("spawn ffmpeg writer thread")?;
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
            au_tx: Some(au_tx),
            writer: Some(writer),
            outstanding,
            dropped_aus: 0,
            pipe_broken,
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
            replay_bytes: 0,
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
        // Reject an oversized AU before it is cloned into the writer channel or the replay buffer.
        check_au_size(au.len())?;
        let mut frame = self.try_next();
        if let Some(tx) = self.au_tx.as_ref() {
            // Never blocks on the pipe: the bound below is ffmpeg's own input backlog.
            let n = au.len();
            if self.outstanding.load(Ordering::Relaxed) + n > AU_BACKLOG_MAX {
                self.dropped_aus += 1;
                if self.dropped_aus == 1 || self.dropped_aus % 300 == 0 {
                    tracing::warn!(
                        dropped = self.dropped_aus,
                        outstanding_mib = self.outstanding.load(Ordering::Relaxed) >> 20,
                        "ffmpeg stopped draining input — dropping AUs until it catches up"
                    );
                }
            } else {
                self.outstanding.fetch_add(n, Ordering::Relaxed);
                if tx.send(au.to_vec()).is_err() {
                    self.outstanding.fetch_sub(n, Ordering::Relaxed);
                    self.pipe_broken.store(true, Ordering::Relaxed);
                }
            }
        }
        if self.pipe_broken.load(Ordering::Relaxed) {
            // ffmpeg exited (bad input); surface it instead of looping on a dead pipe.
            bail!("write AU to ffmpeg: pipe closed");
        }
        self.fed += 1;
        if self.replay.len() < HWACCEL_GRACE_AUS as usize * 2
            && replay_fits(self.replay_bytes, au.len())
        {
            self.replay_bytes += au.len();
            self.replay.push(au.to_vec());
        }
        // A hwaccel that never produces a frame (unsupported profile, no free GPU session) must not
        // silently look like a dead stream: fall back to software once and keep going.
        if self.decoded == 0
            && self.fed > HWACCEL_GRACE_AUS
            && self.hwaccel.is_some()
            && !self.fell_back
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
                    if let Some(tx) = d.au_tx.as_ref() {
                        d.outstanding.fetch_add(au.len(), Ordering::Relaxed);
                        if tx.send(au.clone()).is_err() {
                            d.pipe_broken.store(true, Ordering::Relaxed);
                            break;
                        }
                    }
                }
                d.fed = buffered.len() as u32;
                d.replay_bytes = buffered.iter().map(Vec::len).sum();
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
        // Closing the sender ends the writer thread once it has written everything queued; joining
        // it guarantees ffmpeg saw every AU before stdin closes.
        drop(self.au_tx.take());
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
        let mut out = Vec::new();
        let mut out_bytes = 0usize;
        while let Ok(frame) = self.frames.recv() {
            if out_bytes.saturating_add(frame.len()) > MAX_FINISH_BYTES {
                // Bound what we hold at EOF too. Nothing legitimate approaches the cap (ffmpeg's
                // reorder tail is a few frames), so stop rather than grow without bound on a
                // wedged decoder.
                tracing::warn!(
                    bytes = out_bytes,
                    cap = MAX_FINISH_BYTES,
                    "ffmpeg flush exceeded the byte cap — dropping the remaining tail"
                );
                break;
            }
            out_bytes += frame.len();
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
                let d = self
                    .queued
                    .fetch_sub(1, Ordering::Relaxed)
                    .saturating_sub(1);
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

    /// Rebuild the child at a new frame size. The reader thread sizes every `read` from the frame
    /// geometry, so a session whose delivered mode differs from the requested one desynchronises
    /// the pipe until the decoder is respawned at the truth. The buffered AUs are replayed into
    /// the replacement so it starts from the opening parameter sets, exactly as the hwaccel
    /// fallback does.
    pub fn reinit(&mut self, width: u32, height: u32) {
        if width == self.width && height == self.height {
            return;
        }
        tracing::info!(
            from_w = self.width,
            from_h = self.height,
            to_w = width,
            to_h = height,
            "ffmpeg decoder resizing to the delivered mode"
        );
        let Ok(mut d) = Self::spawn(width, height, self.codec, self.hwaccel) else {
            return;
        };
        let buffered = std::mem::take(&mut self.replay);
        for au in &buffered {
            if let Some(tx) = d.au_tx.as_ref() {
                d.outstanding.fetch_add(au.len(), Ordering::Relaxed);
                if tx.send(au.clone()).is_err() {
                    d.pipe_broken.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
        d.fed = buffered.len() as u32;
        d.replay_bytes = buffered.iter().map(Vec::len).sum();
        d.replay = buffered;
        d.fell_back = self.fell_back;
        *self = d;
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
        // Kill first: if ffmpeg is wedged with a full stdin pipe, the writer thread is blocked in
        // `write_all` and joining it would hang teardown forever (observed: a client that never
        // exited at its `--seconds` deadline). Killing closes the pipe, the write fails, and the
        // thread returns.
        let _ = self.child.kill();
        let _ = self.child.wait();
        drop(self.au_tx.take());
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
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

    fn resize(&mut self, width: u32, height: u32) {
        self.reinit(width, height);
    }

    fn flow_stats(&self) -> (usize, u64) {
        (self.max_queued, self.blocked_ns / 1_000_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_bytes_are_capped_at_the_protocol_maximum() {
        assert_eq!(checked_frame_bytes(1920, 1080).unwrap(), 1920 * 1080 * 4);
        // The largest legal mode is exactly at the cap; one pixel wider is over it.
        assert!(
            checked_frame_bytes(MAX_PROTOCOL_DIMENSION, MAX_PROTOCOL_DIMENSION).is_ok(),
            "the protocol's largest mode must still decode"
        );
        let err =
            checked_frame_bytes(MAX_PROTOCOL_DIMENSION + 1, MAX_PROTOCOL_DIMENSION).unwrap_err();
        assert!(err.to_string().contains("frame cap"), "{err}");
    }

    #[test]
    fn zero_and_overflowing_geometry_are_rejected() {
        assert!(checked_frame_bytes(0, 1080).is_err());
        // 2^32-1 squared overflows usize on 64-bit; it must be an error, not a wrapped allocation.
        assert!(checked_frame_bytes(u32::MAX, u32::MAX).is_err());
    }

    #[test]
    fn au_size_cap_rejects_only_oversized_units() {
        assert!(check_au_size(0).is_ok());
        assert!(check_au_size(MAX_AU_BYTES).is_ok(), "the cap is inclusive");
        let err = check_au_size(MAX_AU_BYTES + 1).unwrap_err();
        assert!(err.to_string().contains("AU cap"), "{err}");
    }

    #[test]
    fn replay_budget_is_enforced() {
        assert!(replay_fits(0, MAX_REPLAY_BYTES));
        assert!(!replay_fits(1, MAX_REPLAY_BYTES));
        assert!(!replay_fits(usize::MAX, 1), "saturating add must not wrap");
    }
}
