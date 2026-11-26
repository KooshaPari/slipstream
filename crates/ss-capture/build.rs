//! Probe whether the installed libspa headers export `SPA_VIDEO_TRANSFER_SMPTE2084`.
//!
//! `libspa-sys` bindgens the SYSTEM headers (system-deps → pkg-config include paths), so a symbol
//! exists in `spa::sys` exactly when the header the build points at defines it. PipeWire only grew
//! the BT2020_10/SMPTE2084/ARIB_STD_B67 block in `spa/param/video/color.h` after 1.0, and the
//! distro builders (Ubuntu 24.04 noble for the .deb) ship the older headers — naming the constant
//! there is an E0425, which is why the HDR builder hardcodes the id and this probe exists.
//!
//! The probe sets `libspa_has_smpte2084`, consumed by the `pw_pods` test: where the symbol exists
//! the test cross-checks the hardcoded id against libspa's enum; where it does not, the test still
//! asserts the id is 14 (its wire position), so a wrong literal is caught on old headers too.
//!
//! Deliberately a no-op off Linux: only `platform::linux::pw_pods` references the libspa binding,
//! and a Windows/macOS build must not acquire a pkg-config or header dependency from this.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The one symbol whose presence is being probed.
const SYMBOL: &str = "SPA_VIDEO_TRANSFER_SMPTE2084";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-check-cfg=cfg(libspa_has_smpte2084)");
    // Re-probe when the header search path or an explicit override changes.
    for var in [
        "SS_CAPTURE_LIBSPA_INCLUDE",
        "PKG_CONFIG_PATH",
        "PKG_CONFIG_LIBDIR",
        "PKG_CONFIG_SYSROOT_DIR",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "linux" {
        return;
    }

    let include_dirs = libspa_include_dirs();
    if include_dirs.iter().any(|dir| header_defines(dir, SYMBOL)) {
        println!("cargo:rustc-cfg=libspa_has_smpte2084");
    }
}

/// Every include dir the libspa headers may live under: an explicit override first (cross builds
/// and boxes without the dev package registered), then whatever `libspa-0.2.pc` reports — the same
/// pkg-config the `libspa-sys` build uses, so a symbol found here is a symbol bindgen will emit —
/// then the conventional prefixes.
fn libspa_include_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Ok(dir) = env::var("SS_CAPTURE_LIBSPA_INCLUDE") {
        if !dir.is_empty() {
            dirs.push(PathBuf::from(dir));
        }
    }

    if let Ok(out) = Command::new("pkg-config")
        .args(["--cflags-only-I", "libspa-0.2"])
        .output()
    {
        if out.status.success() {
            let flags = String::from_utf8_lossy(&out.stdout);
            for flag in flags.split_whitespace() {
                if let Some(path) = flag.strip_prefix("-I") {
                    if !path.is_empty() {
                        dirs.push(PathBuf::from(path));
                    }
                }
            }
        }
    }

    for prefix in [
        "/usr/include/spa-0.2",
        "/usr/local/include/spa-0.2",
        "/usr/include",
        "/usr/local/include",
    ] {
        dirs.push(PathBuf::from(prefix));
    }

    dirs.sort();
    dirs.dedup();
    dirs
}

/// True when `<dir>/spa/param/video/color.h` DEFINES `symbol` (an uncommented enum variant or
/// `#define` line starting with it), not merely mentions it in prose or a comment.
fn header_defines(include_dir: &Path, symbol: &str) -> bool {
    let Ok(header) = std::fs::read_to_string(include_dir.join("spa/param/video/color.h")) else {
        return false;
    };
    header.lines().any(|line| {
        let line = line.trim_start();
        let line = line.strip_prefix("#define ").unwrap_or(line);
        // A mention inside a `//`/`/*` comment is not a definition; bindgen cannot see it either.
        if line.starts_with("//") || line.starts_with('*') || line.starts_with("/*") {
            return false;
        }
        match line.strip_prefix(symbol) {
            // `SPA_VIDEO_TRANSFER_SMPTE2084,` / `= 14` — but not `..._ARIB_STD_B67` etc.
            Some(rest) => !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'),
            None => false,
        }
    })
}
