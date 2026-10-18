//! Application shell: wires settings, notifier, engine and tray together.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Context;
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::run_return::EventLoopExtRunReturn;
use tray_icon::TrayIconEvent;
use tray_icon::menu::MenuEvent;

use crate::actions::{SystemOpener, open_local_dir};
use crate::cli::Cli;
use crate::engine::{Command, EngineDeps, EngineHandle, spawn_engine_thread};
use crate::i18n::{Strings, fill, strings};
use crate::import::import_file;
use crate::instance_lock;
use crate::logging;
use crate::notification::{ActivationPayload, ActivationSink, Level, Notification};
use crate::notify::platform_notifier;
use crate::paths::AppPaths;
use crate::secrets::{KeyringStore, SecretStore};
use crate::settings::Settings;
use crate::state::State;
use crate::status::TrayState;
use crate::store::{Loaded, load_json};
use crate::tray::{OPEN_CONFIG_ID, OPEN_LOGS_ID, QUIT_ID, Tray};

/// Accepted in the default location when `config.yaml` is missing.
const FALLBACK_CONFIG_NAME: &str = "config.yml";

/// Lowercase names that suggest a misnamed config file (hidden `.txt`
/// extension on Windows, case variants on case-sensitive filesystems).
const NEAR_MISS_NAMES: [&str; 5] = [
    "config.yaml",
    "config.yml",
    "config.yaml.txt",
    "config.yml.txt",
    "config.txt",
];

/// Upper bound for waiting on the engine thread at shutdown.
const ENGINE_JOIN_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug)]
enum UserEvent {
    Tray(TrayState),
    Activated(ActivationPayload),
    Exit,
}

pub fn run(cli: Cli) -> anyhow::Result<()> {
    let paths = AppPaths::platform()?;
    paths
        .ensure_dirs()
        .context("creating application directories")?;
    let _log_guard = logging::init(&paths)?;
    let result = run_logged(&paths, &cli);
    if let Err(e) = &result {
        tracing::error!(error = %format!("{e:#}"), "ntfy-native failed");
    }
    result
}

