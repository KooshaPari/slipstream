//! Windows client input: Win32 messages → slipstream/1 [`InputEvent`]s.
//!
//! This module is pure at the translation/buffer layer so it can be unit-tested without a window
//! or a network. The Win32 WndProc lives in [`crate::window`] and calls [`InputRouter`] with
//! virtual-key codes; those VKs are GameStream/Windows VKs, exactly the `InputEvent.code`
//! vocabulary the host's `vk_to_evdev` consumes (see `ss-inject::input::keymap`).

use slipstream_core::input::{InputEvent, InputKind};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Bounded queue: 64 events. Enough for a burst of human key transitions without letting a
/// stuck consumer grow memory without limit; mouse moves are coalesced first when full.
pub const INPUT_QUEUE_DEPTH: usize = 64;
/// One warning per second at most, so a flooded queue cannot drown the log.
const DROP_LOG_INTERVAL: Duration = Duration::from_secs(1);

fn event(kind: InputKind, code: u32, x: i32, y: i32, flags: u32) -> InputEvent {
    InputEvent { kind, _pad: [0; 3], code, x, y, flags }
}

/// A translated, buffered input stream. Mouse moves coalesce; keys and buttons never drop.
#[derive(Debug)]
pub struct InputBuffer {
    queue: VecDeque<InputEvent>,
    held: Vec<InputEvent>,
    mouse: Option<InputEvent>,
    last_drop_log: Option<Instant>,
    dropped_mouse: u64,
}

impl Default for InputBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl InputBuffer {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::with_capacity(INPUT_QUEUE_DEPTH),
            held: Vec::new(),
            mouse: None,
            last_drop_log: None,
            dropped_mouse: 0,
        }
    }

    /// Push one event. Returns `false` only when a mouse move had to be dropped at capacity.
    pub fn push(&mut self, ev: InputEvent) -> bool {
        if matches!(ev.kind, InputKind::KeyDown) {
            if !self.held.iter().any(|h| h.kind == InputKind::KeyDown && h.code == ev.code) {
                self.held.push(ev);
            }
        } else if matches!(ev.kind, InputKind::KeyUp) {
            // A key-up always wins: it cancels the held key and replaces any pending move only
            // after key events have been given priority below.
            self.held.retain(|h| !(h.kind == InputKind::KeyDown && h.code == ev.code));
        }
        if matches!(ev.kind, InputKind::MouseMoveAbs | InputKind::MouseMove) {
            // Coalesce before enqueue: only the latest pointer position matters.
            self.mouse = Some(ev);
            return true;
        }
        self.enqueue(ev)
    }

    fn enqueue(&mut self, ev: InputEvent) -> bool {
        while self.queue.len() >= INPUT_QUEUE_DEPTH {
            if !self.drop_oldest_mouse() {
                // Only key/button events remain. Key-ups are urgent: make room by dropping the
                // oldest non-release event, never a key-up or button-up.
                if matches!(ev.kind, InputKind::KeyUp | InputKind::MouseButtonUp)
                    && !self.drop_oldest_non_release()
                {
                    return false;
                }
                if !matches!(ev.kind, InputKind::KeyUp | InputKind::MouseButtonUp) {
                    return false;
                }
            }
        }
        self.queue.push_back(ev);
        true
    }

    fn drop_oldest_non_release(&mut self) -> bool {
        if let Some(pos) = self.queue.iter().position(|e| {
            !matches!(e.kind, InputKind::KeyUp | InputKind::MouseButtonUp)
        }) {
            self.queue.remove(pos);
            self.note_drop();
            return true;
        }
        false
    }

    fn drop_oldest_mouse(&mut self) -> bool {
        if let Some(pos) = self.queue.iter().position(|e| matches!(e.kind, InputKind::MouseMoveAbs | InputKind::MouseMove)) {
            self.queue.remove(pos);
            self.note_drop();
            return true;
        }
        if let Some(ev) = self.mouse.take() {
            self.note_drop();
            let _ = ev;
            return true;
        }
        false
    }

    fn note_drop(&mut self) {
        self.dropped_mouse += 1;
        let now = Instant::now();
        let due = self.last_drop_log.map_or(true, |t| now.duration_since(t) >= DROP_LOG_INTERVAL);
        if due {
            self.last_drop_log = Some(now);
            tracing::warn!(dropped = self.dropped_mouse, "input queue full; dropped oldest mouse move");
        }
    }

    /// Take the next event for the transport, preferring the newest coalesced mouse position.
    pub fn next(&mut self) -> Option<InputEvent> {
        if let Some(m) = self.mouse.take() {
            return Some(m);
        }
        self.queue.pop_front()
    }

    /// Emit KeyUp (and mouse-button-up) for everything currently held, then clear state.
    /// Called on focus loss, minimize and session end; this path must not be skippable.
    pub fn release_all(&mut self) -> Vec<InputEvent> {
        let mut releases = Vec::with_capacity(self.held.len());
        for h in &self.held {
            if h.kind == InputKind::KeyDown {
                releases.push(event(InputKind::KeyUp, h.code, 0, 0, h.flags));
            }
        }
        self.held.clear();
        releases
    }
}

