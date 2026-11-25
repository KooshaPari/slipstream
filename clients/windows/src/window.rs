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
//!
//! With an [`InputSender`](crate::input::InputSender), the same messages become wire input events:
//! VK down/up, relative mouse deltas, five mouse buttons, and 120-scaled wheel deltas. Focus gates
//! the stream; losing focus forces key/button releases so the host never keeps a stuck modifier.

#![allow(unsafe_code)]

use crate::input::{key, mouse_button, mouse_move, wheel, InputSender};
use crate::present::{DecodedFrame, Presenter};
use anyhow::{Context, Result};
use slipstream_core::input::InputEvent;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    SetDIBitsToDevice, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC,
    HGDIOBJ, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect,
    PeekMessageW, RegisterClassW, ShowWindow, TranslateMessage, MSG, PM_REMOVE, SW_SHOW,
    WA_INACTIVE, WINDOW_EX_STYLE, WM_ACTIVATE, WM_CLOSE, WM_DESTROY, WM_KEYDOWN, WM_KEYUP,
    WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETFOCUS, WM_SYSKEYDOWN,
    WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    XBUTTON1, XBUTTON2,
};

/// Set by the window procedure when the user closes the window or presses Esc.
static CLOSED: AtomicBool = AtomicBool::new(false);

const VK_ESCAPE: usize = 0x1B;

/// Per-window input state, written only by the window procedure.
#[derive(Default)]
struct InputState {
    sender: Option<InputSender>,
    /// VK codes currently held (bits 0..255 across four u64s).
    held_keys: [u64; 4],
    /// Mouse buttons currently held (bits 1..5).
    held_buttons: u8,
    /// Last cursor point for delta calculation; `None` until the first mouse message.
    last_point: Option<(i32, i32)>,
}

/// One slot per live window, keyed by `HWND` value. The client has one presentation window per
/// session; using a table instead of a global keeps a stale window from receiving a later
/// session's input after reconnect.
static INPUT_STATE: Mutex<Vec<(usize, Arc<Mutex<InputState>>)>> = Mutex::new(Vec::new());

fn state_for(hwnd: HWND) -> Arc<Mutex<InputState>> {
    let key = hwnd.0 as usize;
    let mut table = INPUT_STATE.lock().unwrap();
    if let Some((_, state)) = table.iter().find(|(k, _)| *k == key) {
        return state.clone();
    }
    let state = Arc::new(Mutex::new(InputState::default()));
    table.push((key, state.clone()));
    state
}

fn remove_state(hwnd: HWND) {
    let key = hwnd.0 as usize;
    INPUT_STATE.lock().unwrap().retain(|(k, _)| *k != key);
}

fn submit(state: &Arc<Mutex<InputState>>, ev: Option<InputEvent>) {
    if let Some(ev) = ev {
        if let Some(sender) = state.lock().unwrap().sender.as_ref() {
            sender.submit(ev);
        }
    }
}

fn release_held_input(state: &Arc<Mutex<InputState>>) {
    let (sender, keys, buttons) = {
        let mut state = state.lock().unwrap();
        if let Some(sender) = state.sender.as_ref() {
            sender.set_focused(false);
        }
        let keys = std::mem::take(&mut state.held_keys);
        let buttons = std::mem::take(&mut state.held_buttons);
        state.last_point = None;
        (state.sender.clone(), keys, buttons)
    };
    let Some(sender) = sender else {
        return;
    };
    for (word, bits) in keys.into_iter().enumerate() {
        for bit in 0..64 {
            if bits & (1u64 << bit) != 0 {
                let vk = word * 64 + bit;
                if let Some(ev) = key(false, vk as u32) {
                    sender.submit_release(ev);
                }
            }
        }
    }
    for button in 1..=5u8 {
        if buttons & (1 << button) != 0 {
            if let Some(ev) = mouse_button(false, button as u32) {
                sender.submit_release(ev);
            }
        }
    }
}

fn lparam_x(lparam: LPARAM) -> i32 {
    (lparam.0 as i16) as i32
}

fn lparam_y(lparam: LPARAM) -> i32 {
    ((lparam.0 >> 16) as i16) as i32
}

fn wheel_delta(wparam: WPARAM) -> i32 {
    // The high word of WPARAM is already measured in WHEEL_DELTA (120) units for both
    // WM_MOUSEWHEEL and WM_MOUSEHWHEEL; the host injector uses the same scale.
    ((wparam.0 >> 16) as i16) as i32
}