fn run_logged(paths: &AppPaths, cli: &Cli) -> anyhow::Result<()> {
    let lock_file = paths.lock_file();
    let Some(_lock) = instance_lock::acquire(&lock_file)? else {
        return already_running(cli, strings(), &lock_file);
    };
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting");

    let secrets: Arc<dyn SecretStore> =
        Arc::new(KeyringStore::new().context("opening the credential store")?);
    let Startup {
        settings,
        import_error,
    } = initial_settings(paths, cli, secrets.as_ref())?;
    let state = initial_state(paths);

    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }
    let proxy = event_loop.create_proxy();

    // Registers the notification delegate (macOS) or toast activator
    // (Windows); must precede the event loop.
    let on_activate: ActivationSink = {
        let proxy = proxy.clone();
        Arc::new(move |p| {
            let _ = proxy.send_event(UserEvent::Activated(p));
        })
    };
    let notifier = platform_notifier(on_activate, &paths.data_dir);

    let strings = strings();
    if let Some(error) = &import_error {
        let n = import_failed_notification(error, strings);
        let notifier = notifier.clone();
        spawn_detached("import-error", move || {
            if let Err(e) = notifier.show(&n) {
                tracing::warn!(error = %format!("{e:#}"), "showing the import error failed");
            }
        });
    }
    let (engine, engine_thread) = spawn_engine_thread(EngineDeps {
        paths: paths.clone(),
        settings,
        state,
        secrets,
        notifier,
        opener: Arc::new(SystemOpener),
        strings,
        on_tray: Box::new({
            let proxy = proxy.clone();
            move |s| {
                let _ = proxy.send_event(UserEvent::Tray(s));
            }
        }),
        on_exit: Box::new({
            let proxy = proxy.clone();
            move || {
                let _ = proxy.send_event(UserEvent::Exit);
            }
        }),
        on_ui: Box::new(|_| {}),
        tune: |_| {},
        enable_triggers: true,
    });

    let quit_tx = engine.tx.clone();
    let (config_dir, log_dir) = (paths.config_dir.clone(), paths.log_dir.clone());
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == QUIT_ID {
            // If the engine is gone (it panicked), quit without it.
            if quit_tx.send(Command::Quit).is_err() {
                let _ = proxy.send_event(UserEvent::Exit);
            }
        } else if e.id == OPEN_CONFIG_ID {
            open_dir_in_background(config_dir.clone());
        } else if e.id == OPEN_LOGS_ID {
            open_dir_in_background(log_dir.clone());
        }
    }));
    // Unhandled tray events would pile up in tray-icon's global channel.
    TrayIconEvent::set_event_handler(Some(|_: TrayIconEvent| {}));

    let config_hint = config_path(paths, cli).display().to_string();
    let mut current = TrayState::NotConfigured;
    let mut tray: Option<Tray> = None;
    let mut engine_thread = Some(engine_thread);

    event_loop.run_return(|event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {
                match Tray::new(
                    &current,
                    strings,
                    config_hint.clone(),
                    import_error.is_some(),
                ) {
                    Ok(t) => {
                        tracing::info!(state = ?current, "tray icon created");
                        tray = Some(t);
                    }
                    Err(e) => tracing::error!(error = %format!("{e:#}"), "tray icon unavailable"),
                }
            }
            Event::UserEvent(UserEvent::Tray(s)) => {
                if s != current {
                    tracing::info!(state = ?s, "tray state");
                    if let Some(t) = &tray {
                        t.update(&s);
                    }
                    current = s;
                }
            }
            Event::UserEvent(UserEvent::Activated(p)) => {
                let _ = engine.tx.send(Command::Activated(p));
            }
            Event::UserEvent(UserEvent::Exit) => *control_flow = ControlFlow::Exit,
            // Also reached when the OS terminates the app (logout, shutdown).
            Event::LoopDestroyed => {
                tray = None;
                if let Some(thread) = engine_thread.take() {
                    stop_engine(&engine, thread);
                }
            }
            _ => {}
        }
    });
    tracing::info!("stopped");
    Ok(())
}

/// A second start exits quietly; a second start with `--import` fails (stderr,
/// log, exit code), because the running instance would ignore the import.
fn already_running(cli: &Cli, strings: &Strings, lock_file: &Path) -> anyhow::Result<()> {
    match &cli.import {
        None => {
            tracing::info!(
                lock_file = %lock_file.display(),
                "another instance is already running, exiting"
            );
            Ok(())
        }
        Some(path) => {
            let path = path.display().to_string();
            Err(anyhow::anyhow!(fill(
                strings.import_while_running,
                &[("path", &path)]
            )))
        }
    }
}

/// Runs `f` on a short-lived thread, so the main thread never blocks on it
/// and (on Windows) never joins the COM apartment that showing a toast sets
/// up for its thread.
fn spawn_detached(name: &str, f: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(f) {
        tracing::warn!(error = %e, thread = name, "starting a helper thread failed");
    }
}

fn open_dir_in_background(dir: PathBuf) {
    spawn_detached("open-folder", move || {
        if let Err(e) = open_local_dir(&dir) {
            tracing::warn!(error = %format!("{e:#}"), "opening a folder failed");
        }
    });
}

fn import_failed_notification(error: &str, strings: &Strings) -> Notification {
    Notification {
        id: "import-failed".into(),
        thread: "import-failed".into(),
        title: strings.import_failed_title.to_owned(),
        body: error.to_owned(),
        level: Level::Active,
        default_action: None,
        buttons: Vec::new(),
        image: None,
        icon: None,
    }
}

/// Asks the engine to quit (a no-op if it already has) and waits a bounded
/// time for its thread, so a stuck engine cannot keep the app alive.
fn stop_engine(engine: &EngineHandle, thread: JoinHandle<()>) {
    let _ = engine.tx.send(Command::Quit);
    let deadline = Instant::now() + ENGINE_JOIN_TIMEOUT;
    while !thread.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    if thread.is_finished() {
        if thread.join().is_err() {
            tracing::error!("engine thread panicked");
        }
    } else {
        tracing::warn!("engine did not stop in time, exiting anyway");
    }
}

