pub struct Strings {
    pub tags_label: &'static str,
    pub bundle_title: &'static str,
    pub gap_title: &'static str,
    pub gap_body: &'static str,
    pub action_failed_title: &'static str,
    pub tray_ok: &'static str,
    pub tray_problem: &'static str,
    pub tray_auth_failed: &'static str,
    pub tray_paused: &'static str,
    pub tray_not_configured: &'static str,
    pub menu_quit: &'static str,
    pub import_while_running: &'static str,
}

pub static DE: Strings = Strings {
    tags_label: "Tags",
    bundle_title: "{topic}: {n} neue Nachrichten",
    gap_title: "Nachrichten möglicherweise verpasst",
    gap_body: "{topics} auf {instance}",
    action_failed_title: "Aktion fehlgeschlagen: {label}",
    tray_ok: "Verbunden",
    tray_problem: "Verbindungsproblem: {instance}",
    tray_auth_failed: "Anmeldung fehlgeschlagen: {instance}",
    tray_paused: "Pausiert",
    tray_not_configured: "Nicht eingerichtet – {path} anlegen",
    menu_quit: "Beenden",
    import_while_running: "ntfy-native läuft bereits – bitte beenden und --import {path} erneut ausführen",
};

pub static EN: Strings = Strings {
    tags_label: "Tags",
    bundle_title: "{topic}: {n} new messages",
    gap_title: "Messages may have been missed",
    gap_body: "{topics} on {instance}",
    action_failed_title: "Action failed: {label}",
    tray_ok: "Connected",
    tray_problem: "Connection problem: {instance}",
    tray_auth_failed: "Login failed: {instance}",
    tray_paused: "Paused",
    tray_not_configured: "Not set up – create {path}",
    menu_quit: "Quit",
    import_while_running: "ntfy-native is already running – quit it and run --import {path} again",
};

pub fn strings_for(locale: &str) -> &'static Strings {
    if locale.to_ascii_lowercase().starts_with("de") {
        &DE
    } else {
        &EN
    }
}

pub fn strings() -> &'static Strings {
    strings_for(&sys_locale::get_locale().unwrap_or_default())
}

pub fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    vars.iter()
        .fold(template.to_string(), |acc, (name, value)| {
            acc.replace(&format!("{{{name}}}"), value)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_selection() {
        assert_eq!(strings_for("de-DE").menu_quit, "Beenden");
        assert_eq!(strings_for("de").menu_quit, "Beenden");
        assert_eq!(strings_for("en-US").menu_quit, "Quit");
        assert_eq!(strings_for("fr").menu_quit, "Quit");
    }

    #[test]
    fn fill_replaces_all_and_keeps_unknown() {
        assert_eq!(
            fill("{a} {a} {b} {c}", &[("a", "x"), ("b", "y")]),
            "x x y {c}"
        );
    }
}
