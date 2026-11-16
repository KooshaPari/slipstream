//! Decode through `ffmpeg.exe` as a child process: Annex-B access units in on stdin, raw RGBA
//! frames out on stdout.
//!
//! This exists because the two "native" options on this box do not close the loop:
//!   * the Media Foundation HEVC MFT (see [`crate::mf`]) accepts every AU but produces no output —
//!     the Microsoft HEVC decoder is an asynchronous DXVA MFT and needs a D3D11 device manager
//!     before it decodes anything, which is more machinery than "pixels in a window" needs;
//!   * `ffmpeg-next`/`ffmpeg-sys` needs the FFmpeg **development** SDK, which this box does not
//!     have (only `ffmpeg.exe`, shipped with ImageMagick).
//!
//! So the child process is the thin, verifiable path: the exact same bitstream the session already
//! receives, decoded by the ffmpeg build that is already installed, delivered as tight RGBA. The
//! pipe is a real dependency and it is named in the PR: resolution order is `SLIPSTREAM_FFMPEG`,
//! then `ffmpeg` on `PATH`, then the ImageMagick location.
#![allow(dead_code)]

use crate::present::{DecodedFrame, Decoder};
use anyhow::{anyhow, bail, Context, Result};
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};

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
    width: u32,
    height: u32,
    frame_index: u32,
    frame_bytes: usize,
    pub decoded: u64,
    pub eof: bool,
}

impl FfmpegDecoder {
    pub fn new(width: u32, height: u32, codec_u8: u8) -> Result<Self> {
        use slipstream_core::quic::{CODEC_H264, CODEC_HEVC};
        let input_fmt = match codec_u8 {
            CODEC_HEVC => "hevc",
            CODEC_H264 => "h264",
            other => bail!("ffmpeg decoder: unsupported negotiated codec byte {other}"),
        };
        let exe = find_ffmpeg()?;
        let mut child = Command::new(&exe)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                // No `-fflags nobuffer` / `-flags low_delay` here on purpose: with them the raw
                // HEVC demuxer reads nothing from a pipe and exits with "Output file is empty".
                // Verified by hand on this box; the added latency is ffmpeg's normal probe buffer.
                "-f",
                input_fmt,
                "-i",
                "pipe:0",
                "-an",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "pipe:1",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawn {exe}"))?;
        let stdin = child.stdin.take();
        let mut stdout: ChildStdout = child.stdout.take().ok_or_else(|| anyhow!("ffmpeg stdout"))?;
        // Reader thread: one raw RGBA frame per message until ffmpeg's stdout closes.
        let (tx, frames) = mpsc::channel::<Vec<u8>>();
        let frame_bytes = width as usize * height as usize * 4;
        std::thread::Builder::new()
            .name("ffmpeg-frames".into())
            .spawn(move || loop {
                let mut frame = vec![0u8; frame_bytes];
                match read_exact_or_eof(&mut stdout, &mut frame) {
                    Ok(true) => {
                        if tx.send(frame).is_err() {
                            break; // the client went away
                        }
                    }
                    _ => break, // EOF or a broken pipe: ffmpeg is done
                }
            })
            .context("spawn ffmpeg reader thread")?;
        tracing::info!(exe = %exe, codec = input_fmt, width, height, "ffmpeg decoder started");
        Ok(Self {
            child,
            stdin,
            frames,
            width,
            height,
            frame_index: 0,
            frame_bytes,
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
        if let Some(si) = self.stdin.as_mut() {
            // A broken pipe means ffmpeg exited (bad input); surface it instead of looping.
            si.write_all(au).context("write AU to ffmpeg")?;
            si.flush().ok();
        }
        let mut newest = None;
        while let Some(f) = self.try_next() {
            newest = Some(f);
        }
        Ok(newest)
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
                rgba: frame,
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
                self.frame_index = self.frame_index.wrapping_add(1);
                self.decoded += 1;
                Some(DecodedFrame {
                    width: self.width,
                    height: self.height,
                    rgba: frame,
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
}
