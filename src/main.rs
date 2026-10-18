use ntfy_native::cli::{CliAction, parse_args};

fn main() -> anyhow::Result<()> {
    match parse_args(std::env::args_os().skip(1))? {
        CliAction::Version => println!("ntfy-native {}", env!("CARGO_PKG_VERSION")),
        CliAction::Run(_) => {}
    }
    Ok(())
}
