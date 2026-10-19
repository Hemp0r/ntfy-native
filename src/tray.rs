//! Tray icon: left click toggles the popover, the right-click menu has a
//! status line, setup/settings, folder shortcuts and a quit entry.

use std::cell::Cell;

use anyhow::Context;
use tray_icon::menu::{Menu, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::cli::{UiArgs, UiMode};
use crate::i18n::{Strings, fill};
use crate::icons::{IconKind, rgba};
use crate::status::TrayState;
use crate::ui::layout::AnchorRect;

pub const QUIT_ID: &str = "quit";
pub const OPEN_CONFIG_ID: &str = "open-config";
pub const OPEN_LOGS_ID: &str = "open-logs";
pub const SETTINGS_ID: &str = "settings";
pub const SETUP_ID: &str = "setup";

/// Menu position of the setup entry (below the status line).
const SETUP_POSITION: usize = 1;

/// A monitor in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub scale: f64,
}

impl MonitorRect {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// Popover at the tray icon (physical pixels) on the monitor under its
/// centre, else on the first (primary) monitor.
pub fn popover_args(icon: AnchorRect, monitors: &[MonitorRect]) -> UiArgs {
    let (cx, cy) = (icon.x + icon.w / 2.0, icon.y + icon.h / 2.0);
    let monitor = monitors
        .iter()
        .find(|m| m.contains(cx, cy))
        .or(monitors.first());
    UiArgs {
        anchor: Some(icon),
        monitor: monitor.map(|m| (m.x, m.y, m.w, m.h)),
        scale: monitor.map(|m| m.scale as f32),
        ..UiArgs::window(UiMode::Popover)
    }
}

/// "Set up …" is offered until something is set up.
pub fn shows_setup(state: &TrayState) -> bool {
    *state == TrayState::NotConfigured
}

/// macOS tints template images for light and dark menu bars.
const TEMPLATE: bool = cfg!(target_os = "macos");

/// Authentication failures share the problem icon; the status text differs.
pub fn icon_kind(state: &TrayState) -> IconKind {
    match state {
        TrayState::Ok => IconKind::Ok,
        TrayState::Problem { .. } | TrayState::AuthFailed { .. } => IconKind::Problem,
        TrayState::Paused => IconKind::Paused,
        TrayState::NotConfigured => IconKind::NotConfigured,
    }
}

/// `import_failed`: the config file could not be imported at startup; shown
/// instead of the "create the file" hint, which would be misleading.
pub fn status_text(
    state: &TrayState,
    strings: &Strings,
    config_path: &str,
    import_failed: bool,
) -> String {
    match state {
        TrayState::Ok => strings.tray_ok.to_string(),
        TrayState::Problem { instance } => fill(strings.tray_problem, &[("instance", instance)]),
        TrayState::AuthFailed { instance } => {
            fill(strings.tray_auth_failed, &[("instance", instance)])
        }
        TrayState::Paused => strings.tray_paused.to_string(),
        TrayState::NotConfigured if import_failed => strings.tray_config_invalid.to_string(),
        TrayState::NotConfigured => fill(strings.tray_not_configured, &[("path", config_path)]),
    }
}

fn icon(kind: IconKind) -> anyhow::Result<Icon> {
    let (px, w, h) = rgba(kind, TEMPLATE);
    Icon::from_rgba(px, w, h).context("building tray icon")
}

pub struct Tray {
    tray_icon: TrayIcon,
    menu: Menu,
    status: MenuItem,
    setup: MenuItem,
    setup_shown: Cell<bool>,
    strings: &'static Strings,
    config_path: String,
    import_failed: bool,
}

impl Tray {
    /// On macOS this must run after the event loop has started.
    pub fn new(
        state: &TrayState,
        strings: &'static Strings,
        config_path: String,
        import_failed: bool,
    ) -> anyhow::Result<Tray> {
        let text = status_text(state, strings, &config_path, import_failed);
        let status = MenuItem::new(&text, false, None);
        let item = |id: &str, label: &str| MenuItem::with_id(MenuId::new(id), label, true, None);
        let setup = item(SETUP_ID, strings.menu_setup);
        let settings = item(SETTINGS_ID, strings.menu_settings);
        let open_config = item(OPEN_CONFIG_ID, strings.menu_open_config);
        let open_logs = item(OPEN_LOGS_ID, strings.menu_open_logs);
        let quit = item(QUIT_ID, strings.menu_quit);
        let menu = Menu::new();
        menu.append_items(&[
            &status,
            &settings,
            &PredefinedMenuItem::separator(),
            &open_config,
            &open_logs,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .context("building tray menu")?;
        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu.clone()))
            .with_menu_on_left_click(false)
            .with_icon(icon(icon_kind(state))?)
            .with_icon_as_template(TEMPLATE)
            .with_tooltip(&text)
            .build()
            .context("creating tray icon")?;
        let tray = Tray {
            tray_icon,
            menu,
            status,
            setup,
            setup_shown: Cell::new(false),
            strings,
            config_path,
            import_failed,
        };
        tray.show_setup(shows_setup(state));
        Ok(tray)
    }

