//! Embed the Windows executable icon.
//!
//! This puts the openHC mark on the *file* — Explorer, the taskbar, Alt-Tab —
//! which a runtime `with_icon` call cannot do (that only paints the live
//! window). It is a Windows PE resource, so it is a no-op on macOS and Linux,
//! where a file icon needs an application bundle rather than an embedded
//! resource. Icon embedding is cosmetic, so a failure warns instead of failing
//! the build.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the .exe icon: {e}");
        }
    }
    // Rebuild the resource if the icon changes.
    println!("cargo:rerun-if-changed=icon.ico");
}
