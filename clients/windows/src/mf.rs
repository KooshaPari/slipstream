//! HEVC / H.264 decode through the Windows Media Foundation decoder MFTs.
//!
//! This is the decode path this box actually has: `Microsoft.HEVCVideoExtension` is installed, so
//! `MFTEnumEx(MFT_CATEGORY_VIDEO_DECODER, MFVideoFormat_HEVC)` resolves a real HEVC decoder, and
//! H.264 resolves to the always-present `msmpeg2vdec`. No FFmpeg development SDK, no extra codec
//! crate — the same Windows codec stack that plays these files in the OS player.
//!
//! It is a *synchronous* MFT: one AU in, zero or one NV12 frame out, converted here to tight BGRA
//! (the [`DecodedFrame`] contract). Pixels on screen is the bar, not zero-copy.
#![allow(unsafe_code)]

use crate::present::{DecodedFrame, Decoder};
use anyhow::{anyhow, bail, Context, Result};
use slipstream_core::quic::{CODEC_AV1, CODEC_H264, CODEC_HEVC};
use windows::core::GUID;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;

/// The host's negotiated `CODEC_*` byte (from `slipstream_core::quic`), mapped to an MF subtype.
fn subtype_for(codec: u8) -> Result<GUID> {
    match codec {
        CODEC_HEVC => Ok(MFVideoFormat_HEVC),
        CODEC_H264 => Ok(MFVideoFormat_H264),
        CODEC_AV1 => Ok(MFVideoFormat_AV1),
        other => bail!("Media Foundation: unknown negotiated codec byte {other}"),
    }
}

fn pack_size(width: u32, height: u32) -> u64 {
    ((width as u64) << 32) | height as u64
}

/// Human name of a negotiated codec byte, for the messages an operator has to act on.
fn codec_name(codec: u8) -> &'static str {
    match codec {
        CODEC_HEVC => "HEVC",
        CODEC_AV1 => "AV1",
        _ => "H.264",
    }
}

/// Enumerate a synchronous NV12 decoder MFT for `sub` and hand back the first activation.
///
/// Shared by [`MfDecoder::new`] and [`supports_av1`] so the `Hello` advertisement and the decoder
/// that runs cannot disagree about what this install can decode. Every activation other than the
/// returned one is released.
fn find_decoder_activate(sub: GUID) -> Result<Option<IMFActivate>> {
    let input_info = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: sub,
    };
    let output_info = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_DECODER,
            MFT_ENUM_FLAG_SYNCMFT,
            Some(&input_info),
            Some(&output_info),
            &mut activates,
            &mut count,
        )
    }
    .context("MFTEnumEx(video decoder)")?;
    // `activates` is a CoTaskMem array of `count` IMFActivate pointers, not a single pointer.
    // Read it through a bounded slice so the count the API just reported is the only bound in
    // play, then release every activation we were handed: CoTaskMemFree frees the *array*, not
    // the COM objects it points at, so skipping any of them leaks a reference.
    let mut chosen: Option<IMFActivate> = None;
    if count > 0 && !activates.is_null() {
        // SAFETY: `MFTEnumEx` succeeded, so it allocated `count` consecutive
        // `IMFActivate*` slots at `activates`; `Option<IMFActivate>` is layout-compatible
        // with that nullable COM pointer.
        let slots: &[Option<IMFActivate>] =
            unsafe { std::slice::from_raw_parts(activates, count as usize) };
        chosen = slots.iter().find_map(|slot| slot.clone());
    }
    if count > 0 && !activates.is_null() {
        for i in 0..count as usize {
            // SAFETY: element `i` of the array described above; dropping it in place releases
            // the reference the API transferred to us without moving it out of the array.
            unsafe { std::ptr::drop_in_place(activates.add(i)) };
        }
    }
    // Hand the CoTaskMem array back before any early return.
    unsafe { CoTaskMemFree(Some(activates as *const _ as *const core::ffi::c_void)) };
    Ok(chosen)
}