    fn show_setup(&self, show: bool) {
        if show == self.setup_shown.get() {
            return;
        }
        let result = if show {
            self.menu.insert(&self.setup, SETUP_POSITION)
        } else {
            self.menu.remove(&self.setup)
        };
        match result {
            Ok(()) => self.setup_shown.set(show),
            Err(e) => tracing::warn!(error = %e, "updating the setup menu entry failed"),
        }
    }

    pub fn update(&self, state: &TrayState) {
        let text = status_text(state, self.strings, &self.config_path, self.import_failed);
        self.status.set_text(&text);
        self.show_setup(shows_setup(state));
        if let Err(e) = self.tray_icon.set_tooltip(Some(&text)) {
            tracing::warn!(error = %e, "updating tray tooltip failed");
        }
        let result = icon(icon_kind(state)).and_then(|i| {
            self.tray_icon
                .set_icon(Some(i))
                .context("setting tray icon")
        });
        if let Err(e) = result {
            tracing::warn!(error = %format!("{e:#}"), "updating tray icon failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::EN;

    fn problem() -> TrayState {
        TrayState::Problem {
            instance: "ntfy.example.org".into(),
        }
    }

    fn auth_failed() -> TrayState {
        TrayState::AuthFailed {
            instance: "ntfy.example.org".into(),
        }
    }

    #[test]
    fn icon_per_state() {
        assert_eq!(icon_kind(&TrayState::Ok), IconKind::Ok);
        assert_eq!(icon_kind(&problem()), IconKind::Problem);
        assert_eq!(icon_kind(&auth_failed()), IconKind::Problem);
        assert_eq!(icon_kind(&TrayState::Paused), IconKind::Paused);
        assert_eq!(
            icon_kind(&TrayState::NotConfigured),
            IconKind::NotConfigured
        );
    }

    #[test]
    fn status_text_per_state() {
        let path = "/cfg/config.yaml";
        assert_eq!(status_text(&TrayState::Ok, &EN, path, false), "Connected");
        assert_eq!(
            status_text(&problem(), &EN, path, false),
            "Connection problem: ntfy.example.org"
        );
        assert_eq!(
            status_text(&auth_failed(), &EN, path, false),
            "Login failed: ntfy.example.org"
        );
        assert_eq!(status_text(&TrayState::Paused, &EN, path, false), "Paused");
        assert_eq!(
            status_text(&TrayState::NotConfigured, &EN, path, false),
            "Not set up – create /cfg/config.yaml"
        );
    }

    fn icon_at(x: f64, y: f64) -> AnchorRect {
        AnchorRect {
            x,
            y,
            w: 44.0,
            h: 44.0,
        }
    }

    const LEFT: MonitorRect = MonitorRect {
        x: -1920.0,
        y: 0.0,
        w: 1920.0,
        h: 1080.0,
        scale: 1.0,
    };
    const MAIN: MonitorRect = MonitorRect {
        x: 0.0,
        y: 0.0,
        w: 2880.0,
        h: 1800.0,
        scale: 2.0,
    };

    #[test]
    fn popover_args_use_the_monitor_under_the_icon() {
        let a = popover_args(icon_at(2400.0, 0.0), &[LEFT, MAIN]);
        assert_eq!(a.mode, UiMode::Popover);
        assert_eq!(a.anchor, Some(icon_at(2400.0, 0.0)));
        assert_eq!(a.monitor, Some((0.0, 0.0, 2880.0, 1800.0)));
        assert_eq!(a.scale, Some(2.0));
        assert!(!a.probe_ignore_close);

        let a = popover_args(icon_at(-100.0, 1036.0), &[MAIN, LEFT]);
        assert_eq!(a.monitor, Some((-1920.0, 0.0, 1920.0, 1080.0)));
        assert_eq!(a.scale, Some(1.0));
    }

    #[test]
    fn popover_args_fall_back_to_the_first_monitor() {
        let a = popover_args(icon_at(9000.0, 9000.0), &[MAIN, LEFT]);
        assert_eq!(a.monitor, Some((0.0, 0.0, 2880.0, 1800.0)));
        // Without monitors the popover is centred.
        let a = popover_args(icon_at(0.0, 0.0), &[]);
        assert_eq!((a.monitor, a.scale), (None, None));
        assert!(a.anchor.is_some());
    }

    #[test]
    fn setup_entry_only_while_not_configured() {
        assert!(shows_setup(&TrayState::NotConfigured));
        for s in [TrayState::Ok, TrayState::Paused, problem(), auth_failed()] {
            assert!(!shows_setup(&s), "{s:?}");
        }
    }

    #[test]
    fn failed_import_replaces_not_configured_hint() {
        let path = "/cfg/config.yaml";
        assert_eq!(
            status_text(&TrayState::NotConfigured, &EN, path, true),
            "Configuration invalid – see logs"
        );
        // Settings from before a failed re-import still apply.
        assert_eq!(status_text(&TrayState::Ok, &EN, path, true), "Connected");
        assert_eq!(
            status_text(&problem(), &EN, path, true),
            "Connection problem: ntfy.example.org"
        );
    }
}