fn xbutton(wparam: WPARAM) -> u32 {
    match ((wparam.0 >> 16) as u16) & 0xffff {
        XBUTTON1 => 4,
        XBUTTON2 => 5,
        _ => 0,
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let state = state_for(hwnd);
    match msg {
        WM_CLOSE => {
            CLOSED.store(true, Ordering::Relaxed);
            unsafe { windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd) }.ok();
            LRESULT(0)
        }
        WM_DESTROY => {
            CLOSED.store(true, Ordering::Relaxed);
            release_held_input(&state);
            remove_state(hwnd);
            unsafe { windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 == VK_ESCAPE => {
            unsafe {
                windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                    hwnd,
                    WM_CLOSE,
                    WPARAM(0),
                    LPARAM(0),
                )
            }
            .ok();
            LRESULT(0)
        }
        WM_ACTIVATE => {
            if (wparam.0 as u16) == WA_INACTIVE as u16 {
                release_held_input(&state);
            } else if let Some(sender) = state.lock().unwrap().sender.as_ref() {
                sender.set_focused(true);
            }
            LRESULT(0)
        }
        WM_SETFOCUS => {
            if let Some(sender) = state.lock().unwrap().sender.as_ref() {
                sender.set_focused(true);
            }
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            release_held_input(&state);
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = wparam.0 as usize;
            if vk < 256 {
                state.lock().unwrap().held_keys[vk / 64] |= 1u64 << (vk % 64);
            }
            submit(&state, key(true, wparam.0 as u32));
            LRESULT(0)
        }
        WM_KEYUP | WM_SYSKEYUP => {
            let vk = wparam.0 as usize;
            if vk < 256 {
                state.lock().unwrap().held_keys[vk / 64] &= !(1u64 << (vk % 64));
            }
            submit(&state, key(false, wparam.0 as u32));
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = (lparam_x(lparam), lparam_y(lparam));
            let mut state = state.lock().unwrap();
            if let Some((px, py)) = state.last_point {
                let ev = mouse_move(x - px, y - py);
                if let (Some(ev), Some(sender)) = (ev, state.sender.as_ref()) {
                    sender.submit(ev);
                }
            }
            state.last_point = Some((x, y));
            drop(state);
            LRESULT(0)
        }
        WM_LBUTTONDOWN | WM_MBUTTONDOWN | WM_RBUTTONDOWN | WM_XBUTTONDOWN => {
            let button = match msg {
                WM_LBUTTONDOWN => 1,
                WM_MBUTTONDOWN => 2,
                WM_RBUTTONDOWN => 3,
                _ => xbutton(wparam),
            };
            if button >= 1 && button <= 5 {
                state.lock().unwrap().held_buttons |= 1 << button;
            }
            submit(&state, mouse_button(true, button));
            LRESULT(0)
        }
        WM_LBUTTONUP | WM_MBUTTONUP | WM_RBUTTONUP | WM_XBUTTONUP => {
            let button = match msg {
                WM_LBUTTONUP => 1,
                WM_MBUTTONUP => 2,
                WM_RBUTTONUP => 3,
                _ => xbutton(wparam),
            };
            if button >= 1 && button <= 5 {
                state.lock().unwrap().held_buttons &= !(1 << button);
            }
            submit(&state, mouse_button(false, button));
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            submit(&state, wheel(wheel_delta(wparam), false));
            LRESULT(0)
        }
        WM_MOUSEHWHEEL => {
            submit(&state, wheel(wheel_delta(wparam), true));
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The DIB the memory DC has selected, and the one handle that is safe to restore on drop.
///
/// `SelectObject` returns the object it replaced. On the first selection that is the DC's stock
/// bitmap, which lives as long as the DC. On a resize it is the previous DIB, which the resize
/// then deletes, so storing that return value as the restore handle (the old `_old` field did)
/// leaves `Drop` re-selecting a deleted handle. `restore` therefore only ever holds the handle
/// captured at construction, and every handle a resize replaces is deleted, never restored.
#[derive(Debug)]
struct DibHandles {
    restore: HGDIOBJ,
    current: HGDIOBJ,
}

impl DibHandles {
    fn new(restore: HGDIOBJ, first: HGDIOBJ) -> Self {
        Self {
            restore,
            current: first,
        }
    }

    /// Swap in `dib` and return the handle it replaced: the caller must delete that handle.
    ///
    /// Deliberately does not touch `restore`; see the type comment for why the returned handle is
    /// not a valid restore target.
    fn swap(&mut self, dib: HGDIOBJ) -> HGDIOBJ {
        let replaced = self.current;
        self.current = dib;
        replaced
    }

    /// The handle `Drop` may select back into the memory DC (`None` when nothing was restored).
    fn restore_target(&self) -> Option<HGDIOBJ> {
        if self.restore.0.is_null() {
            None
        } else {
            Some(self.restore)
        }
    }
}

pub struct GdiPresenter {
    hwnd: HWND,
    hdc_mem: HDC,
    /// The DIB currently selected into `hdc_mem`, plus the handle to restore on drop.
    dibs: DibHandles,
    /// This window's input bookkeeping, so `Drop` can release whatever is still held down when the
    /// session ends (a host must not keep a key pressed after the client goes away).
    input_state: Arc<Mutex<InputState>>,
    bits: *mut core::ffi::c_void,
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
    /// Create the window and the DIB section. `title` goes in the caption. With `input`, captured
    /// Win32 messages are forwarded to the host through that gated route.
    pub fn new(title: &str, width: u32, height: u32, input: Option<InputSender>) -> Result<Self> {
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
        let mut outer = RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
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
        let input_state = state_for(hwnd);
        input_state.lock().unwrap().sender = input;
        let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };

        let hdc_mem = unsafe { CreateCompatibleDC(None) };
        if hdc_mem.is_invalid() {
            anyhow::bail!("CreateCompatibleDC failed");
        }
        let mut bits = std::ptr::null_mut();
        let bmi = Self::bitmap_info(width, height);
        let dib = unsafe { CreateDIBSection(hdc_mem, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) }
            .context("CreateDIBSection")?;
        // The first selection replaces the DC's stock bitmap: that handle, not the DIB, is what a
        // drop restores.
        let stock = unsafe { SelectObject(hdc_mem, HGDIOBJ(dib.0)) };

        tracing::info!(width, height, "window created");
        Ok(Self {
            hwnd,
            hdc_mem,
            dibs: DibHandles::new(stock, HGDIOBJ(dib.0)),
            input_state,
            bits,
            width,
            height,
            frames: 0,
            bytes: 0,
            convert_ns: 0,
            blit_ns: 0,
            started: Instant::now(),
            last_report: Instant::now(),
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

    /// Drain everything queued and report whether anything was.
    ///
    /// One drain helper for every path: the per-present pump and the idle pump while the session
    /// waits for a frame. Delegates to [`drain_window_messages`] so the idle path and the tests
    /// share the exact loop the presenter uses.
    fn drain_messages(&mut self) -> bool {
        drain_window_messages()
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

/// Drain every pending window and thread message, reporting whether any was seen.
///
/// A `WM_CLOSE` posted to the thread (`PostThreadMessageW`) never reaches [`wndproc`]: Windows
/// discards thread messages in `DispatchMessageW`. Observing it here is what lets a close posted
/// while no frame is arriving still end the session.
fn drain_window_messages() -> bool {
    let mut msg = MSG::default();
    let mut seen = false;
    while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
        seen = true;
        if msg.message == WM_CLOSE {
            CLOSED.store(true, Ordering::Relaxed);
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    seen
}

impl Presenter for GdiPresenter {
    /// Service messages while the stream has no frame for us, then yield briefly.
    ///
    /// The session loop's `NoFrame` branch calls this instead of sleeping blindly, so a stalled or
    /// ended stream cannot leave the window unresponsive: close and Esc are still handled.
    fn pump_idle(&mut self) {
        let _ = self.drain_messages();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    fn present(&mut self, frame: Option<&DecodedFrame>) {
        let _ = self.drain_messages();
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

    fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(2), height.max(2));
        if width == self.width && height == self.height {
            return;
        }
        // Reallocate the DIB at the delivered size and make the client area match it, so the
        // 1:1 blit path survives the resize instead of silently refusing every frame.
        let bmi = Self::bitmap_info(width, height);
        let mut bits = std::ptr::null_mut();
        let dib = match unsafe {
            CreateDIBSection(self.hdc_mem, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
        } {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "could not resize the DIB — keeping the old surface");
                return;
            }
        };
        // Selecting the new DIB returns the one it replaces. That handle is deleted here, so it
        // must not be kept as a restore target: `Drop` would re-select a dead DIB.
        let replaced = self.dibs.swap(HGDIOBJ(dib.0));
        unsafe {
            let _ = DeleteObject(replaced);
        }
        self.bits = bits;
        self.width = width;
        self.height = height;
        let style = WS_OVERLAPPEDWINDOW | WS_VISIBLE;
        let mut outer = RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        unsafe { AdjustWindowRect(&mut outer, style, false) }.ok();
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                outer.right - outer.left,
                outer.bottom - outer.top,
                windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                    | windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER,
            );
        }
        tracing::info!(width, height, "window resized to the delivered mode");
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
        // The session is over on every path that reaches here (clean end, host disconnect, error),
        // and the window may never have received the focus-loss message that normally releases
        // input. Do it here so the host is never left holding a key or button down.
        release_held_input(&self.input_state);
        remove_state(self.hwnd);
        unsafe {
            // Restore the handle captured at construction, never one a resize deleted, then
            // release the DIB that is actually selected.
            if let Some(restore) = self.dibs.restore_target() {
                let _ = SelectObject(self.hdc_mem, restore);
            }
            let _ = DeleteObject(self.dibs.current);
            let _ = DeleteDC(self.hdc_mem);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slipstream_core::input::InputKind;

    /// A handle value only ever compared, never passed to GDI.
    fn handle(n: usize) -> HGDIOBJ {
        HGDIOBJ(n as *mut core::ffi::c_void)
    }

    /// The bug this guards: a resize stored the handle `SelectObject` returned, which is the DIB
    /// the same resize then deletes, and `Drop` re-selected it. The restore target must stay the
    /// handle captured at construction.
    #[test]
    fn a_resize_never_turns_a_deleted_dib_into_the_restore_handle() {
        let stock = handle(1);
        let mut dibs = DibHandles::new(stock, handle(2));

        // First resize: `SelectObject` returns DIB 2, which the resize deletes.
        let replaced = dibs.swap(handle(3));
        assert_eq!(
            replaced.0 as usize, 2,
            "swap must report the DIB it replaced"
        );
        assert_eq!(
            dibs.restore_target().map(|h| h.0 as usize),
            Some(1),
            "drop must restore the construction handle, not a deleted DIB"
        );

        // Second resize: the handle it returns is again the one about to be deleted.
        let replaced = dibs.swap(handle(4));
        assert_eq!(replaced.0 as usize, 3);
        assert_ne!(
            dibs.restore_target().map(|h| h.0 as usize),
            Some(3),
            "a deleted DIB must never become the restore target"
        );
        assert_eq!(dibs.current.0 as usize, 4);
    }

    #[test]
    fn no_restore_handle_is_offered_when_the_first_select_failed() {
        let mut dibs = DibHandles::new(HGDIOBJ(std::ptr::null_mut()), handle(2));
        dibs.swap(handle(3));
        assert!(dibs.restore_target().is_none());
    }

    /// Session end releases what the window still holds down, including a key and buttons that
    /// never received their up message because the session simply ended.
    #[test]
    fn session_end_releases_every_held_key_and_button() {
        let (sender, mut rx) = InputSender::channel();
        sender.set_enabled(true);
        sender.set_focused(true);

        let mut held_keys = [0u64; 4];
        for vk in [0x41u32, 0xA0] {
            // 'A' and left shift
            held_keys[(vk / 64) as usize] |= 1u64 << (vk % 64);
        }
        let held_buttons = (1u8 << 1) | (1u8 << 3); // left + right
        let state = Arc::new(Mutex::new(InputState {
            sender: Some(sender),
            held_keys,
            held_buttons,
            last_point: Some((10, 10)),
        }));

        release_held_input(&state);

        let mut released = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            released.push((ev.kind, ev.code));
        }
        assert!(released.contains(&(InputKind::KeyUp, 0x41)));
        assert!(released.contains(&(InputKind::KeyUp, 0xA0)));
        assert!(released.contains(&(InputKind::MouseButtonUp, 1)));
        assert!(released.contains(&(InputKind::MouseButtonUp, 3)));
        assert_eq!(released.len(), 4, "only the held inputs are released");

        let state = state.lock().unwrap();
        assert_eq!(
            state.held_keys, [0; 4],
            "the release must clear the bookkeeping"
        );
        assert_eq!(state.held_buttons, 0);
        assert!(state.last_point.is_none());
    }

    /// Focus loss must release a held mouse button, not only held keys (5f81c2f5): a button pressed
    /// when the window goes inactive would otherwise stay down on the host.
    #[test]
    fn focus_loss_releases_held_mouse_buttons() {
        let (sender, mut rx) = InputSender::channel();
        sender.set_enabled(true);
        sender.set_focused(true);
        let state = Arc::new(Mutex::new(InputState {
            sender: Some(sender),
            held_keys: [0; 4],
            held_buttons: 1 << 1, // left only
            last_point: None,
        }));

        release_held_input(&state);

        let mut released = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            released.push((ev.kind, ev.code));
        }
        assert_eq!(
            released,
            vec![(InputKind::MouseButtonUp, 1)],
            "exactly the held button is released"
        );
        assert_eq!(state.lock().unwrap().held_buttons, 0);
    }

    /// Teardown runs on every exit path, and more than one of them can fire for the same session
    /// (the window procedure's WM_DESTROY and the presenter's drop). The second pass must emit
    /// nothing: a duplicate release would inject a spurious up event into a later session.
    #[test]
    fn releasing_held_input_twice_emits_nothing_the_second_time() {
        let (sender, mut rx) = InputSender::channel();
        sender.set_enabled(true);
        sender.set_focused(true);
        let state = Arc::new(Mutex::new(InputState {
            sender: Some(sender),
            held_keys: {
                let mut keys = [0u64; 4];
                keys[0x41 / 64] |= 1u64 << (0x41 % 64); // 'A'
                keys
            },
            held_buttons: 1 << 3, // right
            last_point: Some((4, 4)),
        }));

        release_held_input(&state);
        let first: Vec<(InputKind, u32)> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|ev| (ev.kind, ev.code))
            .collect();
        assert_eq!(
            first.len(),
            2,
            "one key-up and one button-up on the first pass, got {first:?}"
        );

        release_held_input(&state);
        assert!(
            matches!(rx.try_recv(), Err(_)),
            "a second teardown released something again"
        );
    }

    /// The bug this guards: the `NoFrame` branch slept bare, so a pending close sat unhandled for
    /// as long as the stream stalled. A presenter that pumps messages while idle must observe a
    /// posted `WM_CLOSE` within 100 ms. The close is posted to the thread (as `wndproc` does for
    /// Esc), which `DispatchMessageW` discards, so only the drain's own check can see it.
    #[test]
    fn no_frame_idle_pump_observes_close_within_100ms() {
        use std::time::Duration;

        struct CountingPresenter {
            polls: u32,
            closed: bool,
        }
        impl Presenter for CountingPresenter {
            fn present(&mut self, _: Option<&DecodedFrame>) {}
            fn status(&mut self, _: &str) {}
            fn pump_idle(&mut self) {
                self.polls += 1;
                let _ = drain_window_messages();
                std::thread::sleep(Duration::from_millis(1));
            }
            fn closed(&self) -> bool {
                self.closed
            }
        }

        CLOSED.store(false, Ordering::Relaxed);
        let mut p = CountingPresenter {
            polls: 0,
            closed: false,
        };
        let start = Instant::now();
        let mut posted = false;
        // A stream that returns NoFrame forever, with a close already pending.
        while !p.closed && start.elapsed() < Duration::from_millis(100) {
            if !posted {
                unsafe {
                    let tid = windows::Win32::System::Threading::GetCurrentThreadId();
                    let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                        tid,
                        WM_CLOSE,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
                posted = true;
            }
            p.pump_idle();
            if CLOSED.load(Ordering::Relaxed) {
                p.closed = true;
            }
        }
        let elapsed = start.elapsed();
        CLOSED.store(false, Ordering::Relaxed);

        assert!(posted, "the test never posted the close");
        assert!(
            p.closed,
            "posted WM_CLOSE not observed while NoFrame repeated"
        );
        assert!(
            elapsed <= Duration::from_millis(100),
            "the idle pump took {elapsed:?}"
        );
        assert!(p.polls >= 1, "the idle pump was never called");
    }
}