/// Whether this Windows install has an AV1 decoder MFT.
///
/// AV1 is not part of the base OS: decode needs the AV1 Video Extension, which may be absent. The
/// `Hello` advertisement calls this so a machine without it never negotiates an AV1 stream. Every
/// failure reads as "no AV1": the advertisement must not promise a decode the session cannot do.
pub fn supports_av1() -> bool {
    if unsafe { MFStartup(MF_VERSION, 0) }.is_err() {
        return false;
    }
    matches!(find_decoder_activate(MFVideoFormat_AV1), Ok(Some(_)))
}

pub struct MfDecoder {
    transform: IMFTransform,
    width: u32,
    height: u32,
    /// The decoder's own output type dimensions (may be rounded up to a macroblock multiple).
    out_w: u32,
    out_h: u32,
    out_stride: i32,
    out_size: u32,
    frame_index: u32,
    /// Sample data format currently accepted by `ProcessInput`.
    input_fmt: InputFmt,
    /// Reused BGRA output buffer (one allocation for the session, not one per frame).
    scratch: Vec<u8>,
    pub decoded: u64,
    pub failed_aus: u64,
    pub stream_changes: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InputFmt {
    /// Annex B with 3/4-byte start codes (what the host emits, and what MF accepts for H.264).
    AnnexB,
    /// 4-byte big-endian length-prefixed NAL units.
    LengthPrefixed,
}

/// A locked Media Foundation buffer that unlocks itself on drop.
///
/// `convert` used to call `Lock2D`/`Lock` and never unlock: the early returns (a failed lock, a
/// null pointer) and the normal path all left the MFT's buffer locked for the life of the decoder.
/// A guard makes the release unconditional on every path — including `?`, `return None`, and a
/// panic — so it cannot be forgotten again.
struct LockedBuffer<'a> {
    buffer: &'a IMFMediaBuffer,
    /// Present when the buffer exposed `IMF2DBuffer`; then `Unlock2D` is the matching release.
    two_d: Option<IMF2DBuffer>,
    ptr: *mut u8,
    pitch: i32,
    /// Test-only: set by `Drop`. `MFCreateMemoryBuffer`'s own `Lock`/`Unlock` do not enforce
    /// exclusivity, so a test cannot observe the platform lock state — but it can observe that the
    /// release ran, which is the property the old code lacked.
    #[cfg(test)]
    released: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for LockedBuffer<'_> {
    fn drop(&mut self) {
        // SAFETY: both unlock calls pair with the lock taken in `lock_buffer`, on the same
        // interface pointer, and run exactly once.
        unsafe {
            match self.two_d.as_ref() {
                Some(b2) => {
                    let _ = b2.Unlock2D();
                }
                None => {
                    let _ = self.buffer.Unlock();
                }
            }
        }
        #[cfg(test)]
        self.released
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Lock `buffer` for reading, preferring `IMF2DBuffer` (which also reports the real stride).
///
/// `None` means the buffer could not be locked, its pointer was null, or a flat buffer reported
/// fewer than `need_bytes` bytes (a short buffer must not be read past its allocation). Every
/// failure path unlocks before returning, so callers cannot leak the lock.
fn lock_buffer<'a>(
    buffer: &'a IMFMediaBuffer,
    fallback_stride: i32,
    need_bytes: usize,
) -> Option<LockedBuffer<'a>> {
    use windows::core::Interface;
    // Prefer the 2D interface (it reports the real stride), but a plain memory buffer can expose
    // `IMF2DBuffer` and still refuse `Lock2D`; that is not a failure, it means the flat path below
    // is the right one. Giving up here was itself a way `convert` lost frames.
    if let Ok(b2) = buffer.cast::<IMF2DBuffer>() {
        let mut ptr = std::ptr::null_mut();
        let mut pitch = 0i32;
        // SAFETY: the interface came from this buffer; `Lock2D` fills both out-params.
        if unsafe { b2.Lock2D(&mut ptr, &mut pitch) }.is_ok() && !ptr.is_null() {
            return Some(LockedBuffer {
                buffer,
                two_d: Some(b2),
                ptr,
                pitch,
                #[cfg(test)]
                released: Default::default(),
            });
        }
        let _ = unsafe { b2.Unlock2D() };
    }
    // `GetMaxLength` is the allocation size. `Lock`'s current-length out-param is 0 until data is
    // written, so it cannot be used as the bound; checking before locking also means a refusal
    // never takes a lock that would need releasing.
    let max = unsafe { buffer.GetMaxLength() }.unwrap_or(0);
    if (max as usize) < need_bytes {
        return None;
    }
    let mut ptr = std::ptr::null_mut();
    // SAFETY: `Lock` pairs with the `Unlock` in `Drop`.
    if unsafe { buffer.Lock(&mut ptr, None, None) }.is_err() || ptr.is_null() {
        let _ = unsafe { buffer.Unlock() };
        return None;
    }
    Some(LockedBuffer {
        buffer,
        two_d: None,
        ptr,
        pitch: fallback_stride,
        #[cfg(test)]
        released: Default::default(),
    })
}

