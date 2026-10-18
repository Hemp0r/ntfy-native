use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::Result;
use lexopt::prelude::*;

#[derive(Debug, Default)]
pub struct Cli {
    pub config: Option<PathBuf>,
    pub import: Option<PathBuf>,
}

#[derive(Debug)]
pub enum CliAction {
    Run(Cli),
    Version,
}

/// Arguments Windows COM activation passes to the process; not real flags.
const COM_ACTIVATION_ARGS: [&str; 2] = ["-Embedding", "-ToastActivated"];

/// Parses arguments without the program name.
pub fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<CliAction> {
    let args = args.into_iter().filter(|a| {
        !COM_ACTIVATION_ARGS
            .iter()
            .any(|c| a.to_str().is_some_and(|s| s.eq_ignore_ascii_case(c)))
    });
    let mut parser = lexopt::Parser::from_args(args);
    let mut cli = Cli::default();
    while let Some(arg) = parser.next()? {
        match arg {
            Long("config") => cli.config = Some(parser.value()?.into()),
            Long("import") => cli.import = Some(parser.value()?.into()),
            Long("version") | Short('V') => return Ok(CliAction::Version),
            _ => return Err(arg.unexpected().into()),
        }
    }
    Ok(CliAction::Run(cli))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::Path;

    fn args<const N: usize>(a: [&str; N]) -> Vec<OsString> {
        a.map(OsString::from).to_vec()
    }

    #[test]
    fn parses_config_flag() {
        let a = parse_args(args(["--config", "/tmp/c.yaml"])).unwrap();
        assert!(
            matches!(a, CliAction::Run(Cli { config: Some(p), import: None }) if p == Path::new("/tmp/c.yaml"))
        );
    }

    #[test]
    fn parses_import_flag() {
        let a = parse_args(args(["--import", "x.yaml"])).unwrap();
        assert!(
            matches!(a, CliAction::Run(Cli { config: None, import: Some(p) }) if p == Path::new("x.yaml"))
        );
    }

    #[test]
    fn version_flag() {
        assert!(matches!(
            parse_args(args(["--version"])).unwrap(),
            CliAction::Version
        ));
    }

    #[test]
    fn ignores_com_activation_args() {
        let a = parse_args(args(["-Embedding", "-ToastActivated"])).unwrap();
        assert!(matches!(
            a,
            CliAction::Run(Cli {
                config: None,
                import: None
            })
        ));
    }

    #[test]
    fn rejects_unknown_flag() {
        assert!(parse_args(args(["--bogus"])).is_err());
    }
}
