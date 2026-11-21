//! Rate-limited keyframe recovery policy for the Windows client.
//!
//! The host encodes an infinite GOP: after the opening IDR, every picture references an earlier
//! one. If that IDR is lost (or a decoder is restarted and replays only deltas), the decoder emits
//! nothing and the session would stay black forever. `RequestKeyframe` is the protocol's explicit
//! recovery message; this module decides *when* to send it and throttles the asks so a host that
//! cannot honor them is never flooded.

use std::future::Future;
use std::time::{Duration, Instant};

/// Wait this long from session start before concluding the opening IDR never landed.
///
/// A software ffmpeg pipe legitimately takes a fraction of a second to probe and prime its codec;
/// two seconds is several times that while still short enough that the user does not stare at a
/// black window. Matches the patience used by the Android client's recovery loops.
const NO_FIRST_FRAME_PATIENCE: Duration = Duration::from_secs(2);

/// While the decoder has produced no picture, an AU that yields no output is a decode error / stale
/// reference. Ask again after this streak rather than on every AU.
const DECODE_ERROR_PATIENCE: Duration = Duration::from_millis(500);

/// Minimum gap between requests. A broken host that never emits an IDR must not receive a request
/// per frame (~95/s); this bounds it to one every three seconds.
const REQUEST_COOLDOWN: Duration = Duration::from_secs(3);

/// Absolute cap. After this many asks the host is clearly unable to re-anchor; further requests are
/// noise, and the existing 45 s no-video watchdog ends the session.
const MAX_REQUESTS: u8 = 8;

/// Why we asked the host for a fresh IDR. Logged with every request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyframeReason {
    /// No decodable frame arrived within the startup window.
    NoFirstFrame,
    /// The decoder rejected or swallowed an access unit.
    DecodeError,
}

impl KeyframeReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoFirstFrame => "no-first-frame",
            Self::DecodeError => "decode-error",
        }
    }
}

/// notifications from the decode loop into the recovery state machine.
#[derive(Debug, Clone, Copy)]
pub enum RecoveryEvent {
    /// A decoded frame appeared; the stream is anchored, stop asking.
    Output,
    /// A received AU produced no picture while the decoder is still unanchored.
    DecodeError,
    /// The decoder was respawned (mode correction or hardware fallback).
    DecoderRestart,
}

/// A due request produced by [`RecoveryState::advance`]. The caller must report whether the write
/// succeeded via [`RecoveryState::mark_sent`]; attempts and successes are tracked separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyframeRequest {
    pub reason: KeyframeReason,
    pub attempted: u8,
}

/// State owned by the session loop; only the loop mutates it (no lock needed).
#[derive(Debug)]
pub struct RecoveryState {
    started: Instant,
    last_request: Option<Instant>,
    first_output: bool,
    error_since: Option<Instant>,
    attempted: u8,
    sent: u8,
}

impl Default for RecoveryState {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            last_request: None,
            first_output: false,
            error_since: None,
            attempted: 0,
            sent: 0,
        }
    }
}

impl RecoveryState {
    /// Decide whether a request is due, updating throttle state atomically. The returned token
    /// records an *attempt*, not a successful send; call [`Self::mark_sent`] only after the write
    /// succeeds. `now` is the loop's monotonic clock reading.
    /// `now` is the loop's monotonic clock reading.
    pub fn advance(&mut self, event: Option<RecoveryEvent>, now: Instant) -> Option<KeyframeRequest> {
        if let Some(event) = event {
            match event {
                RecoveryEvent::Output => {
                    self.first_output = true;
                    self.error_since = None;
                }
                RecoveryEvent::DecodeError => {
                    if !self.first_output {
                        self.error_since.get_or_insert(now);
                    }
                }
                RecoveryEvent::DecoderRestart => {
                    // A replacement decoder has no reference picture of its own. Even if the old
                    // one had produced a frame, the new one needs a fresh IDR to produce anything.
                    self.first_output = false;
                    self.started = now;
                    self.error_since = None;
                }
            }
        }
        if self.attempted >= MAX_REQUESTS
            || self
                .last_request
                .is_some_and(|at| now.duration_since(at) < REQUEST_COOLDOWN)
        {
            return None;
        }
        let reason = if !self.first_output && now.duration_since(self.started) >= NO_FIRST_FRAME_PATIENCE
        {
            KeyframeReason::NoFirstFrame
        } else if !self.first_output
            && self
                .error_since
                .is_some_and(|at| now.duration_since(at) >= DECODE_ERROR_PATIENCE)
        {
            KeyframeReason::DecodeError
        } else {
            return None;
        };
        self.last_request = Some(now);
        self.attempted += 1;
        self.error_since = None;
        Some(KeyframeRequest {
            reason,
            attempted: self.attempted,
        })
    }

    /// Record that a due request was actually written to the control stream.
    pub fn mark_sent(&mut self) {
        if self.sent < self.attempted {
            self.sent += 1;
        }
    }

