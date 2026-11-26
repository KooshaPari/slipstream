//! Embeds the Windows version resource and the per-binary application icon into this binary.
//!
//! Without this the exe shows the generic Windows icon in Explorer, the taskbar and Alt-Tab. Linux
//! builds skip it entirely.

/// Absolute path to this binary's Windows icon.
///
/// The icons live in the repository root's `assets/icons/`, but a build script runs with its crate's
/// manifest directory as the working directory, so a relative path would resolve against the wrong
/// place. `CARGO_MANIFEST_DIR` is the crate root and is stable across cargo invocations.
fn icon_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icons/probe.ico")
}

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    println!("cargo:rerun-if-changed=build.rs");

    let icon = icon_path();
    println!("cargo:rerun-if-changed={}", icon.display());

    let mut res = winresource::WindowsResource::new();
    res.set_icon(&icon.to_string_lossy());
    res.set("ProductName", "Slipstream");
    res.set("FileDescription", "Slipstream probe and latency diagnostic client");
    res.set("LegalCopyright", "Slipstream contributors");
    if let Ok(version) = std::env::var("CARGO_PKG_VERSION") {
        res.set("ProductVersion", &version);
        res.set("FileVersion", &version);
    }

    if let Err(err) = res.compile() {
        // A missing resource compiler should not silently ship an iconless binary, but it also
        // must not break non-Windows toolchains that happen to set the target var.
        panic!("failed to compile Windows resources for slipstream-probe: {err}");
    }
}
