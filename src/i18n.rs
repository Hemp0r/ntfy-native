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
    pub menu_open_config: &'static str,
    pub menu_open_logs: &'static str,
    pub tray_config_invalid: &'static str,
    pub import_failed_title: &'static str,
    pub import_while_running: &'static str,
    pub test_title: &'static str,
    pub test_body: &'static str,
    pub menu_settings: &'static str,
    pub menu_setup: &'static str,
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
    menu_open_config: "Konfigurationsordner öffnen",
    menu_open_logs: "Logs öffnen",
    tray_config_invalid: "Konfiguration fehlerhaft – siehe Logs",
    import_failed_title: "Konfiguration konnte nicht gelesen werden",
    import_while_running: "ntfy-native läuft bereits – bitte beenden und --import {path} erneut ausführen",
    test_title: "ntfy-native",
    test_body: "Benachrichtigungen funktionieren.",
    menu_settings: "Einstellungen …",
    menu_setup: "Einrichten …",
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
    menu_open_config: "Open config folder",
    menu_open_logs: "Open logs",
    tray_config_invalid: "Configuration invalid – see logs",
    import_failed_title: "Could not read the configuration",
    import_while_running: "ntfy-native is already running – quit it and run --import {path} again",
    test_title: "ntfy-native",
    test_body: "Notifications work.",
    menu_settings: "Settings …",
    menu_setup: "Set up …",
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

macro_rules! ui_strings {
    ($($f:ident: $de:expr, $en:expr;)*) => {
        pub struct UiStrings {
            $(pub $f: &'static str,)*
        }

        pub static UI_DE: UiStrings = UiStrings { $($f: $de,)* };
        pub static UI_EN: UiStrings = UiStrings { $($f: $en,)* };

        impl UiStrings {
            pub fn fields(&self) -> Vec<(&'static str, &'static str)> {
                vec![$((stringify!($f), self.$f),)*]
            }
        }
    };
}

ui_strings! {
    popover_title: "ntfy", "ntfy";
    status_connected: "Verbunden", "Connected";
    status_connecting: "Verbinde …", "Connecting …";
    status_problem: "Verbindungsproblem", "Connection problem";
    status_auth_failed: "Anmeldung fehlgeschlagen", "Login failed";
    recent_heading: "Zuletzt", "Recent";
    recent_empty: "Noch keine Nachrichten", "No messages yet";
    gap_hint: "Nachrichten möglicherweise verpasst: {topic}", "Messages may have been missed: {topic}";
    dismiss: "Ausblenden", "Dismiss";
    pause: "Pause", "Pause";
    pause_15m: "15 Minuten", "15 minutes";
    pause_1h: "1 Stunde", "1 hour";
    pause_tomorrow: "Bis morgen früh", "Until tomorrow morning";
    pause_forever: "Bis ich fortsetze", "Until I resume";
    resume: "Fortsetzen", "Resume";
    paused_until: "Pausiert bis {time}", "Paused until {time}";
    paused_until_tomorrow: "Pausiert bis morgen {time}", "Paused until tomorrow {time}";
    paused: "Pausiert", "Paused";
    add_topic: "+ Topic", "+ Topic";
    settings: "Einstellungen", "Settings";
    quit: "Beenden", "Quit";
    just_now: "gerade eben", "just now";
    minutes_ago: "vor {n} Min.", "{n} min ago";
    hours_ago: "vor {n} Std.", "{n} h ago";
    wizard_title: "ntfy-native einrichten", "Set up ntfy-native";
    server_url: "Server-Adresse", "Server address";
    server_hint: "z. B. https://ntfy.sh oder dein eigener Server", "e.g. https://ntfy.sh or your own server";
    auth_heading: "Anmeldung", "Login";
    auth_none: "Ohne Anmeldung", "No login";
    auth_token: "Access-Token", "Access token";
    auth_basic: "Benutzername und Passwort", "Username and password";
    username: "Benutzername", "Username";
    password: "Passwort", "Password";
    keep_secret_hint: "Leer lassen, um das gespeicherte zu behalten", "Leave empty to keep the stored one";
    topics_heading: "Topics", "Topics";
    topic_name: "Topic-Name", "Topic name";
    topic_server: "Server", "Server";
    topic_not_readable: "Kein Leserecht", "No read access";
    notifications_heading: "Benachrichtigungen", "Notifications";
    send_test: "Test-Benachrichtigung senden", "Send test notification";
    notifications_hint_macos: "Falls nichts erscheint: Systemeinstellungen › Mitteilungen › ntfy-native erlauben.", "If nothing appears: allow ntfy-native in System Settings › Notifications.";
    notifications_hint_windows: "Falls nichts erscheint: Einstellungen › System › Benachrichtigungen › ntfy-native einschalten.", "If nothing appears: turn on ntfy-native in Settings › System › Notifications.";
    urgent_global: "Dringende Nachrichten (Priorität 5) dürfen „Nicht stören“ durchbrechen", "Urgent messages (priority 5) may break through Do Not Disturb";
    urgent_default: "Standard", "Default";
    urgent_on: "An", "On";
    urgent_off: "Aus", "Off";
    muted: "Stumm", "Muted";
    back: "Zurück", "Back";
    next: "Weiter", "Next";
    finish: "Fertig", "Finish";
    cancel: "Abbrechen", "Cancel";
    save: "Speichern", "Save";
    testing: "Prüfe Verbindung …", "Checking connection …";
    err_unreachable: "Server nicht erreichbar ({reason})", "Server unreachable ({reason})";
    err_not_ntfy: "Unter dieser Adresse antwortet kein ntfy-Server", "No ntfy server answers at this address";
    err_auth: "Anmeldung abgelehnt – Token bzw. Passwort prüfen", "Login rejected – check token or password";
    err_no_topics: "Mindestens ein Topic angeben", "Add at least one topic";
    err_topic_invalid: "Ungültiger Topic-Name (erlaubt: Buchstaben, Ziffern, - und _)", "Invalid topic name (allowed: letters, digits, - and _)";
    err_topic_duplicate: "Dieses Topic gibt es schon", "This topic already exists";
    servers_heading: "Server", "Servers";
    add_server: "Server hinzufügen", "Add server";
    edit: "Bearbeiten", "Edit";
    remove: "Entfernen", "Remove";
    remove_confirm: "{host} wirklich entfernen?", "Really remove {host}?";
    cache_duration: "Server-Cache-Dauer", "Server cache duration";
    cache_hint: "Muss zur Einstellung cache-duration des Servers passen", "Must match the server's cache-duration setting";
    open_config: "Konfigurationsordner öffnen", "Open config folder";
    open_logs: "Logs öffnen", "Open logs";
    version: "Version {v}", "Version {v}";
    not_configured: "Noch nicht eingerichtet", "Not set up yet";
    setup: "Einrichten …", "Set up …";
    err_save_failed: "Speichern fehlgeschlagen: {reason}", "Saving failed: {reason}";
    urgent_short: "Dringend", "Urgent";
    cache_hours: "{n} Std.", "{n} h";
    cache_days: "{n} Tage", "{n} days";
    cache_custom: "Benutzerdefiniert ({v})", "Custom ({v})";
    general_heading: "Allgemein", "General";
    wizard_done: "Fertig eingerichtet", "All set";
}

pub fn ui_strings_for(locale: &str) -> &'static UiStrings {
    if locale.to_ascii_lowercase().starts_with("de") {
        &UI_DE
    } else {
        &UI_EN
    }
}

