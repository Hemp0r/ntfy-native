use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use lexopt::prelude::*;

use crate::ui::layout::{AnchorRect, parse_anchor};

#[derive(Debug, Default)]
pub struct Cli {
    pub config: Option<PathBuf>,
    pub import: Option<PathBuf>,
}

#[derive(Debug)]
pub enum CliAction {
    Run(Cli),
    Version,
    Ui(UiArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiMode {
    Popover,
    Settings,
    Wizard,
    /// Test mode without a window.
    Probe,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UiArgs {
    pub mode: UiMode,
    /// Tray icon rectangle, physical pixels.
    pub anchor: Option<AnchorRect>,
    /// Monitor of the tray icon (x, y, w, h), physical pixels.
    pub monitor: Option<(f64, f64, f64, f64)>,
    /// Scale factor of that monitor.
    pub scale: Option<f32>,
    pub probe_ignore_close: bool,
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
    let mut mode = None;
    let mut anchor = None;
    let mut monitor = None;
    let mut scale = None;
    let mut probe_ignore_close = false;
    let mut ui_only = None;
    while let Some(arg) = parser.next()? {
        match arg {
            Long("config") => cli.config = Some(parser.value()?.into()),
            Long("import") => cli.import = Some(parser.value()?.into()),
            Long("version") | Short('V') => return Ok(CliAction::Version),
            Long("ui") => mode = Some(parse_mode(&parser.value()?.string()?)?),
            Long("anchor") => {
                let v = parser.value()?.string()?;
                anchor = Some(parse_anchor(&v).with_context(|| format!("invalid --anchor {v:?}"))?);
                ui_only = Some("--anchor");
            }
            Long("monitor") => {
                let v = parser.value()?.string()?;
                let r = parse_anchor(&v)
                    .filter(|r| r.w > 0.0 && r.h > 0.0)
                    .with_context(|| format!("invalid --monitor {v:?}"))?;
                monitor = Some((r.x, r.y, r.w, r.h));
                ui_only = Some("--monitor");
            }
            Long("scale") => {
                let v = parser.value()?.string()?;
                let f = v
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|f| f.is_finite() && *f > 0.0)
                    .with_context(|| format!("invalid --scale {v:?}"))?;
                scale = Some(f);
                ui_only = Some("--scale");
            }
            Long("probe-ignore-close") => {
                probe_ignore_close = true;
                ui_only = Some("--probe-ignore-close");
            }
            _ => return Err(arg.unexpected().into()),
        }
    }
    match mode {
        Some(_) if cli.config.is_some() || cli.import.is_some() => {
            bail!("--ui cannot be combined with --config or --import")
        }
        Some(mode) => Ok(CliAction::Ui(UiArgs {
            mode,
            anchor,
            monitor,
            scale,
            probe_ignore_close,
        })),
        None => match ui_only {
            Some(flag) => bail!("{flag} requires --ui"),
            None => Ok(CliAction::Run(cli)),
        },
    }
}

fn parse_mode(s: &str) -> Result<UiMode> {
    Ok(match s {
        "popover" => UiMode::Popover,
        "settings" => UiMode::Settings,
        "wizard" => UiMode::Wizard,
        "probe" => UiMode::Probe,
        _ => bail!("unknown --ui mode {s:?}"),
    })
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
    fn parses_ui_args() {
        let a = parse_args(args([
            "--ui",
            "popover",
            "--anchor",
            "10,0,22,22",
            "--monitor",
            "0,0,2880,1800",
            "--scale",
            "2",
        ]))
        .unwrap();
        let CliAction::Ui(ui) = a else {
            panic!("expected Ui, got {a:?}")
        };
        assert_eq!(
            ui,
            UiArgs {
                mode: UiMode::Popover,
                anchor: Some(AnchorRect {
                    x: 10.0,
                    y: 0.0,
                    w: 22.0,
                    h: 22.0
                }),
                monitor: Some((0.0, 0.0, 2880.0, 1800.0)),
                scale: Some(2.0),
                probe_ignore_close: false,
            }
        );
        for (flag, mode) in [
            ("settings", UiMode::Settings),
            ("wizard", UiMode::Wizard),
            ("probe", UiMode::Probe),
        ] {
            let Ok(CliAction::Ui(ui)) = parse_args(args(["--ui", flag])) else {
                panic!("--ui {flag}")
            };
            assert_eq!(ui.mode, mode);
            assert_eq!((ui.anchor, ui.monitor, ui.scale), (None, None, None));
        }
        let Ok(CliAction::Ui(ui)) = parse_args(args(["--probe-ignore-close", "--ui", "probe"]))
        else {
            panic!("probe-ignore-close")
        };
        assert!(ui.probe_ignore_close);
    }

    #[test]
    fn rejects_bad_ui_args() {
        for bad in [
            &["--ui", "bogus"][..],
            &["--ui"],
            &["--ui", "popover", "--anchor", "1,2,3"],
            &["--ui", "popover", "--monitor", "0,0,0,1800"],
            &["--ui", "popover", "--monitor", "0,0,-5,1800"],
            &["--ui", "popover", "--scale", "0"],
            &["--ui", "popover", "--scale", "NaN"],
            &["--ui", "popover", "--scale", "x"],
            &["--anchor", "1,2,3,4"],
            &["--probe-ignore-close"],
            &["--ui", "settings", "--config", "c.yaml"],
            &["--import", "c.yaml", "--ui", "settings"],
        ] {
            assert!(
                parse_args(bad.iter().map(OsString::from)).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn ui_flag_is_not_the_app() {
        assert!(matches!(
            parse_args(args(["--ui", "settings"])).unwrap(),
            CliAction::Ui(_)
        ));
        assert!(matches!(parse_args(args([])).unwrap(), CliAction::Run(_)));
    }

    #[test]
    fn rejects_unknown_flag() {
        assert!(parse_args(args(["--bogus"])).is_err());
    }
}