impl MfDecoder {
    /// Build a decoder for `codec` at the negotiated `width`x`height`. Fails (never silently) when
    /// this Windows install has no decoder MFT for the codec.
    pub fn new(width: u32, height: u32, codec: u8) -> Result<Self> {
        if width == 0 || height == 0 {
            bail!("Media Foundation: negotiated mode is {width}x{height}");
        }
        let sub = subtype_for(codec)?;
        unsafe { MFStartup(MF_VERSION, 0) }.context("MFStartup")?;

        // Ask for a *synchronous* decoder that outputs NV12 — the shape every MFT offers. The
        // enumerator is shared with `supports_av1`, which is what the `Hello` advertisement uses.
        let activate = find_decoder_activate(sub)?.ok_or_else(|| {
            anyhow!(
                "no Media Foundation {} decoder on this Windows install{}",
                codec_name(codec),
                if codec == CODEC_H264 {
                    ""
                } else {
                    " (is the matching Video Extension installed?)"
                }
            )
        })?;
        let transform: IMFTransform =
            unsafe { activate.ActivateObject() }.context("activate the decoder MFT")?;

        let itype = unsafe { MFCreateMediaType() }?;
        unsafe {
            itype.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            itype.SetGUID(&MF_MT_SUBTYPE, &sub)?;
            itype.SetUINT64(&MF_MT_FRAME_SIZE, pack_size(width, height))?;
            transform
                .SetInputType(0, &itype, 0)
                .context("decoder SetInputType")?;
        }

        let mut decoder = Self {
            transform,
            width,
            height,
            out_w: width,
            out_h: height,
            out_stride: width as i32,
            out_size: width * height * 3 / 2,
            frame_index: 0,
            input_fmt: InputFmt::AnnexB,
            scratch: Vec::new(),
            decoded: 0,
            failed_aus: 0,
            stream_changes: 0,
        };
        decoder.select_output_type()?;
        Ok(decoder)
    }

    /// Pick the NV12 output type and read back its real geometry + stride.
    fn select_output_type(&mut self) -> Result<()> {
        let mut chosen: Option<IMFMediaType> = None;
        let mut i = 0u32;
        while i < 16 {
            match unsafe { self.transform.GetOutputAvailableType(0, i) } {
                Ok(t) => {
                    let subtype = unsafe { t.GetGUID(&MF_MT_SUBTYPE) }.unwrap_or_default();
                    if subtype == MFVideoFormat_NV12 {
                        chosen = Some(t);
                        break;
                    }
                    i += 1;
                }
                Err(_) => break,
            }
        }
        let otype = chosen.ok_or_else(|| anyhow!("decoder offers no NV12 output type"))?;
        unsafe { otype.SetUINT64(&MF_MT_FRAME_SIZE, pack_size(self.width, self.height)) }
            .map_err(|e| anyhow!("output FRAME_SIZE: {e}"))?;
        unsafe { self.transform.SetOutputType(0, &otype, 0) }.context("decoder SetOutputType")?;

        if let Ok(sz) = unsafe { otype.GetUINT64(&MF_MT_FRAME_SIZE) } {
            self.out_w = (sz >> 32) as u32;
            self.out_h = (sz & 0xffff_ffff) as u32;
        }
        if let Ok(stride) = unsafe { otype.GetUINT32(&MF_MT_DEFAULT_STRIDE) } {
            self.out_stride = stride as i32;
        }
        // `out_size` is the buffer we hand the MFT. Ask it; fall back to the geometry above.
        if let Ok(info) = unsafe { self.transform.GetOutputStreamInfo(0) } {
            if info.cbSize > 0 {
                self.out_size = info.cbSize;
            }
        }
        if self.out_size == 0 {
            self.out_size = self.out_w * self.out_h * 3 / 2;
        }
        Ok(())
    }

