//! Error states shown inline in the windows; localized only when drawn.

use crate::autostart::AutostartState;
use crate::conntest::RATE_LIMITED;
use crate::i18n::{UiStrings, fill};
use crate::ipc::TestOutcome;
use crate::ntfy::client::NetworkKind;
use crate::validate::UrlError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// Network failure category from the core: a [`NetworkKind`] label,
    /// [`RATE_LIMITED`] or `HTTP <code>`; translated when drawn.
    Unreachable(String),
    NotNtfy,
    AuthRejected,
    /// Field/position message from the core's input check (English).
    Invalid(String),
    /// The server address, checked before it is sent.
    Url(UrlError),
    NoTopics,
    TopicInvalid,
    TopicDuplicate,
    /// Reason from the core (English, secret-free).
    SaveFailed(String),
    /// Reason from the core (English).
    AutostartFailed(String),
}

impl Problem {
    /// `None` for a successful test.
    pub fn from_outcome(outcome: &TestOutcome) -> Option<Problem> {
        Some(match outcome {
            TestOutcome::Ok { .. } => return None,
            TestOutcome::InvalidInput { message } => Problem::Invalid(message.clone()),
            TestOutcome::Unreachable { message } => Problem::Unreachable(message.clone()),
            TestOutcome::NotNtfy => Problem::NotNtfy,
            TestOutcome::AuthFailed => Problem::AuthRejected,
        })
    }

    pub fn text(&self, s: &UiStrings) -> String {
        match self {
            Problem::Unreachable(reason) => {
                fill(s.err_unreachable, &[("reason", network_reason(reason, s))])
            }
            Problem::NotNtfy => s.err_not_ntfy.to_owned(),
            Problem::AuthRejected => s.err_auth.to_owned(),
            Problem::Invalid(message) => fill(s.err_invalid, &[("reason", message)]),
            Problem::Url(e) => match e {
                UrlError::Invalid => s.err_url_invalid,
                UrlError::Scheme => s.err_url_scheme,
                UrlError::Credentials => s.err_url_credentials,
                UrlError::QueryOrFragment => s.err_url_query,
            }
            .to_owned(),
            Problem::NoTopics => s.err_no_topics.to_owned(),
            Problem::TopicInvalid => s.err_topic_invalid.to_owned(),
            Problem::TopicDuplicate => s.err_topic_duplicate.to_owned(),
            Problem::SaveFailed(reason) => fill(s.err_save_failed, &[("reason", reason)]),
            Problem::AutostartFailed(reason) => fill(s.autostart_failed, &[("reason", reason)]),
        }
    }
}

/// What to tell the user about an autostart state; `None` when it is plainly on or off.
pub fn autostart_hint(state: AutostartState, s: &UiStrings) -> Option<&'static str> {
    match state {
        AutostartState::Enabled | AutostartState::Disabled => None,
        AutostartState::NeedsApproval => Some(s.autostart_needs_approval),
        // Windows can switch it on again; macOS only in System Settings.
        AutostartState::DisabledByUser if cfg!(target_os = "macos") => {
            Some(s.autostart_disabled_by_user_macos)
        }
        AutostartState::DisabledByUser => Some(s.autostart_disabled_by_user),
        AutostartState::Unavailable => Some(s.autostart_unavailable),
    }
}

