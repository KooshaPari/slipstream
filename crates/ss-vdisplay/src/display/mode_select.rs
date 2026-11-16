//! Nearest-supported display mode selection, shared by every backend that drives a real head.
//!
//! A client may ask for a mode the output cannot hold (a laptop panel asked for `640x360@144`,
//! or any refresh the panel never advertises). Refusing the session is the worst outcome: the
//! user gets a black window and the pipeline retries a refusal that will never succeed. The
//! right answer is the closest mode the output *does* support plus a scaled picture, and the
//! caller is told the request was not honored so the UI can say so instead of scaling silently.
//!
//! Pure on purpose: the preference order is the part that must be provable without a display.

use anyhow::{bail, Result};

use crate::Mode;

/// Marker carried by every unrecoverable mode-selection error. The session's pipeline retry
/// loop matches it to fail once: a driver that refuses a mode will refuse it again, so the eight
/// backoff attempts are pure latency on the way to the same black window.
pub const MODE_REFUSED_MARKER: &str = "vdisplay-mode-refused";

/// What the output will actually show, and whether that is what was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeChoice {
    /// The mode the client asked for — kept for reporting so the session can say
    /// "requested A, showing B" rather than only knowing what it got.
    pub requested: Mode,
    /// The mode that will be applied.
    pub applied: Mode,
    /// `true` when `applied != requested`.
    pub fallback: bool,
}

impl ModeChoice {
    fn exact(mode: Mode) -> Self {
        ModeChoice {
            requested: mode,
            applied: mode,
            fallback: false,
        }
    }
}

/// Modes an output reports may repeat a resolution at several rates; duplicates are harmless
/// here but are collapsed so the aspect/area steps are not biased by how often a driver repeats
/// one entry.
fn dedup(modes: &[Mode]) -> Vec<Mode> {
    let mut out: Vec<Mode> = Vec::with_capacity(modes.len());
    for &m in modes {
        if m.width == 0 || m.height == 0 || m.refresh_hz == 0 {
            continue;
        }
        if !out.contains(&m) {
            out.push(m);
        }
    }
    out
}

/// Aspect-ratio agreement, in ratio units: 16:9 and 1920x1080 agree exactly; 16:10 does not.
const ASPECT_TOLERANCE: f64 = 0.01;

fn aspect(mode: Mode) -> f64 {
    mode.width as f64 / mode.height as f64
}

fn pixels(mode: Mode) -> u64 {
    mode.width as u64 * mode.height as u64
}

/// Closest rate to `want` in a same-resolution group. A lower rate wins ties: it is the one the
/// output demonstrably rates for that resolution, and overdriving is the thing that got refused.
fn closest_rate(candidates: &[Mode], want: u32) -> Mode {
    let mut best = candidates[0];
    for &m in &candidates[1..] {
        let key = |m: Mode| (m.refresh_hz.abs_diff(want), m.refresh_hz);
        if key(m) < key(best) {
            best = m;
        }
    }
    best
}

/// Pick the mode to apply for `requested` out of what `supported` actually offers.
///
/// Preference order, first match wins:
/// 1. the exact mode (nothing to report);
/// 2. the same resolution at a lower refresh — the common laptop case, and the closest thing to
///    "what was asked" that visibly works;
/// 3. the same resolution at any other refresh (nearest, lower on a tie) when nothing lower exists;
/// 4. the nearest aspect-correct resolution, by pixel count, then by rate closeness;
/// 5. the nearest resolution overall, by pixel count, then by rate closeness.
///
/// An empty supported list is an error, not a silent identity: a head that reports no modes
/// cannot be driven, and the caller must fail once with that fact instead of retrying.
pub fn choose_mode(requested: Mode, supported: &[Mode]) -> Result<ModeChoice> {
    let modes = dedup(supported);
    if modes.is_empty() {
        bail!(
            "{MODE_REFUSED_MARKER}: display mode {}x{}@{} not selectable: the output reports no \
             supported modes",
            requested.width,
            requested.height,
            requested.refresh_hz
        );
    }
    if let Some(&exact) = modes.iter().find(|&&m| m == requested) {
        return Ok(ModeChoice::exact(exact));
    }

    let same_res: Vec<Mode> = modes
        .iter()
        .copied()
        .filter(|m| m.width == requested.width && m.height == requested.height)
        .collect();
    if !same_res.is_empty() {
        // Prefer the highest rate below the request (staying inside what the output rates),
        // then the nearest rate at all.
        let below: Vec<Mode> = same_res
            .iter()
            .copied()
            .filter(|m| m.refresh_hz < requested.refresh_hz)
            .collect();
        let applied = if below.is_empty() {
            closest_rate(&same_res, requested.refresh_hz)
        } else {
            *below
                .iter()
                .max_by_key(|m| m.refresh_hz)
                .expect("non-empty")
        };
        return Ok(fallback(requested, applied));
    }

    let want_area = pixels(requested);
    let want_aspect = aspect(requested);
    let mut aspect_matches: Vec<Mode> = modes
        .iter()
        .copied()
        .filter(|m| (aspect(*m) - want_aspect).abs() <= ASPECT_TOLERANCE)
        .collect();

    let applied = if aspect_matches.is_empty() {
        // Last resort: the nearest resolution overall still beats a black window.
        *modes
            .iter()
            .min_by_key(|m| (pixels(**m).abs_diff(want_area), m.refresh_hz))
            .expect("non-empty")
    } else {
        aspect_matches.sort_by_key(|m| (pixels(*m).abs_diff(want_area), m.refresh_hz));
        let group: Vec<Mode> = aspect_matches
            .iter()
            .copied()
            .filter(|m| pixels(*m) == pixels(aspect_matches[0]))
            .collect();
        closest_rate(&group, requested.refresh_hz)
    };
    Ok(fallback(requested, applied))
}