    /// Number of due requests so far, including ones that could not be written.
    pub fn attempted(&self) -> u8 {
        self.attempted
    }

    /// Number of requests successfully written to the control stream, for logs and tests.
    pub fn sent(&self) -> u8 {
        self.sent
    }
}

/// Run a control-stream write on the current Tokio runtime without panicking when none exists.
///
/// Returns `true` only when the request was written. A missing runtime is logged at warn and treated
/// as unsent, so callers do not call [`RecoveryState::mark_sent`] and logs do not claim a recovery
/// that never reached the host.
pub fn try_block_on_write<F>(
    future: F,
    reason: KeyframeReason,
    attempted: u8,
    sent: u8,
    elapsed_ms: u128,
) -> bool
where
    F: Future<Output = std::io::Result<()>>,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => match handle.block_on(future) {
            Ok(()) => {
                tracing::warn!(
                    reason = reason.as_str(),
                    attempted,
                    sent,
                    elapsed_ms,
                    "requested a keyframe — no decodable picture"
                );
                true
            }
            Err(e) => {
                tracing::warn!(
                    reason = reason.as_str(),
                    attempted,
                    sent,
                    elapsed_ms,
                    error = %e,
                    "keyframe request failed"
                );
                false
            }
        },
        Err(e) => {
            tracing::warn!(
                reason = reason.as_str(),
                attempted,
                sent,
                elapsed_ms,
                error = %e,
                "keyframe request not sent: no Tokio runtime"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_request_before_startup_patience() {
        let mut state = RecoveryState::default();
        let started = state.started;
        assert_eq!(state.advance(None, started), None);
        assert_eq!(
            state.advance(None, started + NO_FIRST_FRAME_PATIENCE - Duration::from_millis(1)),
            None
        );
    }

    #[test]
    fn no_runtime_does_not_panic_and_is_not_counted_as_sent() {
        let mut state = RecoveryState::default();
        let request = state
            .advance(None, state.started + NO_FIRST_FRAME_PATIENCE)
            .expect("request is due");
        let write = async { Ok(()) };
        assert!(!try_block_on_write(
            write,
            request.reason,
            request.attempted,
            state.sent(),
            0
        ));
        assert_eq!(state.attempted(), 1);
        assert_eq!(state.sent(), 0);
    }

    #[test]
    fn no_first_frame_fires_after_patience_and_is_throttled() {
        let mut state = RecoveryState::default();
        let due = state.started + NO_FIRST_FRAME_PATIENCE;
        assert_eq!(
            state.advance(None, due),
            Some(KeyframeRequest {
                reason: KeyframeReason::NoFirstFrame,
                attempted: 1
            })
        );
        assert_eq!(state.advance(None, due + Duration::from_secs(1)), None);
        assert_eq!(
            state.advance(None, due + REQUEST_COOLDOWN),
            Some(KeyframeRequest {
                reason: KeyframeReason::NoFirstFrame,
                attempted: 2
            })
        );
    }

    #[test]
    fn output_stops_requests() {
        let mut state = RecoveryState::default();
        let due = state.started + NO_FIRST_FRAME_PATIENCE;
        state.advance(Some(RecoveryEvent::Output), due);
        assert_eq!(state.advance(None, due + Duration::from_secs(10)), None);
    }

    #[test]
    fn decode_error_arms_after_its_own_patience() {
        let mut state = RecoveryState::default();
        let armed = state.started + Duration::from_millis(100);
        assert_eq!(state.advance(Some(RecoveryEvent::DecodeError), armed), None);
        assert_eq!(
            state.advance(None, armed + DECODE_ERROR_PATIENCE),
            Some(KeyframeRequest {
                reason: KeyframeReason::DecodeError,
                attempted: 1
            })
        );
    }

    #[test]
    fn decoder_restart_rearms_the_startup_window() {
        let mut state = RecoveryState::default();
        let due = state.started + NO_FIRST_FRAME_PATIENCE;
        state.advance(Some(RecoveryEvent::Output), due);
        let restarted = due + Duration::from_secs(10);
        state.advance(Some(RecoveryEvent::DecoderRestart), restarted);
        assert_eq!(state.advance(None, restarted), None);
        assert_eq!(
            state.advance(None, restarted + NO_FIRST_FRAME_PATIENCE),
            Some(KeyframeRequest {
                reason: KeyframeReason::NoFirstFrame,
                attempted: 1
            })
        );
    }

    #[test]
    fn lifetime_cap_stops_a_broken_host_from_being_spammed() {
        let mut state = RecoveryState::default();
        let mut now = state.started + NO_FIRST_FRAME_PATIENCE;
        for _ in 0..MAX_REQUESTS {
            assert!(state.advance(None, now).is_some());
            now += REQUEST_COOLDOWN;
        }
        assert_eq!(state.advance(None, now), None);
    }
}
