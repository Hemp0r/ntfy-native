//! Tray icon with a status line and a quit entry.

use anyhow::Context;
use tray_icon::menu::{Menu, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::i18n::{Strings, fill};
use crate::icons::{IconKind, rgba};
use crate::status::TrayState;

pub const QUIT_ID: &str = "quit";

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

pub fn status_text(state: &TrayState, strings: &Strings, config_path: &str) -> String {
    match state {
        TrayState::Ok => strings.tray_ok.to_string(),
        TrayState::Problem { instance } => fill(strings.tray_problem, &[("instance", instance)]),
        TrayState::AuthFailed { instance } => {
            fill(strings.tray_auth_failed, &[("instance", instance)])
        }
        TrayState::Paused => strings.tray_paused.to_string(),
        TrayState::NotConfigured => fill(strings.tray_not_configured, &[("path", config_path)]),
    }
}

fn icon(kind: IconKind) -> anyhow::Result<Icon> {
    let (px, w, h) = rgba(kind, TEMPLATE);
    Icon::from_rgba(px, w, h).context("building tray icon")
}

pub struct Tray {
    tray_icon: TrayIcon,
    status: MenuItem,
    strings: &'static Strings,
    config_path: String,
}

impl Tray {
    /// On macOS this must run after the event loop has started.
    pub fn new(
        state: &TrayState,
        strings: &'static Strings,
        config_path: String,
    ) -> anyhow::Result<Tray> {
        let text = status_text(state, strings, &config_path);
        let status = MenuItem::new(&text, false, None);
        let quit = MenuItem::with_id(MenuId::new(QUIT_ID), strings.menu_quit, true, None);
        let menu = Menu::new();
        menu.append_items(&[&status, &PredefinedMenuItem::separator(), &quit])
            .context("building tray menu")?;
        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(icon(icon_kind(state))?)
            .with_icon_as_template(TEMPLATE)
            .with_tooltip(&text)
            .build()
            .context("creating tray icon")?;
        Ok(Tray {
            tray_icon,
            status,
            strings,
            config_path,
        })
    }

    pub fn update(&self, state: &TrayState) {
        let text = status_text(state, self.strings, &self.config_path);
        self.status.set_text(&text);
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
        assert_eq!(status_text(&TrayState::Ok, &EN, path), "Connected");
        assert_eq!(
            status_text(&problem(), &EN, path),
            "Connection problem: ntfy.example.org"
        );
        assert_eq!(
            status_text(&auth_failed(), &EN, path),
            "Login failed: ntfy.example.org"
        );
        assert_eq!(status_text(&TrayState::Paused, &EN, path), "Paused");
        assert_eq!(
            status_text(&TrayState::NotConfigured, &EN, path),
            "Not set up – create /cfg/config.yaml"
        );
    }
}
