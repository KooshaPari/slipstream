//! Embeds the Windows version resource and application icon into the tray binary.
//!
//! Without this the exe shows the generic Windows icon in Explorer, the taskbar and Alt-Tab, which
//! is exactly what the operator saw. Linux builds skip it entirely.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    println!("cargo:rerun-if-changed=assets/slipstream.ico");
    println!("cargo:rerun-if-changed=build.rs");

    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/slipstream.ico");
    res.set("ProductName", "Slipstream");
    res.set("FileDescription", "Slipstream tray companion");
    res.set("LegalCopyright", "Slipstream contributors");
    if let Ok(version) = std::env::var("CARGO_PKG_VERSION") {
        res.set("ProductVersion", &version);
        res.set("FileVersion", &version);
    }

    if let Err(err) = res.compile() {
        // A missing resource compiler should not silently ship an iconless binary, but it also
        // must not break non-Windows toolchains that happen to set the target var.
        panic!("failed to compile Windows resources for slipstream-tray: {err}");
    }
}
