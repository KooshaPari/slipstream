//! Build script: stamps the build version, and embeds the Windows version resource and the
//! per-binary application icon.
//!
//! Two independent jobs:
//!
//! 1. Build provenance: stamp the exact package/build version into the binary so a running host
//!    can report what it is (mgmt /health, the startup log, `--version`) and a stale/shadowed
//!    install is detectable. CI (deb.yml / rpm.yml / the RPM spec / the PKGBUILD) sets
//!    SLIPSTREAM_BUILD_VERSION to the full package version (e.g. `0.2.0~ci120.g802e98d`); a plain
//!    `cargo build` falls back to the crate version. Deliberately NOT git-derived — the RPM builds
//!    from a `git archive` tarball with no .git, and a hard git dependency would break it.
//! 2. Windows resources: without the embedded icon the exe shows the generic Windows icon in
//!    Explorer, the taskbar and Alt-Tab. Linux builds skip that entirely.

/// Absolute path to this binary's Windows icon.
///
/// The icons live in the repository root's `assets/icons/`, but a build script runs with its crate's
/// manifest directory as the working directory, so a relative path would resolve against the wrong
/// place. `CARGO_MANIFEST_DIR` is the crate root and is stable across cargo invocations.
fn icon_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icons/host.ico")
}

fn main() {
    stamp_version();
    embed_windows_resources();
}

/// Stamp `SLIPSTREAM_VERSION` into the binary. Runs on every platform: the host reports its
/// version on every OS, not just Windows.
fn stamp_version() {
    let version = std::env::var("SLIPSTREAM_BUILD_VERSION")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".into()));
    println!("cargo:rustc-env=SLIPSTREAM_VERSION={version}");
    println!("cargo:rerun-if-env-changed=SLIPSTREAM_BUILD_VERSION");
}

/// Embed the Windows icon and version resource. Windows-only: every other target is a no-op.
fn embed_windows_resources() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    println!("cargo:rerun-if-changed=build.rs");

    let icon = icon_path();
    println!("cargo:rerun-if-changed={}", icon.display());

    let mut res = winresource::WindowsResource::new();
    res.set_icon(&icon.to_string_lossy());
    res.set("ProductName", "Slipstream");
    res.set("FileDescription", "Slipstream streaming host");
    res.set("LegalCopyright", "Slipstream contributors");
    if let Ok(version) = std::env::var("CARGO_PKG_VERSION") {
        res.set("ProductVersion", &version);
        res.set("FileVersion", &version);
    }

    if let Err(err) = res.compile() {
        // A missing resource compiler should not silently ship an iconless binary, but it also
        // must not break non-Windows toolchains that happen to set the target var.
        panic!("failed to compile Windows resources for slipstream-host: {err}");
    }
}
