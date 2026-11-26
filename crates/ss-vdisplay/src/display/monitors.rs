//! Physical-monitor enumeration — "what heads does this host actually have?".
//!
//! This is the read-only counterpart to the rest of this crate: everything else here *creates* and
//! owns virtual outputs, while this module only *reports* the heads the compositor already has, so
//! an operator can pin capture at one of them (`SLIPSTREAM_CAPTURE_MONITOR`) and the console can
//! render a picker. See `design/per-monitor-portal-capture.md` §5.1.
//!
//! It lives in ss-vdisplay because monitor enumeration is a **per-compositor** question and this
//! crate already speaks every one of those dialects — KWin's `kde_output_device_v2`, Mutter's
//! `DisplayConfig.GetCurrentState`, `swaymsg -t get_outputs`, `hyprctl -j monitors`. Each backend's
//! implementation sits beside the code that already talks to it; this module is the shared type and
//! the dispatch.
//!
//! **The geometry is the point.** `x`/`y` are what make a head *identifiable*: two monitors can
//! share a size (and then a size-keyed match is a coin flip — see `ss-inject`'s absolute-coordinate
//! region selection), but they can never share an origin in the compositor's global space.

use crate::Compositor;
use anyhow::{bail, Result};

/// One head as the compositor currently reports it. Logical (post-scale) geometry throughout —
/// the same coordinate space libei regions and compositor layout use, *not* pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct PhysicalMonitor {
    /// Connector name — `DP-1`, `HDMI-A-2`, `eDP-1`. The id `SLIPSTREAM_CAPTURE_MONITOR` names.
    pub connector: String,
    /// Human label for a picker (`make model`, else the connector). Never used for matching.
    pub description: String,
    /// Current mode, in pixels.
    pub width: u32,
    pub height: u32,
    /// Refresh in mHz (60000 = 60 Hz). 0 when the backend doesn't report it.
    pub refresh_mhz: u32,
    /// Top-left in the compositor's global logical space — the identity key (see the module doc).
    pub x: i32,
    pub y: i32,
    /// Logical scale factor (1.0 when unreported).
    pub scale: f64,
    /// The compositor's primary/focused head, when it says.
    pub primary: bool,
    /// Enabled (driven). A disabled head is still listed, so "why can't I pick it?" has an answer.
    pub enabled: bool,
    /// **Best-effort**: this output is one WE created (a managed virtual display), not a real head.
    /// Only KWin can say so reliably (managed outputs carry a name prefix); the other backends
    /// name virtual outputs indistinguishably from physical ones, so this stays `false` there
    /// rather than guessing. Callers use it to grey out nonsense choices, never to filter blindly.
    pub managed: bool,
}

/// Build the picker label from a compositor's make/model, falling back to the connector.
///
/// Compositors fill unknown fields with the literal string `"Unknown"` rather than leaving them
/// empty (seen on-glass: sway reports `"Unknown Unknown"` for a headless output), so treat that as
/// absent too — a picker row reading "Unknown Unknown" is worse than one reading "DP-1".
pub(crate) fn describe(make: &str, model: &str, connector: &str) -> String {
    let known = |s: &str| {
        let s = s.trim();
        !s.is_empty() && !s.eq_ignore_ascii_case("unknown")
    };
    let label = [make, model]
        .iter()
        .map(|s| s.trim())
        .filter(|s| known(s))
        .collect::<Vec<_>>()
        .join(" ");
    if label.is_empty() {
        connector.to_string()
    } else {
        label
    }
}

impl PhysicalMonitor {
    /// `1920x1080@60` — for logs and pickers.
    pub fn mode_label(&self) -> String {
        if self.refresh_mhz == 0 {
            format!("{}x{}", self.width, self.height)
        } else {
            format!(
                "{}x{}@{}",
                self.width,
                self.height,
                (self.refresh_mhz as f64 / 1000.0).round() as u32
            )
        }
    }
}

/// The trailing status tags for one head, in a fixed order.
fn row_tags(m: &PhysicalMonitor, pinned: Option<&str>) -> Vec<&'static str> {
    let mut tags = Vec::new();
    if m.primary {
        tags.push("primary");
    }
    if !m.enabled {
        tags.push("disabled");
    }
    if m.managed {
        tags.push("slipstream virtual display");
    }
    if pinned.is_some_and(|p| p.eq_ignore_ascii_case(&m.connector)) {
        tags.push("PINNED");
    }
    tags
}