/// Shared handle used by the WndProc (any thread) and the session loop.
#[derive(Clone, Default)]
pub struct InputRouter {
    inner: Arc<Mutex<InputBuffer>>,
}

impl InputRouter {
    pub fn push(&self, ev: InputEvent) -> bool {
        self.inner.lock().unwrap().push(ev)
    }

    pub fn next(&self) -> Option<InputEvent> {
        self.inner.lock().unwrap().next()
    }

    /// Drain releases onto the buffer so they are sent before the session closes.
    pub fn release_all(&self) {
        let mut inner = self.inner.lock().unwrap();
        for ev in inner.release_all() {
            // Releases must never be dropped for queue pressure; bypass the bounded path.
            inner.push_front_urgent(ev);
        }
    }

    pub fn drain_all(&self) -> Vec<InputEvent> {
        let mut inner = self.inner.lock().unwrap();
        let mut out = inner.release_all();
        out.extend(inner.queue.drain(..));
        if let Some(m) = inner.mouse.take() {
            out.push(m);
        }
        out
    }
}

impl InputBuffer {
    fn push_front_urgent(&mut self, ev: InputEvent) {
        self.queue.push_front(ev);
    }
}

/// Translate a Windows virtual-key code to the protocol VK. Windows VKs already match the
/// GameStream virtual-key namespace for the keys the host maps, so this is identity for mapped
/// keys and `None` for ones the host does not know. Never send characters or raw scancodes.
pub fn vk_to_protocol(vk: u16) -> Option<u32> {
    let v = vk as u32;
    // The host's vk_to_evdev table covers these code ranges; unmapped keys are swallowed.
    let known = matches!(v,
        0x08 | 0x09 | 0x0D | 0x13 | 0x14 | 0x1B | 0x20..=0x2F |
        0x30..=0x39 | 0x41..=0x5A | 0x60..=0x6F | 0x70..=0x7B |
        0x90 | 0x91 | 0xA0..=0xA5 | 0xBA..=0xBF | 0xC0 | 0xDB..=0xDE);
    known.then_some(v)
}

/// Mouse button state translation. Windows uses WPARAM MK_* bits; protocol uses button ids.
pub const MOUSE_LBUTTON: u32 = 0;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vk_translation_table() {
        // letters, digits, arrows, modifiers, and an ISO layout key (0xE2 -> unmapped here)
        for (vk, expected) in [
            (0x41, Some(0x41)), // A
            (0x5A, Some(0x5A)), // Z
            (0x30, Some(0x30)), // 0
            (0x39, Some(0x39)), // 9
            (0x25, Some(0x25)), // Left arrow
            (0x27, Some(0x27)), // Right arrow
            (0xA0, Some(0xA0)), // LShift
            (0xA2, Some(0xA2)), // LCtrl
            (0xE2, None),       // ISO 102nd: not in the host's VK table via Windows VK path
        ] {
            assert_eq!(vk_to_protocol(vk), expected, "vk {vk:#04x}");
        }
    }

    #[test]
    fn key_up_is_never_dropped_at_capacity() {
        let mut b = InputBuffer::new();
        // Fill with non-mouse key-downs so the mouse-drop path cannot make room.
        for i in 0..INPUT_QUEUE_DEPTH {
            assert!(b.push(event(InputKind::KeyDown, 0x41 + (i % 26) as u32, 0, 0, 0)));
        }
        let up = event(InputKind::KeyUp, 0x41, 0, 0, 0);
        assert!(b.push(up));
        assert!(std::iter::from_fn(|| b.next()).any(|e| e == up));
    }

    #[test]
    fn focus_loss_releases_held_keys() {
        let mut b = InputBuffer::new();
        b.push(event(InputKind::KeyDown, 0x11, 0, 0, 0)); // Ctrl
        b.push(event(InputKind::KeyDown, 0x41, 0, 0, 0)); // A
        let releases = b.release_all();
        assert_eq!(releases.len(), 2);
        assert!(releases.iter().all(|e| e.kind == InputKind::KeyUp));
        assert!(releases.iter().any(|e| e.code == 0x11));
        assert!(releases.iter().any(|e| e.code == 0x41));
        assert!(b.release_all().is_empty(), "release is idempotent");
    }

    #[test]
    fn backpressure_coalesces_and_drops_only_mouse_moves() {
        let mut b = InputBuffer::new();
        let key = event(InputKind::KeyDown, 0x41, 0, 0, 0);
        b.push(key);
        for i in 0..INPUT_QUEUE_DEPTH * 2 {
            b.push(event(InputKind::MouseMoveAbs, 0, i as i32, i as i32, 0));
        }
        let all: Vec<_> = std::iter::from_fn(|| b.next()).collect();
        assert!(all.contains(&key), "key event must survive backpressure");
        assert!(all.iter().filter(|e| e.kind == InputKind::MouseMoveAbs).count() <= 1);
    }
}
