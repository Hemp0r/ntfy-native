# ntfy-native Oberfläche (Popover, Assistent, Einstellungen) – Implementation Plan (Teil 2 von 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ein Klick aufs Tray-Icon öffnet ein Popover (Status, Topics, letzte Nachrichten, Pausieren); beim ersten Start ohne Konfiguration führt ein Assistent durch die Einrichtung; ein Einstellungsfenster verwaltet Server und Topics – alles live übernommen, ohne Neustart.

**Architecture:** Die UI läuft in einem **eigenen Prozess** derselben Binary (`ntfy-native --ui <modus>`), den der Kern bei Bedarf startet und der sich beim Schließen vollständig beendet. Kern und UI sprechen JSON-Zeilen über stdin/stdout des Kindprozesses (Protokoll in `src/ipc.rs`). Der Kern (Engine) bleibt alleiniger Eigentümer von Settings, Secrets und Verbindungen: die UI schickt Anfragen, die Engine wendet sie an, persistiert, startet betroffene Verbindungen neu und schickt einen neuen Snapshot.

**Tech Stack:** Rust, eframe/egui 0.37 (glow-Renderer; MIT/Apache), chrono (lokale Zeit für „bis morgen“ und Datumsanzeige), egui_kittest (Headless-UI-Tests), vorhandener Stack aus Teil 1 (tao, tray-icon, tokio, reqwest, serde).

**Spec:** `docs/superpowers/specs/2026-10-18-ntfy-native-design.md` (insb. 4.1, 4.2, 5.1, 5.3–5.5, 5.11). Entscheidungen aus Teil 1: `docs/superpowers/plans/2026-10-18-ntfy-native-core-decisions.md`.