    /// Annex B (`00 00 01` / `00 00 00 01` separated) → 4-byte big-endian lengths.
    fn to_length_prefixed(au: &[u8]) -> Vec<u8> {
        let mut starts = Vec::new();
        let mut i = 0usize;
        while i + 3 <= au.len() {
            if au[i] == 0 && au[i + 1] == 0 && au[i + 2] == 1 {
                starts.push(i + 3);
                i += 3;
            } else {
                i += 1;
            }
        }
        let mut out = Vec::with_capacity(au.len() + starts.len() * 4);
        for (k, &body) in starts.iter().enumerate() {
            // The next NAL's start code begins at `next - 3`; the trailing zero of a 4-byte
            // start code (if any) is not part of this NAL.
            let end = match starts.get(k + 1) {
                Some(&next) => {
                    let mut e = next - 3;
                    if e > body && au[e - 1] == 0 {
                        e -= 1;
                    }
                    e
                }
                None => au.len(),
            };
            let end = end.clamp(body, au.len());
            let nal = &au[body..end];
            out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            out.extend_from_slice(nal);
        }
        out
    }

    fn submit(&mut self, data: &[u8]) -> Result<()> {
        let sample = unsafe { MFCreateSample() }?;
        let buffer = unsafe { MFCreateMemoryBuffer(data.len() as u32) }?;
        unsafe {
            let mut ptr = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None)?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(data.len() as u32)?;
            sample.AddBuffer(&buffer)?;
            self.transform.ProcessInput(0, &sample, 0)?;
        }
        Ok(())
    }

    /// Drain every output the MFT has ready; return the newest frame (older ones are superseded).
    fn drain(&mut self) -> Option<DecodedFrame> {
        let mut newest = None;
        for _ in 0..4 {
            let osample = match unsafe { MFCreateSample() } {
                Ok(s) => s,
                Err(_) => break,
            };
            let obuf = match unsafe { MFCreateMemoryBuffer(self.out_size) } {
                Ok(b) => b,
                Err(_) => break,
            };
            if unsafe { osample.AddBuffer(&obuf) }.is_err() {
                break;
            }
            let mut data = MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: std::mem::ManuallyDrop::new(Some(osample.clone())),
                dwStatus: 0,
                pEvents: std::mem::ManuallyDrop::new(None),
            };
            let mut status = 0u32;
            match unsafe {
                self.transform
                    .ProcessOutput(0, std::slice::from_mut(&mut data), &mut status)
            } {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => break,
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    self.stream_changes += 1;
                    if self.select_output_type().is_err() {
                        break;
                    }
                    continue;
                }
                Err(e) => {
                    tracing::debug!(error = %e, "ProcessOutput");
                    break;
                }
            }
            if let Some(frame) = self.convert(&osample) {
                newest = Some(frame);
            }
        }
        newest
    }

    fn convert(&mut self, sample: &IMFSample) -> Option<DecodedFrame> {
        let buffer = unsafe { sample.GetBufferByIndex(0) }.ok()?;
        // NV12 is 1.5 bytes/pixel; a flat buffer shorter than that is refused by `lock_buffer`.
        let need = (self.out_w as usize)
            .saturating_mul(self.out_h as usize)
            .saturating_mul(3)
            / 2;
        // The guard unlocks on drop, so every return below (including `?`) releases the buffer.
        let locked = lock_buffer(&buffer, self.out_stride.max(self.width as i32), need)?;
        let n = (self.out_w as usize) * (self.out_h as usize) * 4;
        let mut pixels = std::mem::take(&mut self.scratch);
        pixels.resize(n, 0);
        unsafe {
            nv12_to_bgra(
                locked.ptr,
                locked.pitch,
                self.out_w as usize,
                self.out_h as usize,
                &mut pixels,
            );
        }
        drop(locked);
        self.frame_index = self.frame_index.wrapping_add(1);
        Some(DecodedFrame {
            width: self.out_w,
            height: self.out_h,
            pixels,
            frame_index: self.frame_index,
            pts_ns: 0,
        })
    }
}