/// The config file shown in the "not set up" hint and used for the first
/// import: `--config`, else `config.yaml` in the config directory, else
/// `config.yml` there if only that exists.
fn config_path(paths: &AppPaths, cli: &Cli) -> PathBuf {
    if let Some(p) = &cli.config {
        return p.clone();
    }
    let yaml = paths.default_import_file();
    let yml = paths.config_dir.join(FALLBACK_CONFIG_NAME);
    if !yaml.exists() && yml.exists() {
        yml
    } else {
        yaml
    }
}

/// Files in `dir` whose names suggest a misnamed config file.
fn config_near_misses(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| {
            let lower = name.to_ascii_lowercase();
            NEAR_MISS_NAMES.contains(&lower.as_str())
                && name != "config.yaml"
                && name != FALLBACK_CONFIG_NAME
        })
        .collect()
}

struct Startup {
    settings: Settings,
    /// File name and outermost message of a failed import that left the
    /// settings unchanged.
    import_error: Option<String>,
}

/// Loads `settings.json` and runs the one-time (or explicit) import.
fn initial_settings(
    paths: &AppPaths,
    cli: &Cli,
    secrets: &dyn SecretStore,
) -> anyhow::Result<Startup> {
    let settings_path = paths.settings_file();
    let existing = match load_json::<Settings>(&settings_path)? {
        Loaded::Ok(s) => Some(s),
        Loaded::Missing => None,
        Loaded::Corrupt { backup } => {
            tracing::warn!(backup = %backup.display(), "settings were corrupt and have been backed up");
            None
        }
    };
    // Settings without instances (saved by an unconfigured run) hold nothing
    // the one-time import could overwrite.
    let configured = existing.as_ref().is_some_and(|s| !s.instances.is_empty());
    let import = match &cli.import {
        Some(p) => Some(p.clone()),
        None => {
            let p = config_path(paths, cli);
            if !p.exists() {
                tracing::info!(path = %p.display(), configured, "config file not found");
                if !configured && cli.config.is_none() {
                    for name in config_near_misses(&paths.config_dir) {
                        tracing::warn!(
                            found = %paths.config_dir.join(&name).display(),
                            expected = %p.display(),
                            "found a config file with the wrong name, rename it"
                        );
                    }
                }
                None
            } else if configured {
                tracing::info!(
                    path = %p.display(),
                    "settings exist, ignoring config file; use --import to re-import it"
                );
                None
            } else {
                tracing::info!(path = %p.display(), "config file found, importing");
                Some(p)
            }
        }
    };
    let Some(import) = import else {
        return Ok(Startup {
            settings: existing.unwrap_or_default(),
            import_error: None,
        });
    };
    match import_file(&import, existing.clone(), secrets, &settings_path) {
        Ok(s) => {
            tracing::info!(path = %import.display(), instances = s.instances.len(), "config imported");
            Ok(Startup {
                settings: s,
                import_error: None,
            })
        }
        Err(e) => Ok(after_failed_import(&import, existing, &settings_path, &e)),
    }
}

/// `import_file` may fail after it saved the new settings (while sanitising
/// the file); then those settings apply and the file may still hold secrets.
fn after_failed_import(
    import: &Path,
    existing: Option<Settings>,
    settings_path: &Path,
    err: &anyhow::Error,
) -> Startup {
    let saved = match load_json::<Settings>(settings_path) {
        Ok(Loaded::Ok(s)) if Some(&s) != existing.as_ref() => Some(s),
        _ => None,
    };
    match saved {
        Some(s) => {
            tracing::error!(
                path = %import.display(),
                error = %format!("{err:#}"),
                "config imported, but the file could not be cleaned up; \
                 it may still contain plaintext secrets, delete it"
            );
            Startup {
                settings: s,
                import_error: None,
            }
        }
        None => {
            tracing::error!(
                path = %import.display(),
                error = %format!("{err:#}"),
                "config import failed, the file was left unchanged"
            );
            Startup {
                settings: existing.unwrap_or_default(),
                import_error: Some(import_error_text(import, err)),
            }
        }
    }
}