/// Render the body of `list-monitors`: one line per head, with the columns aligned across *all*
/// rows rather than per row.
///
/// Two formatting rules matter here:
///
/// * **Signs.** Origins are signed (`x`/`y` are `i32`; a head left of or above the primary is
///   negative), so the sign is part of the value (`{:+}`). Formatting a signed value with a
///   literal `+` in front produced `+-1680`; this renders `-1680` and `+0`.
/// * **Widths.** Column widths are computed from the rows themselves. A hard-coded connector width
///   breaks as soon as a name is wider (`\\.\DISPLAY10`), and a right-aligned mode label makes the
///   gap before `at` differ between a 12- and 13-character mode. Deriving each width from the data
///   keeps every column lining up regardless of connector length, mode label, or whether the row
///   carries a `[primary]`-style tag.
///
/// The description is padded to the widest one so the tag column starts at the same place on
/// every row; rows with no tags are right-trimmed so they do not carry trailing spaces.
pub fn format_rows(monitors: &[PhysicalMonitor], pinned: Option<&str>) -> Vec<String> {
    let modes: Vec<String> = monitors.iter().map(PhysicalMonitor::mode_label).collect();
    let positions: Vec<String> = monitors
        .iter()
        .map(|m| format!("{:+},{:+}", m.x, m.y))
        .collect();
    let scales: Vec<String> = monitors.iter().map(|m| m.scale.to_string()).collect();

    let connector_w = monitors
        .iter()
        .map(|m| m.connector.len())
        .max()
        .unwrap_or(0);
    let mode_w = modes.iter().map(String::len).max().unwrap_or(0);
    let position_w = positions.iter().map(String::len).max().unwrap_or(0);
    let scale_w = scales.iter().map(String::len).max().unwrap_or(0);
    let description_w = monitors
        .iter()
        .map(|m| m.description.len())
        .max()
        .unwrap_or(0);

    monitors
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let row = format!(
                "  {:<connector_w$}  {:<mode_w$}  at {:<position_w$}  scale {:<scale_w$}  {:<description_w$}",
                m.connector, modes[i], positions[i], scales[i], m.description,
            );
            let tags = row_tags(m, pinned);
            if tags.is_empty() {
                row.trim_end().to_string()
            } else {
                format!("{row}  [{}]", tags.join(", "))
            }
        })
        .collect()
}