/// NV12 (BT.709 limited range, what the host signals for SDR) → tightly packed 32-bit BGRA, the
/// DIB byte order the presenter expects.
///
/// `pitch` is the luma scanline stride in bytes; the interleaved UV plane sits `pitch * height`
/// bytes after the Y plane (same stride). Pure and unsafe-free at the edges: callers hand it a
/// locked Media Foundation pointer, everything below is bounds-checked slice arithmetic.
///
/// # Safety
/// `yuv` must point at `pitch * height * 3 / 2` readable bytes, and `out` must be
/// `width * height * 4` bytes.
unsafe fn nv12_to_bgra(yuv: *const u8, pitch: i32, width: usize, height: usize, out: &mut [u8]) {
    let stride = pitch.unsigned_abs() as usize;
    // A negative stride means the buffer is bottom-up: its first row is the image's last row.
    let base = if pitch < 0 {
        unsafe { yuv.offset((stride * (height - 1)) as isize) }
    } else {
        yuv
    };
    let nv12 = unsafe { std::slice::from_raw_parts(base, stride * height * 3 / 2) };
    let step: isize = if pitch < 0 {
        -(stride as isize)
    } else {
        stride as isize
    };
    for y in 0..height {
        let yrow = (y as isize * step) as usize;
        let uvrow = (stride * height) as isize + ((y / 2) as isize * step);
        let uvrow = uvrow as usize;
        for x in 0..width {
            let yy = nv12[yrow + x] as i32;
            let u = nv12[uvrow + (x / 2) * 2] as i32 - 128;
            let v = nv12[uvrow + (x / 2) * 2 + 1] as i32 - 128;
            let c = (yy - 16).max(0);
            let r = (298 * c + 409 * v + 128) >> 8;
            let g = (298 * c - 100 * u - 208 * v + 128) >> 8;
            let b = (298 * c + 516 * u + 128) >> 8;
            let o = (y * width + x) * 4;
            out[o] = b.clamp(0, 255) as u8;
            out[o + 1] = g.clamp(0, 255) as u8;
            out[o + 2] = r.clamp(0, 255) as u8;
            out[o + 3] = 255;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `MFStartup` is process-wide and reference-counted; start it once so the two lock tests do
    /// not race each other's shutdown while a buffer is still alive.
    fn mf_started() {
        use std::sync::Once;
        static START: Once = Once::new();
        START.call_once(|| {
            unsafe { MFStartup(MF_VERSION, 0) }.expect("MFStartup");
        });
    }

    /// The property the old `convert` violated: the guard releases the lock when it goes away.
    /// `MFCreateMemoryBuffer` does not enforce exclusivity on `Lock`/`Unlock`, so the platform lock
    /// state is not observable; the guard's `released` flag (set by `Drop`) is, and the buffer is
    /// lockable again afterwards.
    #[test]
    fn the_buffer_lock_is_released_when_the_guard_drops() {
        use std::sync::atomic::Ordering;
        mf_started();
        let buffer = unsafe { MFCreateMemoryBuffer(1024) }.expect("MFCreateMemoryBuffer");
        let locked = lock_buffer(&buffer, 4, 2, 16).expect("first lock");
        assert!(
            !locked.released.load(Ordering::SeqCst),
            "released while held"
        );
        let released = locked.released.clone();
        drop(locked);
        assert!(
            released.load(Ordering::SeqCst),
            "Drop did not release the lock"
        );
        assert!(
            lock_buffer(&buffer, 4, 2, 16).is_some(),
            "the buffer is unusable after the release"
        );
    }

    /// A flat buffer smaller than the NV12 image is refused *and* left unlocked, so the refusal
    /// path cannot leak the lock either.
    #[test]
    fn a_short_flat_buffer_is_refused_and_left_unlocked() {
        mf_started();
        let buffer = unsafe { MFCreateMemoryBuffer(1024) }.expect("MFCreateMemoryBuffer");
        assert!(
            lock_buffer(&buffer, 4, 2, 4096).is_none(),
            "a 1024-byte buffer must not satisfy a 4096-byte read"
        );
        assert!(
            lock_buffer(&buffer, 4, 2, 16).is_some(),
            "the refusal path leaked the lock"
        );
    }

    #[test]
    fn annexb_converts_to_length_prefixed() {
        let au = [0, 0, 0, 1, 0x40, 0x01, 0, 0, 1, 0x42, 0x02, 0x03];
        let lp = MfDecoder::to_length_prefixed(&au);
        assert_eq!(&lp[..4], &2u32.to_be_bytes());
        assert_eq!(&lp[4..6], &[0x40, 0x01]);
        assert_eq!(&lp[6..10], &3u32.to_be_bytes());
        assert_eq!(&lp[10..13], &[0x42, 0x02, 0x03]);
    }

    #[test]
    fn nv12_converts_black_and_white() {
        // 2x2 black (Y=16, UV=128) then 2x2 white (Y=235).
        let mut nv12 = vec![16u8; 4];
        nv12.extend_from_slice(&[128, 128]);
        let mut pixels = vec![0u8; 2 * 2 * 4];
        unsafe { nv12_to_bgra(nv12.as_ptr(), 2, 2, 2, &mut pixels) };
        assert_eq!(&pixels[..4], &[0, 0, 0, 255]);

        let mut nv12 = vec![235u8; 4];
        nv12.extend_from_slice(&[128, 128]);
        unsafe { nv12_to_bgra(nv12.as_ptr(), 2, 2, 2, &mut pixels) };
        assert_eq!(&pixels[..4], &[255, 255, 255, 255]);
    }

    #[test]
    fn nv12_respects_the_pitch() {
        // 2x2 image in a 4-byte-pitch buffer: padding must not bleed into the pixels.
        let mut buf = vec![16u8; 8];
        for row in 0..2 {
            for col in 0..2 {
                buf[row * 4 + col] = 235;
            }
        }
        buf.extend_from_slice(&[128, 128, 128, 128]);
        let mut pixels = vec![0u8; 2 * 2 * 4];
        unsafe { nv12_to_bgra(buf.as_ptr(), 4, 2, 2, &mut pixels) };
        assert_eq!(&pixels[..4], &[255, 255, 255, 255]);
        assert_eq!(&pixels[4..8], &[255, 255, 255, 255]);
    }
}

impl Decoder for MfDecoder {
    fn decode(&mut self, au: &[u8], _codec: u8) -> Option<DecodedFrame> {
        if au.is_empty() {
            return None;
        }
        if let Err(e) = self.submit(au) {
            // Annex B is what the host emits and what the H.264 MFT wants; some HEVC builds want
            // length-prefixed NALs. Switch format once, then retry the same AU.
            if self.input_fmt == InputFmt::AnnexB {
                let lp = Self::to_length_prefixed(au);
                if let Ok(()) = self.submit(&lp) {
                    self.input_fmt = InputFmt::LengthPrefixed;
                    self.failed_aus += 1;
                    return self.drain();
                }
            }
            if self.failed_aus == 0 {
                tracing::warn!(error = %e, "first AU rejected by the Media Foundation decoder");
            }
            self.failed_aus += 1;
            return None;
        }
        let frame = self.drain();
        if frame.is_some() {
            self.decoded += 1;
        }
        frame
    }
}
