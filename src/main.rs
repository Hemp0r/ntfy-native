// Release builds are GUI apps on Windows: no console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ntfy_native::cli::{CliAction, parse_args};

fn main() -> anyhow::Result<()> {
    match parse_args(std::env::args_os().skip(1))? {
        CliAction::Version => println!("ntfy-native {}", env!("CARGO_PKG_VERSION")),
        CliAction::Run(cli) => ntfy_native::app::run(cli)?,
    }
    Ok(())
}