/// Every head `compositor` reports, in the compositor's own order.
///
/// Errors when the backend can't be reached (compositor not running, IPC unavailable) — that is
/// deliberately distinct from `Ok(vec![])`, which means "reached it, it has no heads" (a headless
/// session). Callers that only want to *offer* a picker can treat both as "nothing to show";
/// callers resolving a pinned monitor must not (see [`resolve`]).
pub fn list(compositor: Compositor) -> Result<Vec<PhysicalMonitor>> {
    match compositor {
        #[cfg(target_os = "linux")]
        Compositor::Kwin => crate::kwin_output_mgmt::list_monitors(),
        #[cfg(target_os = "linux")]
        Compositor::Mutter => crate::mutter::list_monitors(),
        #[cfg(target_os = "linux")]
        Compositor::Wlroots => crate::wlroots::list_monitors(),
        #[cfg(target_os = "linux")]
        Compositor::Hyprland => crate::hyprland::list_monitors(),
        // gamescope is only *sometimes* nested. A Bazzite/SteamOS Game Mode session is the DRM
        // master and drives a real connector; the ones this crate spawns are headless and drive
        // none. `gamescope::list_monitors` tells those apart and answers an empty list — not an
        // error — for the nested/headless shapes, which is what this arm used to hard-code for all
        // of them (and why the picker was permanently empty on a TV box).
        #[cfg(target_os = "linux")]
        Compositor::Gamescope => crate::gamescope::list_monitors(),
        // Windows reports its real heads via GDI (`EnumDisplayDevicesW`); the IDD virtual
        // display reports its own heads once it lands (same `PhysicalMonitor` shape, so the
        // picker + pin resolution work unchanged).
        #[cfg(target_os = "windows")]
        Compositor::Windows => crate::windows::list_monitors(),
        #[cfg(target_os = "windows")]
        _ => bail!("compositor '{compositor:?}' is Linux-only on this host"),
        // A Linux host never drives the Windows backend (an explicit pin naming it
        // fails at `detect` with the accepted names).
        #[cfg(target_os = "linux")]
        Compositor::Windows => {
            bail!("the Windows desktop backend cannot run on Linux (SLIPSTREAM_COMPOSITOR=windows)")
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        _ => bail!("physical-monitor enumeration is implemented for the Linux backends only"),
    }
}

/// Resolve a configured monitor name against `monitors`, exactly then case-insensitively.
///
/// **A miss is a hard error carrying the available names**, never a silent fall-back to some other
/// head: an operator who pinned `DP-2` and gets `DP-1` streamed has been shown the wrong screen,
/// which is worse than a host that refuses to start a session and says why
/// (`design/per-monitor-portal-capture.md` §5.2).
pub fn resolve<'a>(monitors: &'a [PhysicalMonitor], want: &str) -> Result<&'a PhysicalMonitor> {
    if let Some(m) = monitors.iter().find(|m| m.connector == want) {
        return Ok(m);
    }
    if let Some(m) = monitors
        .iter()
        .find(|m| m.connector.eq_ignore_ascii_case(want))
    {
        return Ok(m);
    }
    let available = monitors
        .iter()
        .map(|m| m.connector.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if available.is_empty() {
        bail!("no monitor named {want:?} — this compositor reports no monitors at all");
    }
    bail!("no monitor named {want:?} — this host has: {available}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(connector: &str) -> PhysicalMonitor {
        PhysicalMonitor {
            connector: connector.into(),
            description: String::new(),
            width: 1920,
            height: 1080,
            refresh_mhz: 60000,
            x: 0,
            y: 0,
            scale: 1.0,
            primary: false,
            enabled: true,
            managed: false,
        }
    }

    #[test]
    fn resolve_matches_exactly_then_case_insensitively() {
        let ms = [mon("DP-1"), mon("HDMI-A-2")];
        assert_eq!(resolve(&ms, "DP-1").unwrap().connector, "DP-1");
        assert_eq!(resolve(&ms, "hdmi-a-2").unwrap().connector, "HDMI-A-2");
    }

    #[test]
    fn resolve_prefers_the_exact_match_over_a_case_fold() {
        // Connector names are case-sensitive to the compositor; if both spellings exist, the
        // exact one wins rather than whichever the fold happened to reach first.
        let ms = [mon("dp-1"), mon("DP-1")];
        assert_eq!(resolve(&ms, "DP-1").unwrap().connector, "DP-1");
    }

    #[test]
    fn a_miss_lists_what_is_available() {
        let ms = [mon("DP-1"), mon("HDMI-A-2")];
        let err = resolve(&ms, "DP-9").unwrap_err().to_string();
        assert!(err.contains("DP-1") && err.contains("HDMI-A-2"), "{err}");
    }

    #[test]
    fn a_miss_with_no_monitors_says_so() {
        let err = resolve(&[], "DP-1").unwrap_err().to_string();
        assert!(err.contains("no monitors at all"), "{err}");
    }

    /// Compositors write the literal "Unknown" instead of leaving make/model empty (sway does it
    /// for headless outputs), so a picker must not end up showing "Unknown Unknown".
    #[test]
    fn describe_falls_back_to_the_connector_for_empty_or_unknown_fields() {
        assert_eq!(describe("ACME", "U2720Q", "DP-1"), "ACME U2720Q");
        assert_eq!(describe("Unknown", "Unknown", "HEADLESS-1"), "HEADLESS-1");
        assert_eq!(describe("", "", "DP-1"), "DP-1");
        // A half-known pair keeps the half that means something.
        assert_eq!(describe("Unknown", "U2720Q", "DP-1"), "U2720Q");
        assert_eq!(describe("  ", "unknown", "DP-2"), "DP-2");
    }

    #[test]
    fn mode_label_drops_an_unknown_refresh() {
        let mut m = mon("DP-1");
        assert_eq!(m.mode_label(), "1920x1080@60");
        m.refresh_mhz = 0;
        assert_eq!(m.mode_label(), "1920x1080");
    }

    /// A head left of the primary reports a negative origin, which must render as `-1680`, never
    /// `+-1680`; a positive origin keeps its explicit `+`.
    #[test]
    fn negative_and_positive_origins_render_a_single_sign() {
        let right = mon(r"\\.\DISPLAY1"); // x = 0, y = 0
        let mut left = mon(r"\\.\DISPLAY2");
        left.x = -1680;
        left.y = 12;
        let rows = format_rows(&[right, left], None);
        assert!(rows[0].contains("at +0,+0"), "{}", rows[0]);
        assert!(rows[1].contains("at -1680,+12"), "{}", rows[1]);
        assert!(!rows[1].contains("+-"), "{}", rows[1]);
    }

    /// Columns line up across rows with different connector lengths, mode-label lengths, and the
    /// presence of a `[primary]` tag: `at`, `scale`, and the tag column start at one offset.
    #[test]
    fn columns_align_across_lengths_and_tags() {
        let mut a = mon(r"\\.\DISPLAY1");
        a.width = 2560;
        a.height = 1440;
        a.refresh_mhz = 144000;
        a.primary = true;
        a.description = "Dell U2720Q".into();
        let mut b = mon(r"\\.\DISPLAY10");
        b.width = 1680;
        b.height = 1050;
        b.refresh_mhz = 60000;
        b.x = -1680;
        b.y = 12;
        b.description = "LG".into();
        // Captured before the rows are rendered: the description column is padded to the widest.
        let description_pad = a.description.len() - b.description.len();
        let rows = format_rows(&[a, b], None);
        assert_eq!(
            rows[0].find(" at "),
            rows[1].find(" at "),
            "{} || {}",
            rows[0],
            rows[1]
        );
        assert_eq!(rows[0].find(" scale "), rows[1].find(" scale "), "{rows:?}");
        assert!(rows[0].contains("[primary]"), "{}", rows[0]);
        // The description is padded to the widest one, so the tag column starts at the same column
        // on every row. The tagless row is right-trimmed, so its trimmed length plus the padding it
        // gave up is the tag column.
        let tag_col = rows[0].find("[primary]").unwrap();
        assert_eq!(
            tag_col,
            rows[1].len() + description_pad + 2,
            "{} || {}",
            rows[0],
            rows[1]
        );
        assert_eq!(rows[1], rows[1].trim_end());
    }
}
