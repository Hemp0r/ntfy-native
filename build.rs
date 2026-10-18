//! Embeds the app icon into the Windows executable.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    let target_windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    // Cross-checks from other hosts (clippy for the Windows target on macOS)
    // have no resource compiler.
    let host_windows = std::env::var("HOST").is_ok_and(|host| host.contains("windows"));
    if target_windows && host_windows {
        winresource::WindowsResource::new()
            .set_icon("assets/icon.ico")
            .compile()
            .expect("embedding the icon into the Windows executable");
    }
}
