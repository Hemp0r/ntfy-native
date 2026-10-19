//! Manual check of autostart: `--state`, `--enable`, `--disable` or
//! `--repair`; prints the state afterwards.
//!
//! macOS: `scripts/macos-dev-bundle.sh --example autostart_probe`, then run
//! `target/ntfy-native.app/Contents/MacOS/ntfy-native --enable`.
//!
//! Windows: `cargo run --example autostart_probe -- --enable`.

fn main() -> anyhow::Result<()> {
    use ntfy_native::autostart::platform_autostart;

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter("debug")
        .init();

    let autostart = platform_autostart(std::env::current_exe()?);
    match std::env::args().nth(1).as_deref() {
        Some("--state") | None => {}
        Some("--enable") => {
            autostart.set(true)?;
        }
        Some("--disable") => {
            autostart.set(false)?;
        }
        Some("--repair") => autostart.repair()?,
        Some(other) => anyhow::bail!("unknown argument {other:?}"),
    }
    println!("{:?}", autostart.state());
    Ok(())
}