fn fallback(requested: Mode, applied: Mode) -> ModeChoice {
    debug_assert_ne!(requested, applied, "an exact match is returned earlier");
    ModeChoice {
        requested,
        applied,
        fallback: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(width: u32, height: u32, refresh_hz: u32) -> Mode {
        Mode {
            width,
            height,
            refresh_hz,
        }
    }

    /// A laptop-class panel: 1080p at 60 Hz only, plus its scaled-down modes.
    fn panel() -> Vec<Mode> {
        vec![
            m(1920, 1080, 60),
            m(1600, 900, 60),
            m(1280, 720, 60),
            m(1024, 768, 60),
            m(800, 600, 60),
        ]
    }

    #[test]
    fn exact_mode_wins_and_is_not_a_fallback() {
        let choice = choose_mode(m(1280, 720, 60), &panel()).unwrap();
        assert_eq!(choice.applied, m(1280, 720, 60));
        assert_eq!(choice.requested, m(1280, 720, 60));
        assert!(!choice.fallback);
    }

    #[test]
    fn same_resolution_lower_refresh_beats_any_resolution_change() {
        // 1440p@144 asked, panel does 1440p@120 and 1440p@60 plus a closer-in-pixels 1080p@240.
        // Staying at 1440p is the right answer even though 1440p@60 is further from 144 Hz than
        // a hypothetical 1080p@144 would be: the resolution is what the user chose.
        let supported = [
            m(2560, 1440, 120),
            m(2560, 1440, 60),
            m(1920, 1080, 240),
            m(1920, 1080, 144),
        ];
        let choice = choose_mode(m(2560, 1440, 144), &supported).unwrap();
        assert_eq!(choice.applied, m(2560, 1440, 120));
        assert!(choice.fallback);
    }

    #[test]
    fn same_resolution_picks_the_highest_lower_rate() {
        let supported = [m(1920, 1080, 50), m(1920, 1080, 60), m(1920, 1080, 100)];
        let choice = choose_mode(m(1920, 1080, 120), &supported).unwrap();
        assert_eq!(choice.applied, m(1920, 1080, 100));
    }

    #[test]
    fn same_resolution_only_above_falls_back_to_nearest_rate() {
        let supported = [m(1920, 1080, 75), m(1920, 1080, 120)];
        let choice = choose_mode(m(1920, 1080, 60), &supported).unwrap();
        assert_eq!(choice.applied, m(1920, 1080, 75));
    }

    #[test]
    fn resolution_change_prefers_aspect_correct_then_pixel_count() {
        // Asked for 640x360@144 (16:9). Among the panel's 16:9 modes the nearest by area is
        // 720p, not 1080p — "nearest" means nearest to what was asked, not biggest.
        let choice = choose_mode(m(640, 360, 144), &panel()).unwrap();
        assert_eq!(choice.applied, m(1280, 720, 60));
        assert!(choice.fallback);
        // 4:3 1024x768 is closer in pixel count than 1080p but is the wrong aspect; it must lose.
        assert_ne!(choice.applied, m(1024, 768, 60));
    }

    #[test]
    fn no_aspect_match_still_picks_the_nearest_pixel_count() {
        let supported = [m(1024, 768, 60), m(800, 600, 60), m(2560, 1600, 60)];
        // 900x600 is 3:2; 16:10 (1.6) is 0.1 away and 4:3 (1.333) is 0.167 away, so no candidate
        // is aspect-correct within tolerance and pixel count decides: 800x600 (480k) is nearest
        // to 540k, ahead of 1024x768 (786k).
        let choice = choose_mode(m(900, 600, 60), &supported).unwrap();
        assert_eq!(choice.applied, m(800, 600, 60));
    }

    #[test]
    fn no_supported_modes_is_a_clear_error() {
        let err = choose_mode(m(640, 360, 144), &[]).unwrap_err().to_string();
        assert!(err.contains("no supported modes"), "unhelpful error {err:?}");
        // Degenerate entries (zero dimension or rate) are not "supported" either.
        let err = choose_mode(m(640, 360, 144), &[m(0, 0, 0), m(1920, 1080, 0)])
            .unwrap_err()
            .to_string();
        assert!(err.contains("no supported modes"), "unhelpful error {err:?}");
    }

    #[test]
    fn duplicate_entries_do_not_change_the_choice() {
        let supported = [m(1920, 1080, 60), m(1920, 1080, 60), m(1280, 720, 60)];
        let choice = choose_mode(m(1280, 720, 144), &supported).unwrap();
        assert_eq!(choice.applied, m(1280, 720, 60));
    }
}
