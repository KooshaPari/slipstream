//! A real Win32 window that blits decoded frames with GDI.
//!
//! Deliberately the smallest thing that puts moving pixels on screen: one `WS_OVERLAPPEDWINDOW`,
//! one top-down 32-bpp DIB section, `StretchDIBits` per frame (aspect-preserving, so a 1080p stream
//! letterboxes instead of distorting). No D3D11, no swapchain, no GPU interop — a memcpy-class blit
//! at native size, which is what the "moving picture in a window" bar asks for.
//!
//! Everything runs on the session thread: the window is created there, messages are pumped with a
//! non-blocking `PeekMessageW` on every present, and closing the window (or pressing Esc) ends the
//! session instead of hanging it.
#![allow(unsafe_code)]

use crate::present::{DecodedFrame, Presenter};
use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    SetDIBitsToDevice, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
    HDC, HGDIOBJ, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, PeekMessageW,
    RegisterClassW, ShowWindow, TranslateMessage, MSG, PM_REMOVE, SW_SHOW, WINDOW_EX_STYLE,
    WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

/// Set by the window procedure when the user closes the window or presses Esc.
static CLOSED: AtomicBool = AtomicBool::new(false);

const WM_DESTROY: u32 = 0x0002;
const WM_CLOSE: u32 = 0x0010;
const WM_KEYDOWN: u32 = 0x0100;
const VK_ESCAPE: usize = 0x1B;

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CLOSE => {
            CLOSED.store(true, Ordering::Relaxed);
            unsafe { windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd) }.ok();
            LRESULT(0)
        }
        WM_DESTROY => {
            CLOSED.store(true, Ordering::Relaxed);
            unsafe { windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 == VK_ESCAPE => {
            unsafe { windows::Win32::UI::WindowsAndMessaging::PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0)) }
                .ok();
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub struct GdiPresenter {
    hwnd: HWND,
    hdc_mem: HDC,
    dib: HBITMAP,
    bits: *mut core::ffi::c_void,
    /// The DIB that was selected into the memory DC before ours (restored on drop).
    _old: HGDIOBJ,
    width: u32,
    height: u32,
    pub frames: u64,
    pub bytes: u64,
    /// Diagnostic: nanoseconds spent converting + copying into the DIB, and blitting it.
    pub convert_ns: u64,
    pub blit_ns: u64,
    started: Instant,
    last_report: Instant,
}

// The window handle + DIB are thread-affine in principle; the presenter is created and used only
// on the session thread, so sharing the type itself is never exercised.
unsafe impl Send for GdiPresenter {}

impl GdiPresenter {
    /// Create the window and the DIB section. `title` goes in the caption.
    pub fn new(title: &str, width: u32, height: u32) -> Result<Self> {
        let width = width.max(2);
        let height = height.max(2);
        let class_name = wide("slipstream-window");
        let instance = unsafe { GetModuleHandleW(None) }.context("GetModuleHandleW")?;
        let wc = WNDCLASSW {
            style: Default::default(),
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance.into(),
            hIcon: Default::default(),
            hCursor: Default::default(),
            hbrBackground: unsafe {
                windows::Win32::Graphics::Gdi::CreateSolidBrush(
                    windows::Win32::Foundation::COLORREF(0),
                )
            },
            lpszMenuName: PCWSTR::null(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
        };
        unsafe { RegisterClassW(&wc) };
        // Size the *client* area to the frame, not the outer window: a 1:1 client area is what
        // lets `blit` take the un-stretched `SetDIBitsToDevice` path instead of GDI's software
        // stretch. Stretching an 8 MB DIB by a fractional ratio cost 28-46 ms/frame here.
        let style = WS_OVERLAPPEDWINDOW | WS_VISIBLE;
        let mut outer = RECT { left: 0, top: 0, right: width as i32, bottom: height as i32 };
        unsafe { AdjustWindowRect(&mut outer, style, false) }.ok();
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class_name.as_ptr()),
                PCWSTR(wide(title).as_ptr()),
                style,
                64,
                64,
                outer.right - outer.left,
                outer.bottom - outer.top,
                None,
                None,
                instance,
                None,
            )
        }
        .context("CreateWindowExW")?;
        let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };

        let hdc_mem = unsafe { CreateCompatibleDC(None) };
        if hdc_mem.is_invalid() {
            anyhow::bail!("CreateCompatibleDC failed");
        }
        let mut bits = std::ptr::null_mut();
        let bmi = Self::bitmap_info(width, height);
        let dib = unsafe {
            CreateDIBSection(
                hdc_mem,
                &bmi,
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            )
        }
        .context("CreateDIBSection")?;
        let old = unsafe { SelectObject(hdc_mem, HGDIOBJ(dib.0)) };

        tracing::info!(width, height, "window created");
        Ok(Self {
            hwnd,
            hdc_mem,
            dib,
            bits,
            width,
            height,
            frames: 0,
            bytes: 0,
            convert_ns: 0,
            blit_ns: 0,
            started: Instant::now(),
            last_report: Instant::now(),
            _old: old,
        })
    }

    fn bitmap_info(width: u32, height: u32) -> BITMAPINFO {
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            // Negative height = top-down, matching every video frame's row order.
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        bmi
    }

    /// Copy the frame into the DIB. `DecodedFrame` is already in GDI's 32-bpp `BI_RGB` byte order
    /// (B, G, R, X), so this is a straight `memcpy` with no per-pixel work.
    fn copy_frame_to_dib(&mut self, f: &DecodedFrame) {
        let expected = self.width as usize * self.height as usize * 4;
        if f.width != self.width || f.height != self.height || f.pixels.len() != expected {
            return;
        }
        if !self.bits.is_null() {
            unsafe {
                std::ptr::copy_nonoverlapping(f.pixels.as_ptr(), self.bits as *mut u8, expected)
            };
        }
    }

    fn pump(&mut self, mut msg: MSG) {
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    fn blit(&mut self) {
        let mut rect = RECT::default();
        unsafe { GetClientRect(self.hwnd, &mut rect) }.ok();
        let cw = rect.right - rect.left;
        let ch = rect.bottom - rect.top;
        if cw <= 0 || ch <= 0 {
            return;
        }
        // Letterbox: fit the frame inside the client area keeping the aspect ratio.
        let (fw, fh) = (self.width as f64, self.height as f64);
        let scale = (cw as f64 / fw).min(ch as f64 / fh);
        let dw = (fw * scale) as i32;
        let dh = (fh * scale) as i32;
        let dx = (cw - dw) / 2;
        let dy = (ch - dh) / 2;
        let bmi = Self::bitmap_info(self.width, self.height);
        let hdc = unsafe { GetDC(self.hwnd) };
        let _ = self.hdc_mem;
        if dw == self.width as i32 && dh == self.height as i32 {
            // 1:1: a straight copy, the fast GDI path (no resampling).
            unsafe {
                SetDIBitsToDevice(
                    hdc,
                    dx,
                    dy,
                    self.width,
                    self.height,
                    0,
                    0,
                    0,
                    self.height,
                    self.bits,
                    &bmi,
                    DIB_RGB_COLORS,
                )
            };
        } else {
            unsafe {
                StretchDIBits(
                    hdc,
                    dx,
                    dy,
                    dw,
                    dh,
                    0,
                    0,
                    self.width as i32,
                    self.height as i32,
                    Some(self.bits),
                    &bmi,
                    DIB_RGB_COLORS,
                    SRCCOPY,
                )
            };
        }
        unsafe { ReleaseDC(self.hwnd, hdc) };
    }
}