**Basis:** Branch `feat/ui`, abgezweigt von `feat/open-folders` (PR #2). Nach dessen Merge auf `main` rebasen.

## Global Constraints

- Alles aus Teil 1 gilt weiter (Name/IDs `dev.hemp0r.ntfy-native`, macOS 15+, Windows 10/11, Secrets nur im Schlüsselbund/Credential Manager, nie in Logs, `config.yaml` einmaliger Import).
- **Speicher:** Ruhezustand (UI geschlossen) < 50 MB Summe aller Prozesse, Kern weiterhin < 30 MB. Der UI-Prozess darf, solange er offen ist, mehr brauchen, muss sich beim Schließen aber **vollständig beenden** (Spec 5.1). Kein UI-Code und keine UI-Abhängigkeit im Kernpfad laden, solange nicht `--ui` gestartet wurde.
- **Lizenzen:** Nur Abhängigkeiten unter OSI-Lizenz **ohne kommerzielle Doppellizenz** (SignPath-Bedingung) – also kein Slint, keine WebView.
- **Secrets fließen nur UI → Kern** (bei Eingabe). Der Kern schickt der UI nie Secrets, nur „hat Token / Benutzername“. Fehlermeldungen an die UI enthalten keine eingegebenen Werte (wie `import.rs` seit PR #2).
- Alle sichtbaren Texte über `src/i18n.rs` (DE bei Locale `de*`, sonst EN).
- Der Kern blockiert nie auf den UI-Prozess (Schreiben über Kanal + Writer-Thread, Lesen in eigenem Thread).
- Windows-Ziel muss bauen: `PATH="$HOME/.rustup/toolchains/stable-x86_64-apple-darwin/bin:$PATH" cargo clippy --all-targets --target x86_64-pc-windows-msvc -- -D warnings`.

## Review Focus

1. **UI-Prozess stürzt ab, wird beendet oder hängt** → Kern läuft unbeeinflusst weiter, nächster Tray-Klick startet einen frischen UI-Prozess, keine Zombie-Prozesse, Engine nie blockiert. Tests: Task 10 (`child_exit_is_detected`, `hanging_child_is_killed_on_close`).
2. **Secrets aus Assistent/Einstellungen** tauchen nie im Snapshot, in Logs oder Fehlermeldungen auf. Tests: Task 2 (`snapshot_never_contains_secrets`), Task 4 (`apply_errors_never_echo_input`).
3. **Server bearbeiten, während Nachrichten eintreffen** → keine doppelten oder verlorenen Benachrichtigungen; unveränderte URL behält den Nachhol-Zustand. Test: Task 4 (`edit_topics_keeps_state_no_duplicates`).
4. **Falsche Eingaben im Assistenten** (ungültige URL, Server nicht erreichbar, kein ntfy-Server, falsches Token, Topic ohne Leserecht, doppelter/ungültiger Topic, keine Topics) → klare Meldung, nichts gespeichert. Tests: Task 5, Task 8.
5. **Tray oben (macOS) / unten (Windows), mehrere Monitore, HiDPI** → Popover liegt sichtbar am Icon. Test: Task 6 (`popover_position_*`).

---

### Task 1: Gemeinsame Validierung und Instanz-Änderung (ohne UI)

**Files:**
- Create: `src/validate.rs`, `src/instance_edit.rs`
- Modify: `src/import.rs` (nutzt `validate`), `src/lib.rs`

**Interfaces:**
- Consumes: `Settings`, `Instance`, `Topic`, `AuthKind`, `SecretStore`, `account_for`
- Produces:
  ```rust
  // validate.rs (aus import.rs verschoben, Verhalten unverändert)
  pub fn valid_topic_name(name: &str) -> bool;              // ^[-_A-Za-z0-9]{1,64}$
  pub fn normalize_url(raw: &str) -> anyhow::Result<String>; // Fehlertexte ohne Eingabewert
  pub fn same_url(stored: &str, normalized: &str) -> bool;
  pub fn unique_id(settings: &Settings, base: &str) -> String;
  // instance_edit.rs
  pub struct InstanceDraft { pub id: Option<String>, pub url: String, pub auth: AuthDraft, pub cache_duration_secs: i64, pub topics: Vec<Topic> }
  pub enum AuthDraft { None, Token { token: Option<String> }, Basic { username: String, password: Option<String> } } // None-Secret = gespeichertes behalten
  pub fn apply_draft(settings: &Settings, draft: &InstanceDraft, secrets: &dyn SecretStore) -> anyhow::Result<(Settings, String /*instance id*/)>;
  pub fn remove_instance(settings: &Settings, id: &str, secrets: &dyn SecretStore) -> anyhow::Result<Settings>;
  ```
  `InstanceDraft`/`AuthDraft` derive `Clone, PartialEq, Serialize, Deserialize`; **`Debug` manuell mit geschwärzten Secrets** (wie `Auth`).

Regeln `apply_draft`: URL normalisieren; Topics ≥ 1, gültig, ohne Duplikate; `cache_duration_secs` 3600..=2_592_000 (1 h–30 Tage). `id: None` → neue Instanz mit `unique_id(host)`; doppelte (normalisierte) URL einer anderen Instanz → Fehler. `id: Some` → Instanz muss existieren. Auth wie `auth_plan` in `import.rs`: `Token{Some}` setzt, `Token{None}` behält nur bei gespeichertem Token (sonst Fehler „token required“), `Basic{password: Some}` setzt, `Basic{None}` behält nur bei gleichem Benutzernamen, `None` löscht das Secret. Erst validieren, dann Secrets schreiben. Fehlertexte nennen Feld/Position, nie Werte. `remove_instance` löscht das Secret (Fehlen ist ok).

- [ ] **Step 1: Failing tests** (`src/instance_edit.rs`, MemoryStore)
```rust
#[test] fn adds_new_instance_with_unique_id() { /* zwei Drafts gleicher Host, andere Pfade → ids "h" und "h-2" */ }
#[test] fn rejects_duplicate_url_of_other_instance() {}
#[test] fn rejects_empty_or_invalid_topics_and_bad_duration() { /* [] ; ["bad topic"]; ["a","a"]; 60 s → jeweils Err */ }
#[test] fn token_keep_requires_stored_token() { /* Token{None} ohne gespeichertes → Err; mit gespeichertem → Ok, Secret unverändert */ }
#[test] fn basic_keep_only_for_same_username() {}
#[test] fn auth_none_deletes_secret() {}
#[test] fn apply_errors_never_echo_input() { /* url "https://u:s3cret@x", topics ["s3cret x"], token "s3cret" in sonst ungültigem Draft → format!("{e:#}") enthält "s3cret" nicht */ }
#[test] fn draft_debug_redacts_secrets() {}
#[test] fn remove_deletes_secret_and_instance() {}
```
- [ ] **Step 2: Run** `cargo test instance_edit` → FAIL
- [ ] **Step 3: `validate.rs` herauslösen** (import.rs nutzt es; alle bestehenden import-Tests bleiben grün), **`instance_edit.rs` implementieren**
- [ ] **Step 4: Run** `cargo test` → PASS
- [ ] **Step 5: Commit** `feat: shared validation and live instance editing`

---

### Task 2: IPC-Protokoll und UI-Snapshot

**Files:**
- Create: `src/ipc.rs`, `src/snapshot.rs`
- Modify: `src/status.rs` (Status pro Instanz), `src/lib.rs`

**Interfaces:**
- Consumes: `InstanceDraft`, `Settings`, `ConnStatus`, `RecentMessage`, `GapFlag`, `PROBLEM_GRACE_SECS`
- Produces:
  ```rust
  // ipc.rs
  pub const PROTOCOL_VERSION: u32 = 1;
  pub const MAX_LINE: usize = 1 << 20;
  #[serde(tag = "t", rename_all = "snake_case")]
  pub enum CoreMsg { Hello { version: u32 }, Snapshot(UiSnapshot), TestResult { request_id: u64, outcome: TestOutcome }, SaveResult { request_id: u64, result: Result<String, String> }, Close }
  #[serde(tag = "t", rename_all = "snake_case")]
  pub enum UiMsg { Hello { version: u32 }, Request(UiRequest) }
  #[serde(tag = "r", rename_all = "snake_case")]
  pub enum UiRequest {
      PauseAll { until: Option<i64> }, SetTopicMuted { instance_id: String, topic: String, muted: bool },
      SetTopicUrgent { instance_id: String, topic: String, urgent: Option<bool> }, SetUrgentGlobal { enabled: bool },
      DismissGap { instance_id: String, topic: String }, OpenRecent { instance_id: String, message_id: String },
      SendTestNotification, TestConnection { request_id: u64, draft: InstanceDraft },
      SaveInstance { request_id: u64, draft: InstanceDraft }, RemoveInstance { instance_id: String },
      OpenConfigDir, OpenLogs, OpenSettings, OpenWizard, Quit,
  }
  pub enum TestOutcome { Ok { topics: Vec<TopicAccess> }, InvalidInput { message: String }, Unreachable { message: String }, NotNtfy, AuthFailed }
  pub struct TopicAccess { pub name: String, pub readable: bool }
  pub fn write_msg<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()>;      // eine Zeile + flush
  pub fn read_msg<T: DeserializeOwned>(r: &mut impl BufRead) -> io::Result<Option<T>>; // None bei EOF; > MAX_LINE oder kaputt → Err(InvalidData)
  // snapshot.rs
  pub struct UiSnapshot { pub app_version: String, pub configured: bool, pub instances: Vec<InstanceView>, pub recent: Vec<RecentView>, pub gaps: Vec<GapView>, pub paused_until: Option<i64>, pub urgent_breaks_dnd: bool }
  pub struct InstanceView { pub id: String, pub host: String, pub url: String, pub status: InstanceStatus, pub auth: AuthView, pub cache_duration_secs: i64, pub topics: Vec<Topic> }
  pub enum AuthView { None, Token, Basic { username: String } }
  pub enum InstanceStatus { Connecting, Connected, Problem, AuthFailed }
  pub struct RecentView { pub instance_id: String, pub host: String, pub topic: String, pub message_id: String, pub time: i64, pub title: String, pub body: String, pub has_click: bool }
  pub struct GapView { pub instance_id: String, pub host: String, pub topic: String, pub detected_at: i64 }
  pub fn build_snapshot(settings: &Settings, statuses: &BTreeMap<String, (ConnStatus, i64)>, recent: &VecDeque<RecentMessage>, gaps: &[GapFlag], now: i64) -> UiSnapshot;
  // status.rs
  pub fn instance_status(status: Option<&(ConnStatus, i64)>, now: i64) -> InstanceStatus; // gleiche Schonfrist wie aggregate
  ```
  Alle Typen `Clone, Debug, PartialEq, Serialize, Deserialize` (Drafts mit geschwärztem Debug aus Task 1).

- [ ] **Step 1: Failing tests**
```rust
#[test] fn messages_roundtrip_as_single_lines() { /* jede Variante: write_msg → genau ein '\n' → read_msg == Original */ }
#[test] fn eof_is_none_and_garbage_is_invalid_data() {}
#[test] fn oversized_line_rejected() {}
#[test] fn snapshot_never_contains_secrets() { /* MemoryStore-Secret "s3cret", Token-Instanz → serde_json::to_string(&snapshot) enthält "s3cret" nicht; auth == AuthView::Token */ }
#[test] fn snapshot_status_uses_grace() { /* Connecting seit now-10 → Connecting; seit now-20 → Problem; AuthFailed sofort */ }
#[test] fn snapshot_recent_has_click_flag_not_url() {}
#[test] fn snapshot_hides_disconnect_error_text() {}
```
- [ ] **Step 2: Run** → FAIL · **Step 3: Implement** · **Step 4: Run** `cargo test ipc snapshot status` → PASS
- [ ] **Step 5: Commit** `feat: UI IPC protocol and snapshot`

---

### Task 3: Engine – Snapshots und einfache UI-Anfragen

**Files:**
- Modify: `src/engine.rs`, `src/dispatcher.rs`, `tests/engine_fake.rs`

**Interfaces:**
- Consumes: `UiRequest`, `CoreMsg`, `UiSnapshot`, `build_snapshot`
- Produces:
  - `Command::Ui(UiRequest)` und `Command::ResendSnapshot` (neue Varianten)
  - `EngineDeps` + `pub on_ui: Box<dyn Fn(CoreMsg) + Send>` (Snapshots, Test- und Speicherergebnisse)
  - `Dispatcher::set_topic_urgent(&mut self, instance_id, topic, Option<bool>) -> bool`, `set_urgent_global(&mut self, bool) -> bool`, `dismiss_gap(&mut self, instance_id, topic) -> bool`

Verhalten: Nach jedem `settle()` Snapshot bauen und nur bei Änderung über `on_ui(CoreMsg::Snapshot)` melden; `ResendSnapshot` meldet immer. `PauseAll`/`SetTopicMuted` wie die bestehenden Commands. `OpenRecent` sucht die Nachricht in `recent` und öffnet deren `click` über `actions::perform(Open)` (bestehende `is_openable`-Prüfung). `SendTestNotification` zeigt eine Notification (Titel `strings.test_title`, Text `strings.test_body`, Level Active) über die Delivery-Queue. `OpenConfigDir`/`OpenLogs`/`OpenSettings`/`OpenWizard`/`Quit` behandelt die App-Shell (Task 10) – die Engine ignoriert sie mit `debug!`.

- [ ] **Step 1: Failing tests** (`tests/engine_fake.rs`)
```rust
#[tokio::test] async fn snapshot_sent_on_start_and_on_change_only() { /* Start → 1 Snapshot; Nachricht → neuer Snapshot mit recent[0]; Tick ohne Änderung → keiner */ }
#[tokio::test] async fn ui_mute_and_urgent_are_persisted() { /* SetTopicMuted + SetTopicUrgent → settings.json enthält beides, Snapshot spiegelt es */ }
#[tokio::test] async fn dismiss_gap_removes_flag() {}
#[tokio::test] async fn open_recent_uses_stored_click_only() { /* RecordingOpener bekommt die click-URL der Nachricht; unbekannte id → nichts */ }
#[tokio::test] async fn test_notification_is_shown() {}
```
- [ ] **Step 2–4:** RED → implementieren → `cargo test --test engine_fake` + `cargo test` PASS
- [ ] **Step 5: Commit** `feat: engine snapshots and UI requests`

---

### Task 4: Engine – Server speichern/entfernen mit Live-Neustart

**Files:**
- Modify: `src/engine.rs`, `tests/engine_fake.rs`

**Interfaces:**
- Consumes: `apply_draft`, `remove_instance` (Task 1), `SecretStore` (EngineDeps hält bereits `Arc<dyn SecretStore>` – im Engine-Struct aufbewahren)
- Produces: Behandlung von `UiRequest::SaveInstance { request_id, draft }` → `CoreMsg::SaveResult { request_id, Ok(id) | Err(message) }` und `UiRequest::RemoveInstance`.

Verhalten: Neue Settings sofort speichern. Für die betroffene Instanz: Verbindung stoppen und mit neuen Daten starten, wenn sich URL, Auth oder Topic-Liste geändert haben (nur Stummschaltung/Urgent geändert → kein Neustart). `InstanceState` bleibt erhalten, wenn die URL gleich bleibt; bei geänderter URL oder neuer Instanz frisch. Entfernen: Verbindung stoppen, `InstanceState`, Status, Gaps und `recent`-Einträge der Instanz löschen. Fehlertext in `SaveResult` = äußerste Meldung von `apply_draft` (ohne Eingabewerte).

- [ ] **Step 1: Failing tests**
```rust
#[tokio::test] async fn save_new_instance_connects_live() { /* leere Settings, SaveInstance(Fake-Server) → SaveResult Ok, Stream-Anfrage kommt an, Nachricht wird gezeigt */ }
#[tokio::test] async fn edit_topics_keeps_state_no_duplicates() { /* Nachricht X empfangen, Topic hinzufügen → Neustart; Fake-Poll liefert X erneut → X nicht nochmal gezeigt */ }
#[tokio::test] async fn mute_only_change_does_not_restart() { /* Zahl der Stream-Anfragen bleibt gleich */ }
#[tokio::test] async fn remove_instance_stops_connection_and_cleans_up() {}
#[tokio::test] async fn invalid_save_returns_error_and_changes_nothing() {}
```
- [ ] **Step 2–4:** RED → implementieren → PASS (`cargo test --test engine_fake`, `cargo test`)
- [ ] **Step 5: Commit** `feat: live instance save and removal in the engine`

---

### Task 5: Verbindungstest

**Files:**
- Create: `src/conntest.rs`
- Modify: `src/ntfy/client.rs`, `src/engine.rs`, `tests/connection_fake.rs` oder neues `tests/conntest_fake.rs`

**Interfaces:**
- Produces:
  ```rust
  // ntfy/client.rs
  pub async fn health(http: &reqwest::Client, base: &str) -> Result<bool, NtfyError>; // GET {base}/v1/health, 10 s; true bei 200 + JSON {"healthy":true}; bei 404 (ältere Server) Fallback: GET {base}/ntfy-native-probe/json?poll=1&since=1s → true, wenn Content-Type mit application/x-ndjson beginnt; sonst (HTML, anderes JSON) Ok(false)
  pub async fn topic_readable(http: &reqwest::Client, base: &str, topic: &str, auth: Option<&Auth>) -> Result<bool, NtfyError>; // GET {base}/{topic}/json?poll=1&since=1s, 10 s; 2xx → true, 403 → false, 401 → Err(Auth)
  // conntest.rs
  pub async fn test_connection(http: &reqwest::Client, draft: &InstanceDraft, stored: Option<Auth>) -> TestOutcome;
  ```
Ablauf `test_connection`: URL validieren (`InvalidInput`), `health` (Netzfehler → `Unreachable` mit kurzer Fehlerart ohne URL; `false` → `NotNtfy`), Auth aus Draft bzw. `stored` bei „behalten“; wenn Draft keine Topics hat: `topic_readable(… "ntfy-native-auth-check" …)` nur zur Auth-Prüfung (401 → `AuthFailed`, sonst Ok mit leerer Liste); sonst jedes Topic prüfen → `Ok{topics}`. Engine: `UiRequest::TestConnection` läuft als eigener Task (blockiert die Engine nicht) und meldet `CoreMsg::TestResult`.

- [ ] **Step 1: Failing tests** (Fake-Server)
```rust
#[tokio::test] async fn healthy_server_with_readable_topics() {}
#[tokio::test] async fn unreachable_server() {}
#[tokio::test] async fn html_server_is_not_ntfy() {}
#[tokio::test] async fn bad_token_is_auth_failed() {}
#[tokio::test] async fn forbidden_topic_is_not_readable() { /* 403 für "secret" → readable false, anderes true */ }
#[tokio::test] async fn invalid_url_is_invalid_input_without_echo() {}
#[tokio::test] async fn engine_answers_test_request_without_blocking() { /* Test läuft gegen hängenden Server; gleichzeitig PauseAll wird sofort verarbeitet */ }
```
- [ ] **Step 2–4:** RED → implementieren → PASS
- [ ] **Step 5: E2E** gegen Docker-ntfy (`scripts/ntfy-test-server.sh start`): `health` true, offenes Topic lesbar. Ergebnis im Commit-Text.
- [ ] **Step 6: Commit** `feat: connection test for the setup wizard`

---

### Task 6: UI-Grundlagen – Texte, Zeiten, Pausen, Popover-Position

**Files:**
- Create: `src/ui/mod.rs`, `src/ui/text.rs`, `src/ui/time.rs`, `src/ui/layout.rs`
- Modify: `src/i18n.rs` (neue `UiStrings`), `Cargo.toml` (`chrono` mit `clock`)

**Interfaces:**
- Produces:
  ```rust
  // i18n.rs
  pub struct UiStrings { /* Felder siehe Tabelle */ }
  pub fn ui_strings_for(locale: &str) -> &'static UiStrings; pub fn ui_strings() -> &'static UiStrings;
  // ui/time.rs
  pub enum PauseChoice { Minutes15, Hour1, UntilTomorrow, Indefinitely, Resume }
  pub fn pause_until<Tz: chrono::TimeZone>(choice: PauseChoice, now: chrono::DateTime<Tz>) -> Option<i64>; // UntilTomorrow = nächster Tag 08:00 lokal; Indefinitely = i64::MAX; Resume = None
  pub fn format_age(now: i64, then: i64, s: &UiStrings) -> String;  // <60 s „gerade eben“, <60 min „vor {n} Min.“, <24 h „vor {n} Std.“, sonst „dd.MM. HH:mm“ (lokal)
  pub fn format_paused(until: i64, now: i64, s: &UiStrings) -> String; // „Pausiert bis HH:mm“ / „… bis morgen HH:mm“ / „Pausiert“ (i64::MAX)
  // ui/layout.rs
  pub struct AnchorRect { pub x: f64, pub y: f64, pub w: f64, pub h: f64 }   // physische Pixel vom Tray
  pub fn popover_position(anchor: &AnchorRect, size_logical: (f32, f32), scale: f32, monitor_physical: (f64, f64, f64, f64)) -> (f32, f32); // logische Position
  pub fn parse_anchor(s: &str) -> Option<AnchorRect>; // "x,y,w,h"
  ```
Popover-Regeln: horizontal rechtsbündig an der Icon-Mitte, in den Monitor geklemmt (8 px Rand). Liegt das Icon in der oberen Monitorhälfte → Popover darunter (macOS-Menüleiste), sonst darüber (Windows-Taskleiste). `monitor_physical` = (x, y, w, h) des Monitors, auf dem das Icon liegt.

`UiStrings` (DE / EN) – Pflichtfelder: `popover_title` „ntfy“ / „ntfy“; `status_connected` „Verbunden“ / „Connected“; `status_connecting` „Verbinde …“ / „Connecting …“; `status_problem` „Verbindungsproblem“ / „Connection problem“; `status_auth_failed` „Anmeldung fehlgeschlagen“ / „Login failed“; `recent_heading` „Zuletzt“ / „Recent“; `recent_empty` „Noch keine Nachrichten“ / „No messages yet“; `gap_hint` „Nachrichten möglicherweise verpasst: {topic}“ / „Messages may have been missed: {topic}“; `dismiss` „Ausblenden“ / „Dismiss“; `pause` „Pause“ / „Pause“; `pause_15m` „15 Minuten“ / „15 minutes“; `pause_1h` „1 Stunde“ / „1 hour“; `pause_tomorrow` „Bis morgen früh“ / „Until tomorrow morning“; `pause_forever` „Bis ich fortsetze“ / „Until I resume“; `resume` „Fortsetzen“ / „Resume“; `paused_until` „Pausiert bis {time}“ / „Paused until {time}“; `paused_until_tomorrow` „Pausiert bis morgen {time}“ / „Paused until tomorrow {time}“; `paused` „Pausiert“ / „Paused“; `add_topic` „+ Topic“ / „+ Topic“; `settings` „Einstellungen“ / „Settings“; `quit` „Beenden“ / „Quit“; `just_now` „gerade eben“ / „just now“; `minutes_ago` „vor {n} Min.“ / „{n} min ago“; `hours_ago` „vor {n} Std.“ / „{n} h ago“; `wizard_title` „ntfy-native einrichten“ / „Set up ntfy-native“; `server_url` „Server-Adresse“ / „Server address“; `server_hint` „z. B. https://ntfy.sh oder dein eigener Server“ / „e.g. https://ntfy.sh or your own server“; `auth_heading` „Anmeldung“ / „Login“; `auth_none` „Ohne Anmeldung“ / „No login“; `auth_token` „Access-Token“ / „Access token“; `auth_basic` „Benutzername und Passwort“ / „Username and password“; `username` „Benutzername“ / „Username“; `password` „Passwort“ / „Password“; `keep_secret_hint` „Leer lassen, um das gespeicherte zu behalten“ / „Leave empty to keep the stored one“; `topics_heading` „Topics“ / „Topics“; `topic_name` „Topic-Name“ / „Topic name“; `topic_not_readable` „Kein Leserecht“ / „No read access“; `notifications_heading` „Benachrichtigungen“ / „Notifications“; `send_test` „Test-Benachrichtigung senden“ / „Send test notification“; `notifications_hint_macos` „Falls nichts erscheint: Systemeinstellungen › Mitteilungen › ntfy-native erlauben.“ / „If nothing appears: allow ntfy-native in System Settings › Notifications.“; `notifications_hint_windows` „Falls nichts erscheint: Einstellungen › System › Benachrichtigungen › ntfy-native einschalten.“ / „If nothing appears: turn on ntfy-native in Settings › System › Notifications.“; `urgent_global` „Dringende Nachrichten (Priorität 5) dürfen „Nicht stören“ durchbrechen“ / „Urgent messages (priority 5) may break through Do Not Disturb“; `urgent_default` „Standard“ / „Default“; `urgent_on` „An“ / „On“; `urgent_off` „Aus“ / „Off“; `muted` „Stumm“ / „Muted“; `back` „Zurück“ / „Back“; `next` „Weiter“ / „Next“; `finish` „Fertig“ / „Finish“; `cancel` „Abbrechen“ / „Cancel“; `save` „Speichern“ / „Save“; `testing` „Prüfe Verbindung …“ / „Checking connection …“; `err_unreachable` „Server nicht erreichbar ({reason})“ / „Server unreachable ({reason})“; `err_not_ntfy` „Unter dieser Adresse antwortet kein ntfy-Server“ / „No ntfy server answers at this address“; `err_auth` „Anmeldung abgelehnt – Token bzw. Passwort prüfen“ / „Login rejected – check token or password“; `err_no_topics` „Mindestens ein Topic angeben“ / „Add at least one topic“; `servers_heading` „Server“ / „Servers“; `add_server` „Server hinzufügen“ / „Add server“; `edit` „Bearbeiten“ / „Edit“; `remove` „Entfernen“ / „Remove“; `remove_confirm` „{host} wirklich entfernen?“ / „Really remove {host}?“; `cache_duration` „Server-Cache-Dauer“ / „Server cache duration“; `cache_hint` „Muss zur Einstellung cache-duration des Servers passen“ / „Must match the server's cache-duration setting“; `open_config` / `open_logs` wie Tray; `version` „Version {v}“ / „Version {v}“; `not_configured` „Noch nicht eingerichtet“ / „Not set up yet“; `setup` „Einrichten …“ / „Set up …“. Neue Kern-Strings (in `Strings`): `test_title` „ntfy-native“ / „ntfy-native“, `test_body` „Benachrichtigungen funktionieren.“ / „Notifications work.“, `menu_settings` „Einstellungen …“ / „Settings …“, `menu_setup` „Einrichten …“ / „Set up …“.

- [ ] **Step 1: Failing tests**
```rust
#[test] fn pause_choices() { /* now = 2026-10-19T22:30+02:00: 15m → +900; 1h → +3600; Tomorrow → 2026-10-20T08:00+02:00; Indefinitely → i64::MAX; Resume → None */ }
#[test] fn pause_tomorrow_after_midnight_is_same_day_morning() { /* 01:00 → 08:00 desselben Tages */ }
#[test] fn ages() { /* 30 s → "gerade eben"; 300 s → "vor 5 Min."; 7200 s → "vor 2 Std."; 2 Tage → Datumsformat */ }
#[test] fn popover_position_below_top_tray() { /* macOS: Icon y=0, Monitor 0,0,2880,1800, scale 2 → y logisch = Icon-Unterkante/2 + 4 */ }
#[test] fn popover_position_above_bottom_tray() { /* Windows: Icon y=1040 auf 1920x1080, scale 1 → y = 1040 - h - 4 */ }
#[test] fn popover_position_clamped_to_monitor() { /* Icon ganz rechts / zweiter Monitor mit negativem x */ }
#[test] fn ui_strings_complete_in_both_languages() { /* kein Feld leer, Platzhalter in DE und EN gleich */ }
```
- [ ] **Step 2–4:** RED → implementieren → `cargo test ui:: i18n` PASS
- [ ] **Step 5: Commit** `feat: UI strings, time formatting and popover placement`

---

### Task 7: UI-Prozess – Start, Brücke, Fensterarten

**Files:**
- Create: `src/ui/process.rs` (Brücke stdin/stdout ↔ App), `src/ui/app.rs` (eframe-App mit Modus-Weiche), `src/ui/focus.rs` (macOS/Windows Aktivierung)
- Modify: `src/cli.rs` (`--ui`, `--anchor`, `--monitor`), `src/main.rs`, `Cargo.toml` (`eframe` mit `glow`, ohne Default-Features außer nötigen; `egui_kittest` dev)

**Interfaces:**
- Produces:
  ```rust
  // cli.rs
  pub enum UiMode { Popover, Settings, Wizard, Probe }  // Probe: Test-Modus ohne Fenster
  pub enum CliAction { Run(Cli), Version, Ui(UiArgs) }
  pub struct UiArgs { pub mode: UiMode, pub anchor: Option<AnchorRect>, pub monitor: Option<(f64, f64, f64, f64)>, pub probe_ignore_close: bool }
  // ui/process.rs
  pub struct Bridge { pub inbox: std::sync::mpsc::Receiver<CoreMsg>, pub outbox: std::sync::mpsc::Sender<UiMsg> }
  pub fn start_bridge(on_message: impl Fn() + Send + 'static) -> Bridge; // Leser-Thread (stdin) ruft on_message (→ ctx.request_repaint) ; Schreiber-Thread (stdout); EOF auf stdin → Prozess beendet sich
  pub fn run_ui(args: UiArgs) -> anyhow::Result<()>;
  ```
Verhalten: `--ui` startet **ohne** Instance-Lock, Logging in dieselben Logdateien mit Präfix `ui`. Handshake: UI schickt `UiMsg::Hello{PROTOCOL_VERSION}`, erwartet `CoreMsg::Hello` mit gleicher Version (sonst Exit 2). `CoreMsg::Close` oder EOF → Fenster schließen, Prozess endet. Fenster: Popover 360×480 logisch, ohne Rahmen, immer oben, nicht in der Taskleiste, nicht skalierbar, schließt bei Fokusverlust (nachdem es einmal Fokus hatte) und mit Esc; Einstellungen 560×620 und Assistent 520×560 mit normalem Rahmen. Beim ersten Frame aktivieren: macOS `NSApplication.activate()` (objc2-app-kit), Windows `SetForegroundWindow` auf das eigene Fenster (Kern ruft vor dem Start `AllowSetForegroundWindow(ASFW_ANY)`, Task 10). `Probe`-Modus: kein Fenster; Handshake, danach für jede `CoreMsg::Snapshot` eine `UiMsg::Request(UiRequest::SendTestNotification)` senden; `Close`/EOF → Exit 0 (für Task-10-Tests). Zusätzliches Test-Flag `--probe-ignore-close` (`UiArgs.probe_ignore_close: bool`): ignoriert `Close` und EOF und schläft, bis es beendet wird.

- [ ] **Step 1: Failing tests**
```rust
#[test] fn parses_ui_args() { /* --ui popover --anchor 10,0,22,22 --monitor 0,0,2880,1800 → UiArgs{...} ; --ui bogus → Err */ }
#[test] fn ui_flag_is_not_the_app() { /* CliAction::Ui statt Run */ }
// tests/ui_probe.rs (startet CARGO_BIN_EXE_ntfy-native --ui probe):
#[test] fn probe_handshakes_and_exits_on_eof() {}
#[test] fn probe_rejects_wrong_protocol_version() { /* Exit-Code 2 */ }
#[test] fn probe_answers_snapshot_with_request() {}
```
- [ ] **Step 2–4:** RED → implementieren → PASS (Tests + Windows-clippy)
- [ ] **Step 5: Manuell** `cargo run -- --ui settings` (macOS) öffnet ein leeres Fenster, das sich mit Schließen-Knopf beendet; Prozess ist danach weg.
- [ ] **Step 6: Commit** `feat: UI process with stdin/stdout bridge`

---

### Task 8: Popover

**Files:**
- Create: `src/ui/popover.rs`
- Modify: `src/ui/app.rs`

**Interfaces:**
- Consumes: `UiSnapshot`, `UiRequest`, `UiStrings`, `format_age`, `format_paused`, `pause_until`, `popover_position`
- Produces: `pub fn popover_ui(ui: &mut egui::Ui, snap: &UiSnapshot, s: &UiStrings, now: i64, out: &mut Vec<UiRequest>)` – zeichnet und sammelt Anfragen (keine Seiteneffekte, testbar).

Inhalt (Mockup B): Kopf „ntfy“ + Gesamtstatus (grün/rot/grau Punkt + Text, bei Pause `format_paused`); pro Instanz Host mit Status-Punkt; darunter Topics mit Schalter (an = nicht stumm) → `SetTopicMuted`; Lücken-Hinweise (`gap_hint`) mit „Ausblenden“ → `DismissGap`; „Zuletzt“: bis 20 Einträge (Topic · Alter, Titel fett, Text 2 Zeilen gekürzt), Klick bei `has_click` → `OpenRecent`; Fußleiste: „+ Topic“ (Inline-Feld + Instanzwahl bei mehreren → `SaveInstance` mit ergänztem Topic; Fehler aus `SaveResult` inline), „Pause ▾“ (Menü mit den fünf `PauseChoice` → `PauseAll`), „Einstellungen“ → `OpenSettings`, „Beenden“ → `Quit`. Nicht eingerichtet → nur Hinweis `not_configured` und Knopf `setup` → `OpenWizard`.

- [ ] **Step 1: Failing tests** (egui_kittest, Headless)
```rust
#[test] fn shows_status_topics_and_recent() { /* Harness mit Snapshot (2 Instanzen, 3 Nachrichten) → Labels für Hosts, Topic-Namen und Titel per get_by_label auffindbar */ }
#[test] fn toggling_topic_sends_mute_request() { /* Klick auf Schalter "backups" → out enthält SetTopicMuted{muted:true} */ }
#[test] fn pause_menu_sends_until() {}
#[test] fn unconfigured_shows_setup_button() {}
#[test] fn recent_without_click_is_not_clickable() {}
```
- [ ] **Step 2–4:** RED → implementieren → PASS
- [ ] **Step 5: Commit** `feat: tray popover`

---

### Task 9: Assistent und Einstellungen

**Files:**
- Create: `src/ui/wizard.rs` (Zustandsautomat + Ansicht), `src/ui/settings_view.rs`
- Modify: `src/ui/app.rs`

**Interfaces:**
- Produces:
  ```rust
  pub enum WizardStep { Server, Auth, Topics, Notifications, Done }
  pub struct Wizard { pub step: WizardStep, pub draft: InstanceDraft, pub editing: bool, pub busy: Option<u64>, pub error: Option<String>, pub topic_access: Vec<TopicAccess> }
  impl Wizard {
      pub fn new() -> Self;                                   // neue Instanz
      pub fn edit(view: &InstanceView) -> Self;               // vorbefüllt, Secrets leer = behalten
      pub fn next(&mut self, next_id: &mut u64) -> Option<UiRequest>; // Server/Auth/Topics → TestConnection; Notifications → SaveInstance
      pub fn back(&mut self);
      pub fn on_test_result(&mut self, request_id: u64, outcome: &TestOutcome);
      pub fn on_save_result(&mut self, request_id: u64, result: &Result<String, String>);
  }
  pub fn wizard_ui(ui: &mut egui::Ui, w: &mut Wizard, s: &UiStrings, out: &mut Vec<UiRequest>, next_id: &mut u64);
  pub fn settings_ui(ui: &mut egui::Ui, snap: &UiSnapshot, state: &mut SettingsState, s: &UiStrings, out: &mut Vec<UiRequest>, next_id: &mut u64);
  ```
Ablauf: **Server** (URL) → Test (`InvalidInput`/`Unreachable`/`NotNtfy` → Fehler bleibt auf Seite) → **Auth** (keine / Token / Benutzer+Passwort; Weiter testet Auth ohne Topics; `AuthFailed` → `err_auth`) → **Topics** (Liste, Hinzufügen/Entfernen, lokale Prüfung `valid_topic_name`, keine Duplikate, mind. 1; Weiter testet jedes Topic; nicht lesbare markieren und blockieren, bis entfernt) → **Benachrichtigungen** (Knopf `send_test` → `SendTestNotification`, Plattform-Hinweis, globaler Urgent-Schalter → `SetUrgentGlobal`) → Fertig sendet `SaveInstance`; Erfolg → `Done`, danach Fenster schließen. Antworten mit fremder `request_id` ignorieren. Im Bearbeiten-Modus beginnt der Assistent bei **Server** mit vorbefüllten Feldern.
Einstellungen: Serverliste (Host, Status, `Bearbeiten` → eingebetteter Assistent im Bearbeiten-Modus, `Entfernen` mit Bestätigung → `RemoveInstance`), `Server hinzufügen`; pro Topic dreistufiger Urgent-Schalter (Standard/An/Aus → `SetTopicUrgent`) und Stumm-Schalter; globaler Urgent-Schalter; Cache-Dauer pro Server (12 h/24 h/72 h/7 Tage → `SaveInstance`); Knöpfe Konfigurationsordner/Logs; Versionszeile.

- [ ] **Step 1: Failing tests** (reine Logik + kittest)
```rust
#[test] fn server_step_requires_test_ok() { /* next → TestConnection; on_test_result(NotNtfy) → bleibt Server, error gesetzt; Ok → Auth */ }
#[test] fn stale_test_results_are_ignored() {}
#[test] fn topics_step_blocks_unreadable_and_invalid() {}
#[test] fn finish_sends_save_and_done_on_ok() {}
#[test] fn edit_mode_keeps_secrets_empty() { /* Wizard::edit(Token-Instanz) → AuthDraft::Token{token: None} */ }
#[test] fn settings_remove_needs_confirmation() { /* erster Klick zeigt Bestätigung, zweiter sendet RemoveInstance */ }
#[test] fn settings_urgent_tristate() {}
```
- [ ] **Step 2–4:** RED → implementieren → PASS
- [ ] **Step 5: Commit** `feat: setup wizard and settings window`

---

### Task 10: App-Shell – UI-Host, Tray-Klick, Menü, Erststart

**Files:**
- Create: `src/ui_host.rs`, `tests/ui_host.rs`
- Modify: `src/app.rs`, `src/tray.rs`, `src/engine.rs` (Verdrahtung `on_ui`)

**Interfaces:**
- Consumes: `CoreMsg`, `UiMsg`, `UiRequest`, `Command::Ui`, `Command::ResendSnapshot`, `UiArgs`
- Produces:
  ```rust
  pub enum HostEvent { Open(UiMode), Quit, ChildExited }
  pub struct UiHost { /* exe, aktuelles Kind (Modus, Child, Writer-Sender), letzter Snapshot */ }
  impl UiHost {
      pub fn new(exe: PathBuf, engine: EngineHandle, on_event: Arc<dyn Fn(HostEvent) + Send + Sync>) -> Self;
      pub fn open(&mut self, args: UiArgs);          // ersetzt ein laufendes Kind (Close, 1 s warten, dann kill)
      pub fn toggle_popover(&mut self, args: UiArgs); // offenes Popover → schließen, sonst öffnen
      pub fn deliver(&self, msg: CoreMsg);            // an Kind, falls vorhanden; Snapshot wird zusätzlich gemerkt
      pub fn close(&mut self);
      pub fn current_mode(&self) -> Option<UiMode>;
  }
  ```
Verhalten: Start mit `Command::new(exe).arg("--ui")…`, stdin/stdout gepiped, stderr geerbt. Nach `UiMsg::Hello` sendet der Host `CoreMsg::Hello` + letzten Snapshot. Leser-Thread: `Request(OpenSettings|OpenWizard)` → `HostEvent::Open`, `Quit` → `HostEvent::Quit`, `OpenConfigDir`/`OpenLogs` → Ordner öffnen (bestehende `open_local_dir`), Rest → `Command::Ui`. EOF/Fehler → `HostEvent::ChildExited` (Kind wird `wait()`et – keine Zombies). Engine-`on_ui` → `UiHost::deliver` über `Arc<Mutex<UiHost>>`-freien Kanal (Engine darf nie blockieren). Windows: vor jedem Start `AllowSetForegroundWindow(ASFW_ANY)`.
Tray (`tray.rs`/`app.rs`): `with_menu_on_left_click(false)`; Linksklick (`TrayIconEvent::Click` mit linker Taste, Button losgelassen) → `toggle_popover` mit Anker = Icon-Rechteck und Monitor (tao `monitor_from_point`); Rechtsklick → Menü wie bisher plus „Einstellungen …“ (`menu_settings`) und, solange nicht eingerichtet, „Einrichten …“ (`menu_setup`). Erststart: keine Instanzen und kein Importfehler → Assistent automatisch öffnen. Beim Beenden der App: Kind schließen.

- [ ] **Step 1: Failing tests** (`tests/ui_host.rs`, mit `--ui probe`)
```rust
#[test] fn opens_child_and_forwards_requests() { /* Host öffnet Probe, deliver(Snapshot) → Engine-Kanal empfängt Command::Ui(SendTestNotification) */ }
#[test] fn toggle_closes_open_popover() { /* zweimal toggle → Kind beendet, current_mode None */ }
#[test] fn child_exit_is_detected() { /* Kind beendet sich selbst (Close) → HostEvent::ChildExited, kein Zombie (try_wait Some) */ }
#[test] fn hanging_child_is_killed_on_close() { /* Probe mit --probe-ignore-close (Test-Flag) → close() beendet es nach ≤ 1,5 s */ }
#[test] fn new_child_gets_last_snapshot() {}
```
- [ ] **Step 2–4:** RED → implementieren → PASS, Windows-clippy grün
- [ ] **Step 5: Manueller E2E (macOS)** mit Docker-ntfy und Dev-Bundle:
  - Ohne Konfiguration startet der Assistent; mit Server, ohne Anmeldung, Topic `e2e` durchklicken → Tray „Verbunden“, Testbenachrichtigung erscheint (Mitteilungen erlaubt).
  - Linksklick aufs Tray → Popover am Icon; Topic-Schalter, Pause 15 min, Nachricht anklicken öffnet Click-URL; Klick daneben schließt es.
  - Einstellungen: Topic hinzufügen, Server bearbeiten (Token behalten), Server entfernen.
  - **Speicher:** Popover 5× öffnen/schließen, 1 min warten → `pgrep -f "ntfy-native --ui"` leer, `footprint` des Kerns < 30 MB (Wert im Commit-Text).
- [ ] **Step 6: README** (Deutsch): Abschnitte „Popover“, „Einrichtungsassistent“, „Einstellungen“; `config.yaml` als Alternative für automatische Installation.
- [ ] **Step 7: Commit** `feat: tray popover, setup wizard and settings wired into the app`

---

### Task 11: Windows-Abnahme vorbereiten

**Files:**
- Modify: `README.md` (Abschnitt „Manuelle Prüfung Windows“)

- [ ] **Step 1:** Prüfschritte für den Auftraggeber schreiben: Popover-Position an der Taskleiste (unten, auch links/oben angedockt), Fokus beim Öffnen, Schließen bei Klick daneben, Assistent beim Erststart, Speicher im Task-Manager (Kern < 30 MB nach Schließen, kein zweiter `ntfy-native.exe`-Prozess).
- [ ] **Step 2: Commit** `docs: manual Windows acceptance steps for the UI`