/// Unknown reasons (e.g. `HTTP 502`) are shown as they are.
fn network_reason<'a>(reason: &'a str, s: &'a UiStrings) -> &'a str {
    if reason == RATE_LIMITED {
        return s.net_rate_limited;
    }
    match NetworkKind::from_label(reason) {
        Some(NetworkKind::Timeout) => s.net_timeout,
        Some(NetworkKind::Dns) => s.net_dns,
        Some(NetworkKind::Tls) => s.net_tls,
        Some(NetworkKind::Refused) => s.net_refused,
        Some(NetworkKind::Other) => s.net_failed,
        None => reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{UI_DE, UI_EN};

    #[test]
    fn outcomes_map_to_localized_texts() {
        let text = |o: TestOutcome| Problem::from_outcome(&o).map(|p| p.text(&UI_EN));
        assert_eq!(text(TestOutcome::Ok { topics: vec![] }), None);
        assert_eq!(
            text(TestOutcome::Unreachable {
                message: "timeout".into()
            })
            .unwrap(),
            "Server unreachable (timeout)"
        );
        assert_eq!(text(TestOutcome::NotNtfy).unwrap(), UI_EN.err_not_ntfy);
        assert_eq!(text(TestOutcome::AuthFailed).unwrap(), UI_EN.err_auth);
        assert_eq!(
            text(TestOutcome::InvalidInput {
                message: "topic 2: duplicate topic".into()
            })
            .unwrap(),
            "Invalid input: topic 2: duplicate topic"
        );
    }

    #[test]
    fn network_reasons_are_translated() {
        let de = |reason: &str| Problem::Unreachable(reason.into()).text(&UI_DE);
        assert_eq!(
            de(NetworkKind::Timeout.label()),
            "Server nicht erreichbar (Zeitüberschreitung)"
        );
        assert_eq!(
            de(NetworkKind::Refused.label()),
            "Server nicht erreichbar (Verbindung abgelehnt)"
        );
        assert_eq!(
            de(NetworkKind::Dns.label()),
            "Server nicht erreichbar (Name nicht auflösbar)"
        );
        assert_eq!(
            de(RATE_LIMITED),
            "Server nicht erreichbar (zu viele Anfragen)"
        );
        assert_eq!(de("HTTP 502"), "Server nicht erreichbar (HTTP 502)");
        for kind in NetworkKind::ALL {
            assert_ne!(
                de(kind.label()),
                format!("Server nicht erreichbar ({})", kind.label()),
                "{kind:?}"
            );
            assert_eq!(NetworkKind::from_label(kind.label()), Some(kind));
        }
        let en = Problem::Unreachable(NetworkKind::Tls.label().into()).text(&UI_EN);
        assert_eq!(en, "Server unreachable (TLS error)");
    }

    #[test]
    fn url_and_invalid_input_are_localized() {
        assert_eq!(
            Problem::Url(UrlError::Scheme).text(&UI_DE),
            "Die Adresse muss mit http:// oder https:// beginnen"
        );
        assert_eq!(
            Problem::Url(UrlError::Credentials).text(&UI_EN),
            UI_EN.err_url_credentials
        );
        assert_eq!(
            Problem::Invalid("topic 2: duplicate topic".into()).text(&UI_DE),
            "Ungültige Eingabe: topic 2: duplicate topic"
        );
    }

    #[test]
    fn save_failures_are_prefixed() {
        let p = Problem::SaveFailed("could not save settings".into());
        assert_eq!(p.text(&UI_EN), "Saving failed: could not save settings");
        assert_eq!(
            p.text(&UI_DE),
            "Speichern fehlgeschlagen: could not save settings"
        );
    }

    #[test]
    fn autostart_failures_and_hints_are_localized() {
        let p = Problem::AutostartFailed("access denied".into());
        assert_eq!(p.text(&UI_EN), "Could not set up autostart: access denied");
        assert_eq!(
            p.text(&UI_DE),
            "Autostart konnte nicht eingerichtet werden: access denied"
        );
        assert_eq!(autostart_hint(AutostartState::Enabled, &UI_EN), None);
        assert_eq!(autostart_hint(AutostartState::Disabled, &UI_EN), None);
        assert_eq!(
            autostart_hint(AutostartState::NeedsApproval, &UI_DE),
            Some(
                "macOS verlangt eine Bestätigung: Systemeinstellungen › Allgemein › Anmeldeobjekte"
            )
        );
        assert_eq!(
            autostart_hint(AutostartState::Unavailable, &UI_EN),
            Some("Only available in the installed app")
        );
        let by_user = autostart_hint(AutostartState::DisabledByUser, &UI_EN);
        if cfg!(target_os = "macos") {
            assert_eq!(
                by_user,
                Some("Disabled in System Settings › General › Login Items – allow it there")
            );
        } else {
            assert_eq!(
                by_user,
                Some("Disabled in Task Manager – switch on here to enable")
            );
        }
    }
}
