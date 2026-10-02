//! Embeds the application icon and version metadata into the Windows executable.
//!
//! Runs on the host, so the target OS must be read from CARGO_CFG_TARGET_OS at runtime rather
//! than via cfg!. On non-Windows targets this is a no-op. Cross-compiling from Linux needs
//! x86_64-w64-mingw32-windres and -ar on PATH (winresource picks the prefix from TARGET).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/icon.ico")
        .set("ProductName", "nettest")
        .set("FileDescription", "nettest network troubleshooting server")
        .set("LegalCopyright", "Slayton's Technology Services")
        .set("CompanyName", "Slayton's Technology Services");
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed Windows icon/metadata: {e}");
    }
}