pub fn ui_strings() -> &'static UiStrings {
    ui_strings_for(&sys_locale::get_locale().unwrap_or_default())
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
    fn folder_and_import_error_strings() {
        assert_eq!(DE.menu_open_config, "Konfigurationsordner öffnen");
        assert_eq!(EN.menu_open_config, "Open config folder");
        assert_eq!(DE.menu_open_logs, "Logs öffnen");
        assert_eq!(EN.menu_open_logs, "Open logs");
        assert_eq!(
            DE.tray_config_invalid,
            "Konfiguration fehlerhaft – siehe Logs"
        );
        assert_eq!(EN.tray_config_invalid, "Configuration invalid – see logs");
        assert_eq!(
            DE.import_failed_title,
            "Konfiguration konnte nicht gelesen werden"
        );
        assert_eq!(EN.import_failed_title, "Could not read the configuration");
    }

    fn placeholders(s: &str) -> Vec<String> {
        let mut v: Vec<String> = s
            .split('{')
            .skip(1)
            .filter_map(|p| p.split_once('}').map(|(n, _)| n.to_string()))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn ui_strings_complete_in_both_languages() {
        let de = UI_DE.fields();
        let en = UI_EN.fields();
        assert_eq!(de.len(), en.len());
        assert!(de.len() > 60);
        for ((name, d), (_, e)) in de.iter().zip(en.iter()) {
            assert!(!d.is_empty() && !e.is_empty(), "{name} empty");
            assert_eq!(placeholders(d), placeholders(e), "{name} placeholders");
        }
        assert_eq!(ui_strings_for("de-AT").quit, "Beenden");
        assert_eq!(ui_strings_for("en").quit, "Quit");
        assert_eq!(DE.menu_settings, "Einstellungen …");
        assert_eq!(EN.menu_setup, "Set up …");
    }

    #[test]
    fn fill_replaces_all_and_keeps_unknown() {
        assert_eq!(
            fill("{a} {a} {b} {c}", &[("a", "x"), ("b", "y")]),
            "x x y {c}"
        );
    }
}
