//! Manual check of autostart: `--state`, `--enable`, `--disable` or
//! `--repair`; prints the state afterwards. macOS: a leading
//! `--launch-agent` skips the login item, as if the signature were refused.
//!
//! macOS: `scripts/macos-dev-bundle.sh --example autostart_probe`, then run
//! `target/ntfy-native.app/Contents/MacOS/ntfy-native --enable`.
//!
//! Windows: `cargo run --example autostart_probe -- --enable`.

use std::sync::Arc;

use ntfy_native::autostart::{Autostart, platform_autostart};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter("debug")
        .init();

    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let launch_agent = args.first().is_some_and(|a| a == "--launch-agent");
    if launch_agent {
        args.remove(0);
    }
    let program = std::env::current_exe()?;
    let autostart: Arc<dyn Autostart> = if launch_agent {
        launch_agent_only(program)?
    } else {
        platform_autostart(program)
    };
    match args.first().map(String::as_str) {
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

#[cfg(target_os = "macos")]
fn launch_agent_only(program: std::path::PathBuf) -> anyhow::Result<Arc<dyn Autostart>> {
    Ok(Arc::new(
        ntfy_native::autostart::macos::MacAutostart::launch_agent_only(program)?,
    ))
}

#[cfg(not(target_os = "macos"))]
fn launch_agent_only(_program: std::path::PathBuf) -> anyhow::Result<Arc<dyn Autostart>> {
    anyhow::bail!("--launch-agent is macOS only")
}