impl Presenter for GdiPresenter {
    fn present(&mut self, frame: Option<&DecodedFrame>) {
        let mut msg = MSG::default();
        self.pump(msg);
        let _ = &mut msg;
        if let Some(f) = frame {
            self.frames += 1;
            self.bytes += f.pixels.len() as u64;
            let t0 = Instant::now();
            self.copy_frame_to_dib(f);
            self.convert_ns += t0.elapsed().as_nanos() as u64;
            let t1 = Instant::now();
            self.blit();
            self.blit_ns += t1.elapsed().as_nanos() as u64;
        }
        if self.last_report.elapsed() >= std::time::Duration::from_secs(2) {
            let secs = self.started.elapsed().as_secs_f64();
            tracing::info!(
                frames = self.frames,
                fps = format!("{:.1}", self.frames as f64 / secs.max(0.001)),
                mib = self.bytes / (1024 * 1024),
                "present"
            );
            self.last_report = Instant::now();
        }
    }

    fn status(&mut self, message: &str) {
        tracing::info!(%message, "status");
    }

    fn stage_ns(&self) -> (u64, u64) {
        (self.convert_ns, self.blit_ns)
    }

    fn closed(&self) -> bool {
        CLOSED.load(Ordering::Relaxed)
    }
}

impl Drop for GdiPresenter {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.hdc_mem, self._old);
            let _ = DeleteObject(HGDIOBJ(self.dib.0));
            let _ = DeleteDC(self.hdc_mem);
        }
    }
}