/// The outermost error message, prefixed with the file name unless it
/// already contains the file's path.
fn import_error_text(import: &Path, err: &anyhow::Error) -> String {
    let msg = err.to_string();
    if msg.contains(&import.display().to_string()) {
        return msg;
    }
    match import.file_name() {
        Some(name) => format!("{}: {msg}", name.to_string_lossy()),
        None => format!("{}: {msg}", import.display()),
    }
}

fn initial_state(paths: &AppPaths) -> State {
    match load_json::<State>(&paths.state_file()) {
        Ok(Loaded::Ok(s)) => s,
        Ok(Loaded::Missing) => State::default(),
        Ok(Loaded::Corrupt { backup }) => {
            tracing::warn!(backup = %backup.display(), "state was corrupt and has been backed up, starting fresh");
            State::default()
        }
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "cannot read state, starting fresh");
            State::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{MemoryStore, account_for};
    use crate::store::save_json;
    use std::fs;

    const YAML: &str =
        "instances:\n  - url: http://127.0.0.1:18080\n    token: tk_secret\n    topics: [alpha]\n";

    fn setup() -> (tempfile::TempDir, AppPaths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(dir.path());
        paths.ensure_dirs().unwrap();
        (dir, paths)
    }

    fn topics(s: &Settings) -> Vec<String> {
        s.instances
            .iter()
            .flat_map(|i| i.topics.iter().map(|t| t.name.clone()))
            .collect()
    }

    #[test]
    fn second_start_exits_quietly_unless_importing() {
        use crate::i18n::{DE, EN};
        let lock = Path::new("/x/instance.lock");
        assert!(already_running(&Cli::default(), &EN, lock).is_ok());
        let cli = Cli {
            import: Some(PathBuf::from("/x/new.yaml")),
            ..Cli::default()
        };
        let err = already_running(&cli, &EN, lock).unwrap_err().to_string();
        assert!(
            err.contains("already running") && err.contains("--import /x/new.yaml"),
            "{err}"
        );
        let err = already_running(&cli, &DE, lock).unwrap_err().to_string();
        assert!(err.contains("läuft bereits"), "{err}");
    }

    #[test]
    fn first_start_imports_default_config() {
        let (_dir, paths) = setup();
        fs::write(paths.default_import_file(), YAML).unwrap();
        let store = MemoryStore::new();
        let s = initial_settings(&paths, &Cli::default(), &store)
            .unwrap()
            .settings;
        assert_eq!(topics(&s), ["alpha"]);
        assert!(!paths.default_import_file().exists());
        assert!(paths.config_dir.join("config.imported.yaml").exists());
        assert_eq!(
            store.get(&account_for("127.0.0.1")).unwrap().as_deref(),
            Some("tk_secret")
        );
    }

    #[test]
    fn missing_config_is_not_configured() {
        let (_dir, paths) = setup();
        let s = initial_settings(&paths, &Cli::default(), &MemoryStore::new())
            .unwrap()
            .settings;
        assert!(s.instances.is_empty());
        assert!(!paths.settings_file().exists());
    }

    #[test]
    fn existing_settings_win_over_config_file() {
        let (_dir, paths) = setup();
        let store = MemoryStore::new();
        fs::write(paths.default_import_file(), YAML).unwrap();
        initial_settings(&paths, &Cli::default(), &store).unwrap();
        fs::write(paths.default_import_file(), YAML.replace("alpha", "beta")).unwrap();
        let s = initial_settings(&paths, &Cli::default(), &store)
            .unwrap()
            .settings;
        assert_eq!(topics(&s), ["alpha"]);
        assert!(paths.default_import_file().exists());
    }

    #[test]
    fn empty_settings_still_import_config() {
        // The engine saves settings on exit even when nothing is set up.
        let (_dir, paths) = setup();
        save_json(&paths.settings_file(), &Settings::default()).unwrap();
        fs::write(paths.default_import_file(), YAML).unwrap();
        let s = initial_settings(&paths, &Cli::default(), &MemoryStore::new())
            .unwrap()
            .settings;
        assert_eq!(topics(&s), ["alpha"]);
        assert!(!paths.default_import_file().exists());
    }

    #[test]
    fn explicit_import_reimports() {
        let (dir, paths) = setup();
        let store = MemoryStore::new();
        fs::write(paths.default_import_file(), YAML).unwrap();
        initial_settings(&paths, &Cli::default(), &store).unwrap();
        let other = dir.path().join("other.yaml");
        fs::write(&other, YAML.replace("alpha", "beta")).unwrap();
        let cli = Cli {
            import: Some(other.clone()),
            ..Cli::default()
        };
        let s = initial_settings(&paths, &cli, &store).unwrap().settings;
        assert_eq!(topics(&s), ["beta"]);
        assert!(!other.exists());
    }

    #[test]
    fn config_flag_used_when_unconfigured() {
        let (dir, paths) = setup();
        let custom = dir.path().join("custom.yaml");
        fs::write(&custom, YAML).unwrap();
        let cli = Cli {
            config: Some(custom.clone()),
            ..Cli::default()
        };
        let s = initial_settings(&paths, &cli, &MemoryStore::new())
            .unwrap()
            .settings;
        assert_eq!(topics(&s), ["alpha"]);
        assert_eq!(config_path(&paths, &cli), custom);
    }

    #[test]
    fn invalid_config_leaves_file_and_runs_unconfigured() {
        let (_dir, paths) = setup();
        fs::write(paths.default_import_file(), "instances: [nonsense").unwrap();
        let s = initial_settings(&paths, &Cli::default(), &MemoryStore::new())
            .unwrap()
            .settings;
        assert!(s.instances.is_empty());
        assert!(paths.default_import_file().exists());
    }

    #[test]
    fn failed_reimport_keeps_existing_settings() {
        let (dir, paths) = setup();
        let existing = Settings {
            urgent_breaks_dnd: true,
            ..Settings::default()
        };
        save_json(&paths.settings_file(), &existing).unwrap();
        let bad = dir.path().join("bad.yaml");
        fs::write(&bad, "instances: [nonsense").unwrap();
        let cli = Cli {
            import: Some(bad),
            ..Cli::default()
        };
        let s = initial_settings(&paths, &cli, &MemoryStore::new())
            .unwrap()
            .settings;
        assert_eq!(s, existing);
    }

    #[test]
    fn config_yml_fallback_imported() {
        let (_dir, paths) = setup();
        let yml = paths.config_dir.join("config.yml");
        fs::write(&yml, YAML).unwrap();
        assert_eq!(config_path(&paths, &Cli::default()), yml);
        let s = initial_settings(&paths, &Cli::default(), &MemoryStore::new()).unwrap();
        assert_eq!(topics(&s.settings), ["alpha"]);
        assert!(!yml.exists());
        assert!(paths.config_dir.join("config.imported.yaml").exists());
    }

    #[test]
    fn config_yaml_wins_over_config_yml() {
        let (_dir, paths) = setup();
        let yml = paths.config_dir.join("config.yml");
        fs::write(paths.default_import_file(), YAML).unwrap();
        fs::write(&yml, YAML.replace("alpha", "beta")).unwrap();
        let s = initial_settings(&paths, &Cli::default(), &MemoryStore::new()).unwrap();
        assert_eq!(topics(&s.settings), ["alpha"]);
        assert!(yml.exists());
    }

    #[test]
    fn config_flag_has_no_yml_fallback() {
        let (dir, paths) = setup();
        fs::write(dir.path().join("custom.yml"), YAML).unwrap();
        let cli = Cli {
            config: Some(dir.path().join("custom.yaml")),
            ..Cli::default()
        };
        let s = initial_settings(&paths, &cli, &MemoryStore::new()).unwrap();
        assert!(s.settings.instances.is_empty());
        assert_eq!(config_path(&paths, &cli), dir.path().join("custom.yaml"));
    }

    #[test]
    fn hint_names_config_yaml_when_nothing_exists() {
        let (_dir, paths) = setup();
        assert_eq!(
            config_path(&paths, &Cli::default()),
            paths.default_import_file()
        );
    }

    #[test]
    fn near_misses_are_found() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "config.yaml.txt",
            "Config.yml",
            "CONFIG.TXT",
            "other.yaml",
            "config.json",
        ] {
            fs::write(dir.path().join(name), "").unwrap();
        }
        fs::create_dir(dir.path().join("config.yml.txt")).unwrap();
        let mut found = config_near_misses(dir.path());
        found.sort();
        assert_eq!(found, ["CONFIG.TXT", "Config.yml", "config.yaml.txt"]);
        assert!(config_near_misses(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn exact_names_are_no_near_misses() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.yaml"), "").unwrap();
        fs::write(dir.path().join("config.yml"), "").unwrap();
        fs::write(dir.path().join("config.imported.yaml"), "").unwrap();
        assert!(config_near_misses(dir.path()).is_empty());
    }

    #[test]
    fn failed_import_is_reported() {
        let (_dir, paths) = setup();
        fs::write(
            paths.default_import_file(),
            "instances:\n  - token: tk_s3cret\n    topics: [a]\n",
        )
        .unwrap();
        let s = initial_settings(&paths, &Cli::default(), &MemoryStore::new()).unwrap();
        assert!(s.settings.instances.is_empty());
        let err = s.import_error.expect("import error");
        assert!(
            err.starts_with("config.yaml: instance 1: missing field `url`"),
            "{err}"
        );
        assert!(!err.contains("s3cret"), "{err}");
    }

    #[test]
    fn import_error_names_the_file_once() {
        let (_dir, paths) = setup();
        let yml = paths.config_dir.join("config.yml");
        fs::write(&yml, "instances: [nonsense").unwrap();
        let s = initial_settings(&paths, &Cli::default(), &MemoryStore::new()).unwrap();
        let err = s.import_error.expect("import error");
        assert!(err.starts_with("config.yml: "), "{err}");

        // Messages that already contain the path are not prefixed again.
        fs::write(&yml, [0xFF, 0xFE, b'x', 0]).unwrap();
        let s = initial_settings(&paths, &Cli::default(), &MemoryStore::new()).unwrap();
        let err = s.import_error.expect("import error");
        assert!(err.starts_with(&yml.display().to_string()), "{err}");
    }

    #[test]
    fn successful_or_skipped_import_reports_nothing() {
        let (_dir, paths) = setup();
        let store = MemoryStore::new();
        assert_eq!(
            initial_settings(&paths, &Cli::default(), &store)
                .unwrap()
                .import_error,
            None
        );
        fs::write(paths.default_import_file(), YAML).unwrap();
        let s = initial_settings(&paths, &Cli::default(), &store).unwrap();
        assert_eq!(s.import_error, None);
        // A broken file is ignored (not imported) once settings exist.
        fs::write(paths.default_import_file(), "instances: [nonsense").unwrap();
        let s = initial_settings(&paths, &Cli::default(), &store).unwrap();
        assert_eq!(s.import_error, None);
        assert_eq!(topics(&s.settings), ["alpha"]);
    }

    #[test]
    fn import_failed_notification_is_active() {
        use crate::i18n::EN;
        use crate::notification::Level;
        let n = import_failed_notification("instance 1: missing field `url`", &EN);
        assert_eq!(n.title, "Could not read the configuration");
        assert_eq!(n.body, "instance 1: missing field `url`");
        assert_eq!(n.level, Level::Active);
        assert!(n.default_action.is_none() && n.buttons.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn failure_after_save_uses_saved_settings() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, paths) = setup();
        fs::write(paths.default_import_file(), YAML).unwrap();
        // The sanitized copy cannot be written into a read-only directory.
        fs::set_permissions(&paths.config_dir, fs::Permissions::from_mode(0o555)).unwrap();
        let s = initial_settings(&paths, &Cli::default(), &MemoryStore::new());
        fs::set_permissions(&paths.config_dir, fs::Permissions::from_mode(0o755)).unwrap();
        let s = s.unwrap();
        assert_eq!(topics(&s.settings), ["alpha"]);
        assert_eq!(s.import_error, None);
        assert!(paths.default_import_file().exists());
    }
}
