# ntfy-native Kern-Bridge – Implementation Plan (Teil 1 von 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ein lauffähiger, schlanker Hintergrunddienst für macOS und Windows, der ntfy-Topics aus einer importierten `config.yaml` abonniert, verpasste Nachrichten zuverlässig nachholt bzw. Lücken meldet und jede Nachricht als native OS-Benachrichtigung zustellt – mit Tray-Icon für den Status.

**Architecture:** Eine Rust-Binary (`lib` + `bin`). Der Hauptthread führt die Plattform-Ereignisschleife (`tao`) mit Tray-Icon und den nativen Notification-Callbacks aus. Ein zweiter Thread betreibt eine `tokio`-current-thread-Runtime mit der *Engine*: pro ntfy-Instanz eine Verbindung (Catch-up-Poll + JSON-Stream mit Keepalive-Watchdog), ein reiner *Dispatcher* (Pausen, Bündelung, letzte Nachrichten) und die Persistenz. Plattformcode steckt ausschließlich hinter dem Trait `Notifier`.

**Tech Stack:** Rust (Edition 2024), tokio (current_thread), reqwest (`native-tls`, ohne Default-Features), serde/serde_json/serde_yaml_ng, keyring, tao + tray-icon, objc2 + objc2-user-notifications (macOS), windows-Crate (WinRT-Toasts, COM, Registry), if-watch, lexopt, directories, humantime, httpdate, sys-locale, open, tracing. Tests: Standard-Testharness, axum als Fake-ntfy-Server, Docker-ntfy für E2E.

**Spec:** `docs/superpowers/specs/2026-10-18-ntfy-native-design.md`

**Gesamtaufteilung:** Dieser Plan (Teil 1) baut den Kern bis zur nutzbaren App mit `config.yaml`-Einrichtung. **Teil 2** (eigener Plan): UI-Prozess mit Popover, Wizard, Einstellungen, IPC zum Kern. **Teil 3** (eigener Plan): Autostart, Update-Prüfung, `.dmg`/`.msi`, Signierung/Notarisierung, Release-CI, automatisierter Speicher-Check. Teil 1 legt die Schnittstellen (`Command`, `TrayState`, `Settings`, `State`) so an, dass Teil 2 sie nur nutzt.

## Global Constraints

- Name `ntfy-native`; macOS Bundle-ID und Windows AppUserModelID **`dev.hemp0r.ntfy-native`** – auch Keychain-/Credential-Service-Name. Nie ändern.
- Plattformen: macOS aktuelle + vorherige Hauptversion (26, 15 → `LSMinimumSystemVersion` 15.0), Apple Silicon + Intel; Windows 10/11 x64 (ARM64 wünschenswert). Linux ist kein Ziel.
- Benachrichtigungen nur über native OS-APIs (`UNUserNotificationCenter` bzw. WinRT-Toasts). Keine Eigenbau-Pop-ups. **Critical Alerts werden nicht verwendet.**
- Ruhezustand < 50 MB RAM (Summe aller App-Prozesse). **Budget für den Kern aus diesem Plan: < 30 MB**, damit Teil 2 Luft hat. Keine WebView, tokio nur als current_thread-Runtime.
- Geheimnisse nie dauerhaft im Klartext auf der Platte; nur im macOS-Schlüsselbund bzw. in der Windows-Anmeldeinformationsverwaltung.
- `config.yaml` ist ein einmaliger Import (Spec 5.9); danach ist der App-Zustand (`settings.json`) die einzige Quelle der Wahrheit.
- Nicht in v1: Senden/Publishen, Archiv über `RECENT_MAX = 20` letzte Nachrichten hinaus.
- Lizenz `MIT OR Apache-2.0`.
- Alle für Nutzer sichtbaren Texte über `src/i18n.rs` (Deutsch bei System-Locale `de*`, sonst Englisch).
- Umgebung des Auftraggebers: Intel-Mac, Homebrew-Rust 1.98 **ohne rustup**, Docker vorhanden. Für `cargo check --target x86_64-pc-windows-msvc` wird rustup benötigt – **vor einer Toolchain-Änderung den Auftraggeber fragen**; sonst prüft die CI (Task 1) den Windows-Build.

## Review Focus

1. **ntfy-Server nicht erreichbar oder falsche Zugangsdaten beim Start** → kein Absturz, Tray zeigt nach Schonfrist „Verbindungsproblem“ bzw. sofort „Anmeldung fehlgeschlagen“, Wiederholung mit Backoff bzw. `auth_retry`, keine Benachrichtigungsflut. Tests: Task 8 (`unauthorized_reports_auth_failed_and_waits`, `server_down_retries_with_backoff`), Task 10 (`aggregate_*`).
2. **Unsaubere Serverdaten** (Nicht-JSON-Zeilen, unbekannte Event-Typen, fehlende/ungültige Priorität, unbekannte Action-Typen, leere Nachricht) → Nachricht kommt trotzdem sinnvoll an, nichts stürzt ab. Tests: Task 5, Task 8 (`invalid_json_line_is_skipped`).
3. **Beschädigte oder fehlende `settings.json`/`state.json`** (Absturz beim Schreiben, Handbearbeitung) → App startet, beschädigte Datei wird gesichert statt überschrieben. Tests: Task 2 (`corrupt_file_is_backed_up`, `save_is_atomic`).
4. **Gefährliche/seltsame URLs und Anhänge** (`file:`, `javascript:`, riesige oder Nicht-Bild-Anhänge) → werden nie geöffnet bzw. nicht über die Grenzen hinaus geladen. Tests: Task 9, Task 11.
5. **Doppelte Zustellung** über Poll/Stream-Überlappung, Reconnect und App-Neustart → genau eine Benachrichtigung pro Nachrichten-ID. Tests: Task 7, Task 8 (`duplicate_between_poll_and_stream_delivered_once`), Task 12 (`restart_with_saved_state_does_not_renotify`).

---

### Task 1: Crate-Gerüst, CLI, Pfade, CI

**Files:**
- Create: `Cargo.toml`, `src/lib.rs`, `src/main.rs`, `src/cli.rs`, `src/paths.rs`, `LICENSE-MIT`, `LICENSE-APACHE`, `README.md`, `.github/workflows/ci.yml`

**Interfaces:**
- Produces:
  - `paths::APP_NAME: &str = "ntfy-native"`, `paths::APP_ID: &str = "dev.hemp0r.ntfy-native"`
  - `pub struct AppPaths { pub config_dir: PathBuf, pub data_dir: PathBuf, pub cache_dir: PathBuf, pub log_dir: PathBuf }`
  - `AppPaths::platform() -> anyhow::Result<AppPaths>` (über `directories::ProjectDirs::from("dev", "hemp0r", "ntfy-native")`, `log_dir = data_dir/logs`)
  - `AppPaths::under(root: &Path) -> AppPaths` (Unterordner `config`, `data`, `cache`, `data/logs`; für Tests)
  - Methoden: `settings_file()` = `data_dir/settings.json`, `state_file()` = `data_dir/state.json`, `default_import_file()` = `config_dir/config.yaml`, `lock_file()` = `data_dir/instance.lock`, `media_dir()` = `cache_dir/media`, `ensure_dirs(&self) -> io::Result<()>`
  - `pub enum CliAction { Run(Cli), Version }`, `pub struct Cli { pub config: Option<PathBuf>, pub import: Option<PathBuf> }`
  - `cli::parse_args(args: impl IntoIterator<Item = OsString>) -> anyhow::Result<CliAction>` (Argumente **ohne** Programmnamen)

- [ ] **Step 1: Cargo-Projekt anlegen**

`cargo init --lib --name ntfy-native`, dazu `src/main.rs`. In `Cargo.toml`: `edition = "2024"`, `rust-version = "1.89"` (für `File::try_lock`), `license = "MIT OR Apache-2.0"`, `[profile.release] lto = true, codegen-units = 1, strip = true`. Dependencies vorerst: `anyhow`, `directories`, `lexopt`. Crates jeweils mit `cargo add` in der aktuellen Version hinzufügen.

- [ ] **Step 2: Failing tests schreiben** (`src/cli.rs`, `src/paths.rs`, jeweils `#[cfg(test)]`)

```rust
#[test] fn parses_config_flag() {
    let a = parse_args(["--config", "/tmp/c.yaml"].map(OsString::from)).unwrap();
    assert!(matches!(a, CliAction::Run(Cli { config: Some(p), import: None }) if p == Path::new("/tmp/c.yaml")));
}
#[test] fn parses_import_flag() { /* --import x.yaml → import == Some("x.yaml") */ }
#[test] fn version_flag() { /* --version → CliAction::Version */ }
#[test] fn ignores_com_activation_args() {
    // Windows-COM startet uns mit diesen Argumenten (Task 14)
    let a = parse_args(["-Embedding", "-ToastActivated"].map(OsString::from)).unwrap();
    assert!(matches!(a, CliAction::Run(Cli { config: None, import: None })));
}
#[test] fn rejects_unknown_flag() { assert!(parse_args(["--bogus"].map(OsString::from)).is_err()); }
#[test] fn paths_under_root() {
    let p = AppPaths::under(Path::new("/r"));
    assert_eq!(p.settings_file(), Path::new("/r/data/settings.json"));
    assert_eq!(p.default_import_file(), Path::new("/r/config/config.yaml"));
    assert_eq!(p.media_dir(), Path::new("/r/cache/media"));
}
```

- [ ] **Step 3: Tests ausführen, Fehlschlag prüfen** – `cargo test` → FAIL (Funktionen fehlen)

- [ ] **Step 4: `cli.rs` und `paths.rs` implementieren**; `main.rs` ruft vorerst nur `parse_args(std::env::args_os().skip(1))` auf und gibt bei `Version` `ntfy-native <CARGO_PKG_VERSION>` aus. `lib.rs` deklariert die Module.

- [ ] **Step 5: Tests ausführen** – `cargo test` → PASS

- [ ] **Step 6: Lizenzdateien, README-Gerüst, CI**

`LICENSE-MIT` und `LICENSE-APACHE` (Standardtexte, Copyright „ntfy-native contributors“). `README.md`: ein Absatz zum Zweck (aus Spec 1), Abschnittsüberschriften `## Konfiguration`, `## Server-Empfehlungen`, `## Entwicklung` (werden in späteren Tasks gefüllt). `.github/workflows/ci.yml`: Matrix `macos-latest`, `windows-latest`; Schritte `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.

- [ ] **Step 7: Commit** – `git add -A && git commit -m "chore: scaffold ntfy-native crate, CLI and paths"`

---

### Task 2: Settings, State und atomare Persistenz

**Files:**
- Create: `src/store.rs`, `src/settings.rs`, `src/state.rs`

**Interfaces:**
- Produces:
  - `store::save_json<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()>` – schreibt `<path>.tmp`, `sync_all`, `rename`
  - `pub enum Loaded<T> { Missing, Ok(T), Corrupt { backup: PathBuf } }`
  - `store::load_json<T: DeserializeOwned>(path: &Path) -> anyhow::Result<Loaded<T>>` – bei Parse-Fehler wird die Datei nach `<name>.corrupt-<unix-sekunden>` umbenannt
  - `settings.rs` (alle Felder `#[serde(default)]`, unbekannte Felder ignorieren):
    ```rust
    pub struct Settings { pub version: u32 /*1*/, pub instances: Vec<Instance>, pub urgent_breaks_dnd: bool /*false*/, pub paused_until: Option<i64> /*i64::MAX = unbegrenzt*/ }
    pub struct Instance { pub id: String, pub url: String, pub auth: AuthKind, pub cache_duration_secs: i64 /*43200*/, pub topics: Vec<Topic> }
    #[serde(tag = "type", rename_all = "lowercase")] pub enum AuthKind { None, Token, Basic { username: String } }
    pub struct Topic { pub name: String, pub muted: bool, pub urgent_breaks_dnd: Option<bool> }
    impl Settings { pub fn instance(&self, id: &str) -> Option<&Instance>; pub fn urgent_allowed(&self, instance_id: &str, topic: &str) -> bool }
    impl Instance { pub fn host(&self) -> String /* Host aus url, Fallback url */ }
    ```
  - `state.rs`:
    ```rust
    pub const SEEN_RING: usize = 200; pub const RECENT_MAX: usize = 20;
    pub struct State { pub version: u32, pub instances: BTreeMap<String, InstanceState>, pub recent: VecDeque<RecentMessage>, pub gaps: Vec<GapFlag> }
    pub struct InstanceState { pub watermark: Option<i64>, pub last_message: Option<SeenMessage>, pub seen_ids: VecDeque<String> }
    pub struct SeenMessage { pub id: String, pub time: i64 }
    pub struct RecentMessage { pub instance_id: String, pub topic: String, pub id: String, pub time: i64, pub title: String, pub body: String, pub click: Option<String> }
    pub struct GapFlag { pub instance_id: String, pub topic: String, pub detected_at: i64 }
    ```
    Alle `Clone, Debug, Default, PartialEq, Serialize, Deserialize`.

- [ ] **Step 1: Failing tests schreiben** (in `store.rs`/`settings.rs`, temporäre Verzeichnisse via `tempfile` als dev-dependency)

```rust
#[test] fn roundtrip_settings() { /* save_json → load_json == Loaded::Ok(gleiches Settings) */ }
#[test] fn missing_file_is_missing() { /* load_json(nicht vorhanden) → Loaded::Missing */ }
#[test] fn corrupt_file_is_backed_up() {
    // Datei mit "{nicht json" → Loaded::Corrupt { backup }; backup existiert mit Originalinhalt, Original existiert nicht mehr
}
#[test] fn save_is_atomic() { /* nach save_json existiert kein "<path>.tmp" mehr; Ziel enthält vollständiges JSON */ }
#[test] fn old_json_gets_defaults() {
    let s: Settings = serde_json::from_str(r#"{"instances":[{"id":"a","url":"https://x","topics":[{"name":"t"}]}]}"#).unwrap();
    assert_eq!(s.instances[0].cache_duration_secs, 43200);
    assert_eq!(s.instances[0].auth, AuthKind::None);
    assert!(!s.urgent_breaks_dnd);
}
#[test] fn urgent_allowed_topic_overrides_global() {
    // global false + topic Some(true) → true; global true + topic Some(false) → false; global true + None → true
}
```

- [ ] **Step 2: Tests ausführen** – `cargo test store settings` → FAIL
- [ ] **Step 3: Implementieren** (Abhängigkeiten `serde` mit `derive`, `serde_json`, `url` für `host()`)
- [ ] **Step 4: Tests ausführen** – `cargo test` → PASS
- [ ] **Step 5: Commit** – `git commit -m "feat: settings/state models with atomic JSON persistence"`

---

### Task 3: Secret Store

**Files:**
- Create: `src/secrets.rs`

**Interfaces:**
- Consumes: `settings::{Instance, AuthKind}`, `paths::APP_ID`
- Produces:
  ```rust
  pub trait SecretStore: Send + Sync {
      fn get(&self, account: &str) -> anyhow::Result<Option<String>>;
      fn set(&self, account: &str, secret: &str) -> anyhow::Result<()>;
      fn delete(&self, account: &str) -> anyhow::Result<()>;
  }
  pub struct KeyringStore;            // Service = APP_ID, nativer Keychain/Credential-Manager-Backend
  pub struct MemoryStore { /* Mutex<HashMap<String,String>>, optional fail_on_set: bool */ } // pub, für Tests
  pub fn account_for(instance_id: &str) -> String;   // "instance:<id>"
  #[derive(Clone, Debug, PartialEq)] pub enum Auth { Token(String), Basic { username: String, password: String } }
  pub fn resolve_auth(store: &dyn SecretStore, inst: &Instance) -> anyhow::Result<Option<Auth>>;
  ```

- [ ] **Step 1: Failing tests schreiben**

```rust
#[test] fn memory_roundtrip() { /* set → get Some; delete → get None */ }
#[test] fn resolve_none() { /* AuthKind::None → Ok(None), Store wird nicht gefragt */ }
#[test] fn resolve_token() { /* Secret "tk_abc" unter account_for("a") → Some(Auth::Token("tk_abc")) */ }
#[test] fn resolve_basic() { /* AuthKind::Basic{username:"u"} + Secret "pw" → Some(Auth::Basic{username:"u",password:"pw"}) */ }
#[test] fn resolve_missing_secret_errors() { /* Fehlermeldung enthält inst.url */ }
#[test] #[ignore] fn keyring_roundtrip_manual() { /* KeyringStore set/get/delete mit account "test:<pid>" – manuell auf macOS/Windows ausführen */ }
```

- [ ] **Step 2: Tests ausführen** → FAIL
- [ ] **Step 3: Implementieren.** `keyring` (aktuelle Major-Version) mit den nativen Backends für macOS Keychain und Windows Credential Manager; API der installierten Version in deren Doku nachsehen.
- [ ] **Step 4: Tests ausführen** – `cargo test secrets` → PASS; zusätzlich lokal `cargo test keyring_roundtrip_manual -- --ignored` → PASS
- [ ] **Step 5: Commit** – `git commit -m "feat: secret store backed by OS keychain"`

---

### Task 4: `config.yaml`-Import

**Files:**
- Create: `src/import.rs`
- Modify: `README.md` (Abschnitt `## Konfiguration`)

**Interfaces:**
- Consumes: `Settings`, `Instance`, `Topic`, `AuthKind`, `SecretStore`, `account_for`, `store::save_json`
- Produces:
  ```rust
  pub struct ImportFile { pub urgent_breaks_dnd: Option<bool>, pub instances: Vec<ImportInstance> }
  pub struct ImportInstance { pub url: String, pub token: Option<String>, pub username: Option<String>, pub password: Option<String>, pub cache_duration_secs: Option<i64>, pub topics: Vec<Topic> }
  pub fn parse_import(yaml: &str) -> anyhow::Result<ImportFile>;
  pub fn apply_import(existing: Option<Settings>, file: &ImportFile, secrets: &dyn SecretStore) -> anyhow::Result<Settings>;
  pub fn sanitized_yaml(file: &ImportFile) -> String;
  pub fn import_file(path: &Path, existing: Option<Settings>, secrets: &dyn SecretStore, settings_path: &Path) -> anyhow::Result<Settings>;
  ```

YAML-Format (so auch in die README):

```yaml
urgent_breaks_dnd: false          # optional, global
instances:
  - url: https://ntfy.example.org
    token: tk_xxxxxxxx            # ODER username + password, ODER nichts
    cache_duration: 12h           # optional, humantime; muss zur Server-Einstellung passen
    topics:
      - backups                   # Kurzform
      - name: frigate
        urgent_breaks_dnd: true   # optional, pro Topic
```

Regeln: `url` muss `http://` oder `https://` sein, abschließender `/` wird entfernt. Topic-Namen `^[-_A-Za-z0-9]{1,64}$`. `token` und `username/password` schließen sich aus; `username` ohne `password` ist ein Fehler. Instanz-IDs: Host der URL, bei Kollision `-2`, `-3` …; bestehende Instanz mit gleicher (normalisierter) URL behält ihre ID. Re-Import ersetzt Topics und Auth der betroffenen Instanz, andere Instanzen bleiben unverändert, `urgent_breaks_dnd` nur wenn in der Datei gesetzt. Ablauf von `import_file`: lesen → parsen → Secrets speichern (`apply_import`) → `save_json(settings_path)` → `<dir>/config.imported.yaml` mit `sanitized_yaml` schreiben → Original löschen. Schlägt ein Schritt vor dem Speichern der Settings fehl, bleibt die Originaldatei unverändert.

- [ ] **Step 1: Failing tests schreiben**

```rust
#[test] fn parses_short_and_long_topics() { /* Beispiel oben → 2 Topics, frigate.urgent_breaks_dnd == Some(true) */ }
#[test] fn parses_duration() { /* cache_duration: 36h → Some(129600) */ }
#[test] fn rejects_invalid_topic_name() { /* "bad topic" → Err, Meldung enthält "bad topic" */ }
#[test] fn rejects_token_and_password() { /* beides gesetzt → Err */ }
#[test] fn rejects_non_http_url() { /* "ftp://x" → Err */ }
#[test] fn apply_stores_secret_not_in_settings() {
    // MemoryStore: get(account_for(id)) == Some("tk_xxxxxxxx"); serde_json::to_string(&settings) enthält "tk_" nicht; auth == AuthKind::Token
}
#[test] fn reimport_keeps_id_and_replaces_topics() { /* gleiche URL mit trailing "/" → gleiche id, Topics ersetzt; zweite Instanz unverändert */ }
#[test] fn ids_unique_per_host() { /* zwei Instanzen gleicher Host, andere Pfade → "host" und "host-2" */ }
#[test] fn import_file_sanitizes_and_removes_original() {
    // nach import_file: config.yaml existiert nicht, config.imported.yaml existiert und enthält weder "tk_" noch "password:"; settings.json existiert
}
#[test] fn import_file_keeps_original_on_secret_failure() { /* MemoryStore{fail_on_set:true} → Err; config.yaml unverändert; settings.json nicht angelegt */ }
```

- [ ] **Step 2: Tests ausführen** → FAIL
- [ ] **Step 3: Implementieren** (`serde_yaml_ng`, `humantime`; Topic-Kurzform per `#[serde(untagged)]`-Hilfsenum)
- [ ] **Step 4: Tests ausführen** – `cargo test import` → PASS
- [ ] **Step 5: README `## Konfiguration`** – Format oben, Speicherort `default_import_file()` je Plattform, `--config <pfad>`, `--import <pfad>`, Hinweis „Secrets wandern in den Schlüsselbund, die Datei wird zu `config.imported.yaml` ohne Secrets“.
- [ ] **Step 6: Commit** – `git commit -m "feat: one-time config.yaml import with secret migration"`

---

### Task 5: ntfy-Datenmodell

**Files:**
- Create: `src/ntfy/mod.rs`, `src/ntfy/model.rs`

**Interfaces:**
- Produces:
  ```rust
  pub enum Event { Open { time: i64 }, Keepalive { time: i64 }, Message(Message), Other { event: String, time: i64 } }
  pub fn parse_event(line: &str) -> Result<Event, serde_json::Error>;
  pub struct Message { pub id: String, pub time: i64, pub topic: String, pub title: Option<String>, pub message: Option<String>,
                       pub tags: Vec<String>, pub priority: u8 /*1..=5*/, pub click: Option<String>, pub icon: Option<String>,
                       pub actions: Vec<Action>, pub attachment: Option<Attachment> }
  pub enum Action { View { label: String, url: String },
                    Http { label: String, url: String, method: String /*Default "POST"*/, headers: BTreeMap<String,String>, body: Option<String> },
                    Unsupported }
  pub struct Attachment { pub name: String, pub mime: Option<String> /*JSON-Feld "type"*/, pub size: Option<u64>, pub url: String }
  impl Attachment { pub fn is_image(&self) -> bool } // mime "image/*" oder Endung png/jpg/jpeg/gif/webp
  ```
  `Message`, `Action`, `Attachment`: `Clone, Debug, PartialEq, Serialize, Deserialize`.

- [ ] **Step 1: Failing tests schreiben**

```rust
#[test] fn parses_full_message() {
    let l = r#"{"id":"sPs71M8A2T","time":1700000000,"expires":1700043200,"event":"message","topic":"backups","title":"Backup","message":"OK","priority":4,"tags":["white_check_mark","nas"],"click":"https://x/y","icon":"https://x/i.png","actions":[{"action":"view","label":"Open","url":"https://x"},{"action":"http","label":"Ack","url":"https://x/ack"}],"attachment":{"name":"a.png","type":"image/png","size":10,"expires":1,"url":"https://x/a.png"}}"#;
    let Event::Message(m) = parse_event(l).unwrap() else { panic!() };
    assert_eq!(m.priority, 4);
    assert_eq!(m.actions[1], Action::Http { label: "Ack".into(), url: "https://x/ack".into(), method: "POST".into(), headers: BTreeMap::new(), body: None });
    assert!(m.attachment.unwrap().is_image());
}
#[test] fn parses_keepalive_and_open() { /* {"id":"x","time":5,"event":"keepalive","topic":"a,b"} → Keepalive{time:5}; analog open */ }
#[test] fn unknown_event_is_other() { /* "poll_request" → Other{event:"poll_request",..} */ }
#[test] fn priority_defaults_and_clamps() { /* fehlt → 3; 0 → 3; 9 → 5 */ }
#[test] fn broadcast_action_is_unsupported() { /* {"action":"broadcast","label":"x","extras":{"a":"b"}} → Unsupported */ }
#[test] fn unknown_fields_ignored() { /* zusätzliches Feld "foo":1 → Ok */ }
#[test] fn invalid_json_is_err() { assert!(parse_event("garbage").is_err()); }
```

- [ ] **Step 2: Tests ausführen** → FAIL
- [ ] **Step 3: Implementieren.** Ein Roh-Struct mit `event: String` deserialisieren und dann auf `Event` abbilden; Priorität per eigener Normalisierung.
- [ ] **Step 4: Tests ausführen** – `cargo test ntfy::model` → PASS
- [ ] **Step 5: Commit** – `git commit -m "feat: ntfy event and message model"`

---

### Task 6: Notification-Modell, Emoji, i18n, Rendering

**Files:**
- Create: `src/notification.rs`, `src/emoji.rs`, `src/emoji_table.rs` (generiert), `scripts/gen_emoji.py`, `src/i18n.rs`, `src/render.rs`, `THIRD_PARTY_NOTICES.md`

**Interfaces:**
- Consumes: `ntfy::model::{Message, Action, Attachment}`
- Produces:
  ```rust
  // notification.rs
  pub enum Level { Passive, Active, Urgent }            // Passive = still, Active = Ton, Urgent = Ton + zeitkritisch
  pub enum MediaRef { Remote(String), Local(PathBuf) }
  #[derive(Serialize, Deserialize)] #[serde(tag = "kind")]
  pub enum ActivationPayload { Open { url: String }, Http { label: String, method: String, url: String, headers: BTreeMap<String,String>, body: Option<String> } }
  impl ActivationPayload { pub fn encode(&self) -> String /*JSON*/; pub fn decode(s: &str) -> Option<Self> }
  pub struct Button { pub label: String, pub payload: ActivationPayload }
  pub struct Notification { pub id: String, pub thread: String, pub title: String, pub body: String, pub level: Level,
                            pub default_action: Option<ActivationPayload>, pub buttons: Vec<Button>,
                            pub image: Option<MediaRef>, pub icon: Option<MediaRef> }
  pub trait Notifier: Send + Sync { fn show(&self, n: &Notification) -> anyhow::Result<()>; }
  pub type ActivationSink = Arc<dyn Fn(ActivationPayload) + Send + Sync>;
  pub struct LogNotifier;                                   // loggt nur (Fallback)
  pub struct RecordingNotifier { pub shown: Mutex<Vec<Notification>> }  // pub, für Tests
  // emoji.rs
  pub fn lookup(alias: &str) -> Option<&'static str>;
  // i18n.rs
  pub struct Strings { pub tags_label, pub bundle_title, pub gap_title, pub gap_body, pub action_failed_title,
                       pub tray_ok, pub tray_problem, pub tray_auth_failed, pub tray_paused, pub tray_not_configured, pub menu_quit: &'static str }
  pub fn strings_for(locale: &str) -> &'static Strings;    // "de*" → DE, sonst EN
  pub fn strings() -> &'static Strings;                     // sys_locale::get_locale()
  pub fn fill(template: &str, vars: &[(&str, &str)]) -> String;  // ersetzt "{name}"
  // render.rs
  pub fn render(instance_id: &str, msg: &Message, urgent_allowed: bool, s: &Strings) -> Notification;
  ```

Texte (DE / EN): `tags_label` „Tags“ / „Tags“; `bundle_title` „{topic}: {n} neue Nachrichten“ / „{topic}: {n} new messages“; `gap_title` „Nachrichten möglicherweise verpasst“ / „Messages may have been missed“; `gap_body` „{topics} auf {instance}“ / „{topics} on {instance}“; `action_failed_title` „Aktion fehlgeschlagen: {label}“ / „Action failed: {label}“; `tray_ok` „Verbunden“ / „Connected“; `tray_problem` „Verbindungsproblem: {instance}“ / „Connection problem: {instance}“; `tray_auth_failed` „Anmeldung fehlgeschlagen: {instance}“ / „Login failed: {instance}“; `tray_paused` „Pausiert“ / „Paused“; `tray_not_configured` „Nicht eingerichtet – {path} anlegen“ / „Not set up – create {path}“; `menu_quit` „Beenden“ / „Quit“.

Render-Regeln: `id = "{instance_id}:{msg.id}"`, `thread = "{instance_id}/{topic}"`. Titel = Emojis aller bekannten Tags (Reihenfolge der Tags, mit Leerzeichen getrennt) + `" "` + (`title` oder Topic-Name). Body = `message` (oder leer); unbekannte Tags als neue Zeile `"{tags_label}: a, b"`; Nicht-Bild-Anhang als Zeile `"📎 {name}"`. Level: Priorität 1–2 → `Passive`, 3–4 → `Active`, 5 → `Urgent` nur wenn `urgent_allowed`, sonst `Active`. `default_action`: `click` → `Open`; sonst Nicht-Bild-Anhang → `Open(attachment.url)`. Bild-Anhang → `image = Remote(url)`. `icon` → `icon = Remote(url)`. Buttons: `View` → `Open`, `Http` → `Http`, `Unsupported` weglassen, höchstens 3.

- [ ] **Step 1: Emoji-Tabelle generieren.** `scripts/gen_emoji.py` lädt `https://raw.githubusercontent.com/github/gemoji/master/db/emoji.json` (Quelle der ntfy-Emoji-Zuordnung) und schreibt `src/emoji_table.rs` als `pub static EMOJI: &[(&str, &str)]`, sortiert nach Alias (alle Aliase je Emoji). Generierte Datei committen; gemoji-MIT-Lizenzhinweis in `THIRD_PARTY_NOTICES.md` und als Kopfkommentar.

- [ ] **Step 2: Failing tests schreiben**

```rust
#[test] fn emoji_lookup() { assert_eq!(lookup("warning"), Some("⚠️")); assert_eq!(lookup("+1"), Some("👍")); assert_eq!(lookup("zzz_unknown"), None); }
#[test] fn emoji_table_sorted() { assert!(EMOJI.windows(2).all(|w| w[0].0 < w[1].0)); }
#[test] fn locale_selection() { /* strings_for("de-DE").menu_quit == "Beenden"; "en-US" und "fr" → "Quit" */ }
#[test] fn title_with_emoji_and_fallback_to_topic() {
    // tags ["warning","skull","nas"], title None, topic "backups" → title "⚠️ 💀 backups", body endet mit "Tags: nas"
}
#[test] fn priority_levels() { /* 1,2 → Passive; 3,4 → Active; 5+false → Active; 5+true → Urgent; 4+true → Active */ }
#[test] fn click_and_attachments() {
    // click gesetzt → default_action Open(click); kein click + Anhang "a.pdf" → Open(url) und body enthält "📎 a.pdf"; Bild-Anhang → image Remote(url), keine 📎-Zeile
}
#[test] fn buttons_mapped_and_capped() { /* view, http, Unsupported, view, view → 3 Buttons, Unsupported fehlt */ }
#[test] fn ids_and_thread() { /* id "inst:abc", thread "inst/backups" */ }
#[test] fn payload_roundtrip() { /* encode → decode == Some(gleich); decode("x") == None */ }
```

- [ ] **Step 3: Tests ausführen** → FAIL
- [ ] **Step 4: Implementieren** (`sys-locale` hinzufügen)
- [ ] **Step 5: Tests ausführen** – `cargo test` → PASS
- [ ] **Step 6: Commit** – `git commit -m "feat: notification model, emoji tags, i18n and rendering"`

---

### Task 7: Sync-Logik (Dedupe, Catch-up, Lückenerkennung)

**Files:**
- Create: `src/sync.rs`
- Modify: `README.md` (Abschnitt `## Server-Empfehlungen`)

**Interfaces:**
- Consumes: `state::{InstanceState, SeenMessage, SEEN_RING}`, `ntfy::model::Message`
- Produces:
  ```rust
  pub const DEDUP_MARGIN_SECS: i64 = 10;
  pub fn catch_up_since(st: &InstanceState) -> Option<i64>;   // None ohne Historie; sonst min(watermark, last_message.time) - DEDUP_MARGIN_SECS
  pub fn stream_since(server_now: i64) -> i64;                 // server_now - DEDUP_MARGIN_SECS
  impl InstanceState {
      pub fn accept(&mut self, msg: &Message) -> bool;         // false bei bekannter ID oder msg.time < watermark - DEDUP_MARGIN_SECS
      pub fn observe_time(&mut self, server_time: i64);        // watermark = max(watermark, t)
  }
  pub fn detect_gap(st: &InstanceState, polled: &[Message], server_now: i64, cache_duration: i64) -> bool;
  ```

Regeln: Es werden **nur Serverzeiten** verglichen (Event-`time`, `Date`-Header), nie die lokale Uhr. `accept` legt bei `true` die ID in `seen_ids` (älteste fliegt bei > `SEEN_RING`), setzt `last_message` auf die Nachricht mit größter `time` und hebt `watermark` an. `detect_gap` wird mit dem Zustand **vor** `accept` der Poll-Ergebnisse aufgerufen und ist `true`, wenn (a) `server_now - watermark > cache_duration` oder (b) `last_message` existiert, `server_now - last_message.time < cache_duration` und keine Nachricht in `polled` diese ID hat.

- [ ] **Step 1: Failing tests schreiben**

```rust
#[test] fn no_history_no_catch_up() { assert_eq!(catch_up_since(&InstanceState::default()), None); }
#[test] fn since_uses_min_minus_margin() { /* watermark 1000, last_message.time 900 → Some(890) */ }
#[test] fn duplicate_id_rejected() { /* accept(m) true, accept(m) false */ }
#[test] fn old_message_rejected_recent_unseen_accepted() { /* watermark 1000: time 900 → false; time 995 neue ID → true */ }
#[test] fn seen_ring_capped() { /* 201 Nachrichten → seen_ids.len()==200, erste ID nicht mehr enthalten */ }
#[test] fn watermark_monotonic() { /* observe_time(100), observe_time(50) → watermark Some(100) */ }
#[test] fn last_message_is_latest() { /* accept time 10, dann time 5 → last_message.time == 10 */ }
#[test] fn gap_when_offline_longer_than_cache() { /* watermark 0, server_now 50000, cache 43200 → true */ }
#[test] fn no_gap_when_last_message_in_poll() { /* last_message (id "a", now-100), polled enthält "a" → false */ }
#[test] fn gap_when_recent_last_message_missing() { /* last_message now-100, watermark now-60, polled leer → true (Server-Neustart ohne Cache) */ }
#[test] fn no_gap_when_last_message_expired_normally() { /* last_message now-100000, watermark now-60, polled leer → false */ }
```

- [ ] **Step 2: Tests ausführen** → FAIL
- [ ] **Step 3: Implementieren**
- [ ] **Step 4: Tests ausführen** – `cargo test sync` → PASS
- [ ] **Step 5: README `## Server-Empfehlungen`** – `cache-file` setzen (sonst gehen Nachrichten bei Server-Neustart verloren), `cache-duration` ausreichend lang (z. B. `72h` für Wochenenden), in der App `cache_duration` passend zur Server-Einstellung; Erklärung des Hinweises „Nachrichten möglicherweise verpasst“.
- [ ] **Step 6: Commit** – `git commit -m "feat: dedupe, catch-up and gap detection logic"`

---

### Task 8: ntfy-HTTP-Client und Verbindungs-Task

**Files:**
- Create: `src/ntfy/client.rs`, `src/connection.rs`, `tests/connection_fake.rs`, `tests/support/mod.rs` (axum-Fake-Server), `scripts/ntfy-test-server.sh`, `tests/e2e_ntfy.rs`

**Interfaces:**
- Consumes: `Event`, `Message`, `parse_event`, `Auth`, `InstanceState`, `catch_up_since`, `stream_since`, `detect_gap`, `Instance`
- Produces:
  ```rust
  // ntfy/client.rs
  #[derive(thiserror::Error, Debug)] pub enum NtfyError { Auth(u16), Status(u16), Network(String) }
  pub struct PollResult { pub messages: Vec<Message>, pub server_now: Option<i64> /*aus Date-Header*/ }
  pub type EventStream = Pin<Box<dyn Stream<Item = Result<Event, NtfyError>> + Send>>;
  pub fn http_client() -> reqwest::Client;   // connect_timeout 10s, tcp_keepalive 30s, User-Agent "ntfy-native/<version>"
  pub fn topics_url(base: &str, topics: &[String]) -> String;   // "{base ohne /}/{t1,t2}/json"
  pub async fn poll(http: &reqwest::Client, base: &str, topics: &[String], since: i64, auth: Option<&Auth>) -> Result<PollResult, NtfyError>;   // ?poll=1&since=<since>, Timeout 30s
  pub async fn subscribe(http: &reqwest::Client, base: &str, topics: &[String], since: Option<i64>, auth: Option<&Auth>) -> Result<EventStream, NtfyError>;
  // connection.rs
  pub struct ConnectionConfig { pub instance_id: String, pub base_url: String, pub topics: Vec<String>, pub auth: Option<Auth>, pub cache_duration: i64,
                                pub keepalive_timeout: Duration, pub backoff_initial: Duration, pub backoff_max: Duration, pub auth_retry: Duration }
  impl ConnectionConfig { pub fn production(inst: &Instance, auth: Option<Auth>) -> Self }  // 120s, 1s, 30s, 60s
  #[derive(Clone, Debug, PartialEq)] pub enum ConnStatus { Connecting, Connected, Disconnected { error: String }, AuthFailed }
  pub enum ConnEvent { Status(ConnStatus), Messages { messages: Vec<Message>, catch_up: bool }, Gap, State(InstanceState) }
  #[derive(Clone)] pub struct ConnControl { pub reconnect: Arc<tokio::sync::Notify>, pub cancel: tokio_util::sync::CancellationToken }
  pub fn spawn_connection(cfg: ConnectionConfig, state: InstanceState, http: reqwest::Client,
                          tx: mpsc::UnboundedSender<(String, ConnEvent)>, ctl: ConnControl) -> tokio::task::JoinHandle<()>;
  ```

HTTP-Details: 401/403 → `NtfyError::Auth`; andere Nicht-2xx → `Status`. Token → `bearer_auth`, Basic → `basic_auth`. Stream: Bytes in Zeilen zerlegen; leere Zeilen ignorieren; Zeilen, die `parse_event` nicht versteht, mit `tracing::warn!` überspringen.

Verbindungs-Algorithmus (verbindlich):

```text
loop {
  send Status(Connecting)
  stream_since = None
  if let Some(since) = catch_up_since(&state) {
      poll(since)                                   -- Fehler → handle_error; continue
      now = poll.server_now.unwrap_or(lokale Zeit)  -- nur als Fallback
      if detect_gap(&state, &polled, now, cfg.cache_duration) { send Gap }
      accepted = polled.filter(|m| state.accept(m))
      if !accepted.is_empty() { send Messages{accepted, catch_up: true} }
      send State(state.clone())
      stream_since = Some(stream_since(now))
  }
  subscribe(stream_since)                           -- Fehler → handle_error; continue
  send Status(Connected); backoff zurücksetzen
  loop select {
      cancel                    → return
      reconnect.notified()      → break
      timeout(keepalive_timeout, stream.next()) →
          Elapsed / None / Err  → send Status(Disconnected{grund}); break
          Open                  → nichts (verschiebt die watermark NICHT)
          Keepalive{time}       → state.observe_time(time); State senden höchstens alle 60 s
          Message(m)            → if state.accept(&m) { send Messages{[m], false}; send State }
          Other                 → ignorieren
  }
  warten(backoff)  -- abbrechbar durch cancel (return) und reconnect (sofort weiter)
}
handle_error: Auth → Status(AuthFailed), warten(auth_retry); sonst Status(Disconnected{err}), warten(backoff);
backoff verdoppelt sich ab backoff_initial bis backoff_max, ±20 % Jitter
```

- [ ] **Step 1: Fake-Server bauen** (`tests/support/mod.rs`, axum als dev-dependency): konfigurierbare Antworten für `GET /:topics/json` (Poll vs. Stream anhand `poll=1`), Stream-Body als Kanal, in den der Test Zeilen schiebt oder der offen hängen bleibt; zeichnet alle Query-Strings und `Authorization`-Header auf; Statuscode pro Anfrage steuerbar.

- [ ] **Step 2: Failing tests schreiben** (`tests/connection_fake.rs`, Tuning: `keepalive_timeout` 300 ms, `backoff_initial` 50 ms, `backoff_max` 200 ms, `auth_retry` 1 s)

```rust
#[tokio::test] async fn first_connect_without_history_streams_without_since() { /* erste Anfrage ohne poll und ohne since; Live-Nachricht → Messages{catch_up:false} */ }
#[tokio::test] async fn keepalive_timeout_triggers_catch_up_poll() { /* Stream hängt nach einer Nachricht → Disconnected{..}; nächste Anfrage hat poll=1&since=<time-10> */ }
#[tokio::test] async fn duplicate_between_poll_and_stream_delivered_once() { /* gleiche ID in Poll und Stream → insgesamt 1 Message-Event */ }
#[tokio::test] async fn gap_event_on_missing_last_message() { /* Zustand mit last_message ~jetzt, Poll leer mit Date-Header → ConnEvent::Gap */ }
#[tokio::test] async fn unauthorized_reports_auth_failed_and_waits() { /* 401 → Status(AuthFailed); innerhalb 500 ms genau 1 Anfrage */ }
#[tokio::test] async fn server_down_retries_with_backoff() { /* Port ohne Server → mindestens 3 Status(Disconnected{..})-Events innerhalb 1 s, kein Panic */ }
#[tokio::test] async fn reconnect_notify_forces_new_connection() { /* ctl.reconnect.notify_one() → zweite Stream-Anfrage */ }
#[tokio::test] async fn invalid_json_line_is_skipped() { /* "garbage\n" dann gültige Nachricht → Nachricht kommt an */ }
#[tokio::test] async fn sends_auth_headers() { /* Token → "Bearer tk_x"; Basic → "Basic dTpwdw==" */ }
#[tokio::test] async fn cancel_stops_task() { /* cancel → JoinHandle endet in < 200 ms */ }
```

- [ ] **Step 3: Tests ausführen** – `cargo test --test connection_fake` → FAIL
- [ ] **Step 4: `client.rs` und `connection.rs` implementieren** (Crates: `tokio` mit `rt, macros, time, sync`, `tokio-util`, `futures-util`, `reqwest` mit `default-features = false` + `native-tls`, `stream`, `json`, `httpdate`, `thiserror`, `tracing`, `rand` für Jitter)
- [ ] **Step 5: Tests ausführen** – `cargo test --test connection_fake` → PASS
- [ ] **Step 6: E2E gegen echtes ntfy.** `scripts/ntfy-test-server.sh start|stop|restart [--no-cache]` startet `binwiederhier/ntfy serve` per Docker auf `127.0.0.1:18080` mit `--keepalive-interval 5s` und (ohne `--no-cache`) `--cache-file /var/cache/ntfy/cache.db`. `tests/e2e_ntfy.rs` läuft nur, wenn `NTFY_E2E_URL` gesetzt ist (sonst früh `return`):
  - `e2e_live_and_catch_up`: verbinden, A publizieren (`POST {url}/e2e-topic`) → A kommt; Task abbrechen; B und C publizieren; mit gespeichertem Zustand neu starten → B und C genau einmal mit `catch_up: true`, A nicht noch einmal.
  - `e2e_gap_after_restart_without_cache`: Nachricht empfangen, Task abbrechen, `scripts/ntfy-test-server.sh restart --no-cache`, neu verbinden → `ConnEvent::Gap`.

  Run: `scripts/ntfy-test-server.sh start && NTFY_E2E_URL=http://127.0.0.1:18080 cargo test --test e2e_ntfy -- --test-threads=1`
  Expected: PASS. Bestätigt nebenbei die Annahmen „`since=<unix>` funktioniert über mehrere Topics“ und „`Date`-Header vorhanden“; schlägt das fehl, Annahme im Code korrigieren und hier vermerken.

- [ ] **Step 7: Commit** – `git commit -m "feat: ntfy client and resilient per-instance connection"`

---

### Task 9: Medien laden (Bilder, Icons)

**Files:**
- Create: `src/media.rs`, `tests/media_fake.rs`

**Interfaces:**
- Consumes: `Notification`, `MediaRef`, `Auth`, Fake-Server aus `tests/support`
- Produces:
  ```rust
  pub const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024; pub const MAX_ICON_BYTES: u64 = 1024 * 1024;
  pub struct MediaFetcher { /* http, dir, timeout 10s */ }
  impl MediaFetcher {
      pub fn new(http: reqwest::Client, dir: PathBuf) -> Self;
      pub async fn fetch(&self, url: &str, file_stem: &str, max_bytes: u64, auth: Option<&Auth>) -> Option<PathBuf>;
      pub async fn resolve(&self, n: Notification, base_url: &str, auth: Option<&Auth>) -> Notification;  // Remote → Local oder None
      pub fn cleanup(&self, older_than: Duration) -> std::io::Result<usize>;
  }
  ```

Regeln: nur `http`/`https`; `Content-Type` muss `image/*` sein; Abbruch, wenn `Content-Length` oder gestreamte Bytes `max_bytes` übersteigen (keine Teildatei zurücklassen); Endung aus Content-Type (`png`, `jpg`, `gif`, `webp`); Auth-Header nur, wenn die URL mit `base_url` beginnt. `resolve` lädt Bild (`MAX_IMAGE_BYTES`, Stem `<sanitisierte id>-image`) und Icon (`MAX_ICON_BYTES`, Stem `<id>-icon`) parallel; Fehlschläge → Feld `None`, die Benachrichtigung geht trotzdem raus.

- [ ] **Step 1: Failing tests schreiben**

```rust
#[tokio::test] async fn fetches_png() { /* Datei existiert, Bytes identisch, Endung .png */ }
#[tokio::test] async fn rejects_non_image() { /* text/html → None, keine Datei */ }
#[tokio::test] async fn rejects_large_content_length() { /* Content-Length 6 MiB → None */ }
#[tokio::test] async fn rejects_oversized_stream_without_length() { /* chunked > 5 MiB → None, keine Teildatei im dir */ }
#[tokio::test] async fn rejects_file_scheme() { /* "file:///etc/passwd" → None, keine Anfrage */ }
#[tokio::test] async fn auth_only_same_origin() { /* base_url = Fake-Server → Header gesendet; andere Origin → kein Header */ }
#[tokio::test] async fn resolve_drops_failed_refs() { /* image 404 → image None, Rest unverändert */ }
#[test] fn cleanup_removes_old_files() { /* Datei mit alter mtime (filetime als dev-dep) → entfernt, neue bleibt */ }
```

- [ ] **Step 2: Tests ausführen** → FAIL
- [ ] **Step 3: Implementieren**
- [ ] **Step 4: Tests ausführen** – `cargo test --test media_fake media` → PASS
- [ ] **Step 5: Commit** – `git commit -m "feat: capped media download for notification images and icons"`

---

### Task 10: Dispatcher und Status-Aggregation

**Files:**
- Create: `src/dispatcher.rs`, `src/status.rs`

**Interfaces:**
- Consumes: `Settings`, `RecentMessage`, `GapFlag`, `RECENT_MAX`, `Message`, `render`, `Strings`, `fill`, `Notification`, `Level`, `ConnStatus`
- Produces:
  ```rust
  // dispatcher.rs
  pub const BUNDLE_THRESHOLD: usize = 3;
  pub struct Dispatcher { pub settings: Settings, pub recent: VecDeque<RecentMessage>, pub gaps: Vec<GapFlag> /* + strings */ }
  impl Dispatcher {
      pub fn new(settings: Settings, recent: VecDeque<RecentMessage>, gaps: Vec<GapFlag>, strings: &'static Strings) -> Self;
      pub fn on_messages(&mut self, instance_id: &str, msgs: &[Message], catch_up: bool, now: i64) -> Vec<Notification>;
      pub fn on_gap(&mut self, instance_id: &str, now: i64) -> Option<Notification>;
      pub fn pause_all(&mut self, until: Option<i64>);           // None = fortsetzen, Some(i64::MAX) = unbegrenzt
      pub fn set_topic_muted(&mut self, instance_id: &str, topic: &str, muted: bool) -> bool;  // true wenn geändert
      pub fn is_paused(&mut self, now: i64) -> bool;              // abgelaufene Pause wird gelöscht
  }
  // status.rs
  pub const PROBLEM_GRACE_SECS: i64 = 15;
  #[derive(Clone, Debug, PartialEq)] pub enum TrayState { Ok, Problem { instance: String }, AuthFailed { instance: String }, Paused, NotConfigured }
  pub fn aggregate(settings: &Settings, statuses: &BTreeMap<String, (ConnStatus, i64 /*seit*/)>, paused: bool, now: i64) -> TrayState;
  ```
  `now` ist hier lokale Unix-Zeit (Pausen und Schonfrist sind lokale Konzepte).

Regeln: Jede Nachricht kommt vorne in `recent` (max. `RECENT_MAX`, neueste zuerst). Benachrichtigt wird nicht, wenn das Topic `muted` ist oder `is_paused(now)`. Bei `catch_up == true` und mehr als `BUNDLE_THRESHOLD` Nachrichten eines Topics → eine Sammel-Benachrichtigung: `id = "{inst}:bundle:{topic}:{neueste id}"`, Titel `bundle_title`, Body = Titel + Zeilenumbruch + Text der neuesten Nachricht, Level = höchstes Level der gebündelten Nachrichten. Live-Nachrichten werden nie gebündelt. `on_gap`: `GapFlag` für **alle** Topics der Instanz anlegen (bestehende Flags für dasselbe Topic ersetzen); Benachrichtigung (Level `Active`, `gap_title`, `gap_body` mit nicht stummgeschalteten Topics durch „, “ getrennt und `Instance::host()`), unterdrückt bei globaler Pause oder wenn alle Topics stumm sind. `aggregate`-Vorrang: `NotConfigured` (keine Instanz mit Topics) > `AuthFailed` (sofort) > `Problem` (nicht `Connected` seit ≥ `PROBLEM_GRACE_SECS`) > `Paused` > `Ok`; Label = `Instance::host()`.

- [ ] **Step 1: Failing tests schreiben**

```rust
#[test] fn message_notifies_and_is_recent() { /* 1 Notification, recent[0].id == msg.id */ }
#[test] fn muted_topic_recorded_not_notified() { /* 0 Notifications, recent.len()==1 */ }
#[test] fn pause_until_suppresses_then_expires() { /* pause_all(Some(now+60)): bei now → 0; bei now+61 → 1 und settings.paused_until == None */ }
#[test] fn pause_indefinitely() { /* Some(i64::MAX) → bei now+10^9 noch pausiert */ }
#[test] fn catch_up_bundles_above_threshold() { /* 4× Topic a + 2× Topic b, catch_up → 3 Notifications (1 Bündel a, 2× b); recent.len()==6 */ }
#[test] fn live_burst_not_bundled() { /* 5 Nachrichten, catch_up false → 5 */ }
#[test] fn recent_capped_newest_first() { /* 25 Nachrichten → len 20, recent[0] ist die neueste */ }
#[test] fn urgent_respects_overrides() { /* Prio 5: global false + Topic Some(true) → Urgent; global true + Topic Some(false) → Active */ }
#[test] fn gap_flags_and_notification() { /* 2 Topics, eins stumm → 2 GapFlags, Body nennt nur das nicht stumme Topic */ }
#[test] fn gap_suppressed_when_paused_but_flagged() { /* pausiert → None, Flags trotzdem angelegt */ }
#[test] fn aggregate_not_configured() { /* keine Instanzen → NotConfigured */ }
#[test] fn aggregate_grace_period() { /* Connecting seit now-10 → Ok; seit now-20 → Problem{instance:"ntfy.example.org"} */ }
#[test] fn aggregate_auth_failed_immediate_and_precedence() { /* AuthFailed seit now → AuthFailed, auch wenn pausiert */ }
#[test] fn aggregate_paused() { /* alles Connected + paused → Paused */ }
```

- [ ] **Step 2: Tests ausführen** → FAIL
- [ ] **Step 3: Implementieren**
- [ ] **Step 4: Tests ausführen** – `cargo test dispatcher status` → PASS
- [ ] **Step 5: Commit** – `git commit -m "feat: dispatcher with pause, bundling, recent list and tray status"`

---

### Task 11: Aktionen ausführen (URL öffnen, HTTP-Action)

**Files:**
- Create: `src/actions.rs`, `tests/actions_fake.rs`

**Interfaces:**
- Consumes: `ActivationPayload`
- Produces:
  ```rust
  pub trait UrlOpener: Send + Sync { fn open(&self, url: &str) -> anyhow::Result<()>; }
  pub struct SystemOpener;                                    // open::that_detached
  pub struct RecordingOpener { pub opened: Mutex<Vec<String>> } // pub, für Tests
  pub fn is_openable(url: &str) -> bool;                      // nur http, https, mailto
  pub async fn perform(http: &reqwest::Client, payload: &ActivationPayload, opener: &dyn UrlOpener) -> anyhow::Result<()>;
  ```
  HTTP-Action: Methode/Header/Body aus Payload, Timeout 15 s, Nicht-2xx → `Err`. HTTP-Actions dürfen nur `http`/`https`-URLs ansprechen.

- [ ] **Step 1: Failing tests schreiben**

```rust
#[tokio::test] async fn open_https_calls_opener() { /* Open{"https://x"} → opened == ["https://x"] */ }
#[tokio::test] async fn open_rejects_file_and_javascript() { /* "file:///etc/passwd", "javascript:alert(1)" → Err, opened leer */ }
#[tokio::test] async fn http_action_sends_request() { /* Fake-Server sieht PUT, Header "X-A: 1", Body "b" */ }
#[tokio::test] async fn http_action_error_status() { /* 500 → Err */ }
#[test] fn openable_schemes() { /* http, https, mailto → true; ftp, file, javascript, "ntfy://x" → false */ }
```

- [ ] **Step 2: Tests ausführen** → FAIL
- [ ] **Step 3: Implementieren** (Crate `open`)
- [ ] **Step 4: Tests ausführen** – `cargo test --test actions_fake actions` → PASS
- [ ] **Step 5: Commit** – `git commit -m "feat: safe execution of click and http actions"`

---

### Task 12: Engine-Schleife und Reconnect-Auslöser (Aufwachen, Netzwechsel)

**Files:**
- Create: `src/engine.rs`, `src/triggers.rs`, `tests/engine_fake.rs`

**Interfaces:**
- Consumes: alles aus Task 2–11
- Produces:
  ```rust
  // engine.rs
  #[derive(Debug)] pub enum Command { Activated(ActivationPayload), ReconnectAll, PauseAll(Option<i64>),
                                      SetTopicMuted { instance_id: String, topic: String, muted: bool }, Quit }
  pub struct EngineDeps { pub paths: AppPaths, pub settings: Settings, pub state: State, pub secrets: Arc<dyn SecretStore>,
                          pub notifier: Arc<dyn Notifier>, pub opener: Arc<dyn UrlOpener>, pub strings: &'static Strings,
                          pub on_tray: Box<dyn Fn(TrayState) + Send>, pub on_exit: Box<dyn FnOnce() + Send>,
                          pub tune: fn(&mut ConnectionConfig) /* Produktion: |_| {} */ , pub enable_triggers: bool }
  #[derive(Clone)] pub struct EngineHandle { pub tx: mpsc::UnboundedSender<Command> }
  pub async fn run_engine(deps: EngineDeps, tx: mpsc::UnboundedSender<Command>, rx: mpsc::UnboundedReceiver<Command>) -> anyhow::Result<()>;
  pub fn spawn_engine_thread(deps: EngineDeps) -> (EngineHandle, std::thread::JoinHandle<()>);   // tokio current_thread runtime
  // triggers.rs
  pub fn is_wake_jump(prev: SystemTime, now: SystemTime, tick: Duration, threshold: Duration) -> bool;  // now - prev > tick + threshold
  pub fn spawn_wake_detector(tx: mpsc::UnboundedSender<Command>) -> tokio::task::JoinHandle<()>;       // tick 5 s, threshold 30 s
  pub fn spawn_network_watcher(tx: mpsc::UnboundedSender<Command>) -> tokio::task::JoinHandle<()>;     // if-watch; erste 5 s ignorieren; 2 s Debounce
  ```

Ablauf `run_engine`: Medien-Cache aufräumen (älter als 1 Tag); für jede Instanz mit Topics `resolve_auth` (Fehler → Status `AuthFailed` für diese Instanz, Fehler loggen) und `spawn_connection` mit `ConnectionConfig::production` + `tune`; bei `enable_triggers` Wake- und Netzwerk-Watcher starten (bekommen eine Kopie von `tx`). `select!` über Verbindungs-Events, Commands und einen 5-s-Ticker:
- `Messages` → `Dispatcher::on_messages` → für jede Notification `MediaFetcher::resolve` (Auth der Instanz) → `notifier.show` (Fehler loggen); `Gap` → `on_gap` → `show`; `State` → `state.instances` ersetzen, als „dirty“ markieren; `Status` → `statuses` aktualisieren (Zeitstempel nur bei Statuswechsel).
- Nach jedem Event und jedem Tick `aggregate` berechnen; `on_tray` nur bei Änderung aufrufen.
- `Activated` → `actions::perform`; Fehler → Passive-Notification `action_failed_title` mit Fehlertext als Body.
- `ReconnectAll` → `notify_one()` auf allen `ConnControl.reconnect`.
- `PauseAll`/`SetTopicMuted` → Dispatcher; bei Änderung `settings.json` sofort speichern.
- Ticker: Pausenablauf prüfen; `state.json` speichern, wenn dirty (höchstens alle 5 s; `recent` und `gaps` aus dem Dispatcher übernehmen).
- `Quit` (und Ctrl-C/SIGTERM über `tokio::signal`) → alle `cancel`, Settings und State speichern, `on_exit()` aufrufen, zurückkehren.

- [ ] **Step 1: Failing tests schreiben**

```rust
#[test] fn wake_jump_detection() { /* prev→now +5 s → false; +40 s → true */ }
#[tokio::test] async fn message_reaches_notifier_once() { /* Fake-ntfy, RecordingNotifier: 1 Nachricht → shown.len()==1 */ }
#[tokio::test] async fn muted_via_command_not_shown() { /* SetTopicMuted → nächste Nachricht nicht gezeigt; settings.json hat muted=true */ }
#[tokio::test] async fn quit_persists_state_and_calls_exit() { /* Quit → state.json enthält watermark/seen id; on_exit aufgerufen */ }
#[tokio::test] async fn restart_with_saved_state_does_not_renotify() {
    // Engine 1 empfängt Nachricht X, Quit; Engine 2 mit geladenem state.json, Fake-Poll liefert X erneut → X wird nicht erneut gezeigt
}
#[tokio::test] async fn activated_open_uses_opener() { /* Command::Activated(Open{"https://x"}) → RecordingOpener.opened == ["https://x"] */ }
#[tokio::test] async fn tray_state_reported() { /* nach Connect → on_tray(TrayState::Ok); Server 401 → AuthFailed{instance} */ }
```

- [ ] **Step 2: Tests ausführen** → FAIL
- [ ] **Step 3: Implementieren** (`if-watch` mit tokio-Feature, `tokio` + `signal`)
- [ ] **Step 4: Tests ausführen** – `cargo test --test engine_fake triggers` → PASS
- [ ] **Step 5: Commit** – `git commit -m "feat: engine loop with persistence and wake/network reconnect"`

---

### Task 13: macOS-Notifier und Entwicklungs-App-Bundle

**Files:**
- Create: `src/notify/mod.rs`, `src/notify/macos.rs`, `packaging/macos/Info.plist`, `packaging/macos/ntfy-native.entitlements`, `scripts/macos-dev-bundle.sh`, `examples/notify_demo.rs`, `tests/fixtures/sample.png` (kleines PNG < 50 KB)
- Modify: `README.md` (`## Entwicklung`)

**Interfaces:**
- Consumes: `Notification`, `Notifier`, `ActivationSink`, `ActivationPayload`, `Level`, `MediaRef`
- Produces:
  - `notify::platform_notifier(on_activate: ActivationSink) -> Arc<dyn Notifier>` – macOS: `MacNotifier`, Windows: `WinNotifier` (Task 14), sonst bzw. bei Fehler `LogNotifier` (mit `warn!`)
  - `pub struct MacNotifier`; `MacNotifier::new(on_activate: ActivationSink) -> anyhow::Result<Self>`
  - `pub fn category_id(labels: &[&str]) -> String` (plattformunabhängig, FNV-1a-Hash hex, Präfix `"btn-"`), getestet

Umsetzung (`objc2`, `objc2-foundation`, `objc2-user-notifications`, `block2`; nur `cfg(target_os = "macos")`):
- `new`: Fehler, wenn `NSBundle.mainBundle.bundleIdentifier` fehlt („App muss aus einem Bundle laufen – `scripts/macos-dev-bundle.sh`“). Delegate-Klasse (`define_class!`) setzen, **bevor** die Ereignisschleife läuft. `willPresentNotification` → Banner + Liste + Ton. `didReceiveNotificationResponse` → `actionIdentifier` == Default → `userInfo["default"]`, `"a0"`…`"a2"` → `userInfo["b0"]`…; String mit `ActivationPayload::decode` an `on_activate`; Completion-Handler immer aufrufen. `requestAuthorization(alert | sound)`, Ergebnis loggen.
- `show`: `UNMutableNotificationContent` mit Titel, Body, `threadIdentifier = n.thread`, Ton `defaultSound` außer bei `Passive`; `interruptionLevel`: `Passive` → passive, `Active` → active, `Urgent` → timeSensitive. Bei Buttons: Kategorie `category_id(labels)` mit `UNNotificationAction` `"a0"`… registrieren (alle bisher registrierten Kategorien behalten, max. 32 in einem LRU-Set, `setNotificationCategories` mit dem ganzen Set). `MediaRef::Local`-Bild als `UNNotificationAttachment`. Icon wird auf macOS ignoriert (das OS erlaubt kein Icon pro Benachrichtigung). Request-ID = `n.id`.

Bundle (`scripts/macos-dev-bundle.sh`): `cargo build --release`, `target/ntfy-native.app/Contents/{MacOS/ntfy-native, Info.plist}` anlegen, signieren mit `codesign --force --options runtime --entitlements packaging/macos/ntfy-native.entitlements --sign "${MACOS_SIGN_IDENTITY:--}"`. `Info.plist`: `CFBundleIdentifier` `dev.hemp0r.ntfy-native`, `CFBundleName`/`CFBundleExecutable` `ntfy-native`, `CFBundleShortVersionString` aus `Cargo.toml`, `LSUIElement` true, `LSMinimumSystemVersion` 15.0. Entitlements: `com.apple.developer.usernotifications.time-sensitive` = true. Funktioniert die zeitkritische Stufe mit der jeweiligen Signatur nicht, fällt das OS auf „active“ zurück – im README vermerken; endgültige Signatur/Provisioning ist Teil 3.

- [ ] **Step 1: Failing test schreiben** – `category_id_stable`: `category_id(&["Open","Ack"]) == category_id(&["Open","Ack"])`, `!= category_id(&["Ack","Open"])`, beginnt mit `"btn-"`
- [ ] **Step 2: Test ausführen** → FAIL
- [ ] **Step 3: `notify/mod.rs` mit `category_id` und `platform_notifier`, `MacNotifier` implementieren**
- [ ] **Step 4: Tests + Build** – `cargo test notify && cargo clippy --all-targets -- -D warnings` → PASS
- [ ] **Step 5: Manuelle Prüfung** mit einer Hilfs-Binary `examples/notify_demo.rs` (zeigt je eine Notification pro Level, mit Buttons, mit Bild aus `tests/fixtures/sample.png`, druckt Aktivierungen auf stdout; Bundle-Skript akzeptiert `--example notify_demo`). Erwartet: drei Banner; `Passive` ohne Ton; Klick auf Banner und auf Buttons druckt die Payload; bei aktivem Fokus „Nicht stören“ erscheint nur die `Urgent`-Benachrichtigung, und nur, wenn zeitkritische Mitteilungen in den Systemeinstellungen erlaubt sind. Ergebnis im Commit-Text festhalten.
- [ ] **Step 6: README `## Entwicklung`** – Bundle-Skript, `MACOS_SIGN_IDENTITY`, warum ein Bundle nötig ist.
- [ ] **Step 7: Commit** – `git commit -m "feat: native macOS notifications via UNUserNotificationCenter"`

---

### Task 14: Windows-Notifier (WinRT-Toasts, AUMID, COM-Aktivierung)

**Files:**
- Create: `src/notify/toast_xml.rs` (plattformunabhängig, getestet), `src/notify/windows.rs`
- Modify: `src/notify/mod.rs`, `src/main.rs` (`#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]`)

**Interfaces:**
- Consumes: `Notification`, `Notifier`, `ActivationSink`, `ActivationPayload`, `Level`, `MediaRef`, `APP_ID`, `APP_NAME`
- Produces:
  - `pub const TOAST_ACTIVATOR_CLSID: &str = "AE88E831-148B-400C-8273-6ABC4FDEA102";` (fest, nie ändern)
  - `toast_xml::build(n: &Notification, urgent_scenario: bool) -> String`
  - `toast_xml::escape(s: &str) -> String`
  - `pub struct WinNotifier`; `WinNotifier::new(on_activate: ActivationSink) -> anyhow::Result<Self>`

XML-Format von `build`:

```xml
<toast launch="{escape(default_action.encode())}"[ scenario="urgent"]>
  <visual><binding template="ToastGeneric">
    <text>{title}</text><text>{body}</text>
    [<image placement="appLogoOverride" src="file:///{icon}"/>]
    [<image src="file:///{image}"/>]
  </binding></visual>
  [<audio silent="true"/>]                                   <!-- bei Passive -->
  [<actions><action content="{label}" arguments="{payload}" activationType="foreground"/>…</actions>]
</toast>
```
`scenario="urgent"` nur bei `Level::Urgent && urgent_scenario`; nur `MediaRef::Local` wird eingebettet; ohne `default_action` ist `launch` leer.

Umsetzung (`windows`-Crate; Features u. a. `UI_Notifications`, `Data_Xml_Dom`, `Win32_UI_Shell`, `Win32_System_Com`, `Win32_System_Registry`, `implement`; nur `cfg(windows)`):
- `new`: unter `HKCU\Software\Classes\AppUserModelId\dev.hemp0r.ntfy-native` die Werte `DisplayName` = `ntfy-native` und `CustomActivator` = `{CLSID}` setzen; unter `HKCU\Software\Classes\CLSID\{CLSID}\LocalServer32` den Standardwert `"<exe-Pfad>" -ToastActivated`. `SetCurrentProcessExplicitAppUserModelID(APP_ID)`. COM initialisieren und `CoRegisterClassObject(CLSID, factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE)` mit einer Factory, die `INotificationActivationCallback` implementiert. `Activate(_, invokedArgs, …)` → `ActivationPayload::decode(invokedArgs)` → `on_activate`.
- `show`: `XmlDocument::LoadXml(build(n, true))`, `ToastNotification`, `Tag` und `Group` = gekürzte Hashes von `n.id`/`n.thread` (≤ 64 Zeichen), `ToastNotificationManager::CreateToastNotifierWithId(APP_ID).Show`. Schlägt `Show` mit `scenario` fehl (älteres Windows 10), einmal mit `build(n, false)` wiederholen.
- Wird die App durch einen Toast-Klick neu gestartet, kommt sie mit `-ToastActivated`/`-Embedding` hoch (in Task 1 bereits ignoriert). Die Aktivierung kommt nach `CoRegisterClassObject` über denselben Callback.

- [ ] **Step 1: Failing tests schreiben** (in `toast_xml.rs`, läuft auf allen Plattformen)

```rust
#[test] fn escapes_xml() { assert_eq!(escape(r#"<a&"b'>"#), "&lt;a&amp;&quot;b&apos;&gt;"); }
#[test] fn passive_is_silent() { /* Level::Passive → enthält <audio silent="true"/> */ }
#[test] fn urgent_scenario_only_when_allowed() { /* Urgent + true → scenario="urgent"; Urgent + false → kein scenario; Active + true → kein scenario */ }
#[test] fn buttons_carry_encoded_payload() { /* 2 Buttons → 2 <action>, arguments = escape(payload.encode()) */ }
#[test] fn only_local_media_embedded() { /* Remote-Bild → kein <image>; Local → src="file:///…" */ }
#[test] fn output_is_well_formed() { /* Titel mit "<&>" → mit quick-xml (dev-dep) fehlerfrei parsebar */ }
```

- [ ] **Step 2: Tests ausführen** → FAIL
- [ ] **Step 3: `toast_xml.rs` implementieren** → `cargo test toast_xml` → PASS
- [ ] **Step 4: `WinNotifier` implementieren.** Build-Prüfung: CI-Job `windows-latest` (Push auf einen Branch) oder lokal `cargo check --target x86_64-pc-windows-msvc` (rustup nötig – siehe Global Constraints). Expected: grün, `clippy -D warnings` sauber.
- [ ] **Step 5: Manuelle Prüfung auf dem Windows-Rechner des Auftraggebers** mit `examples/notify_demo.rs` (Task 13): Toasts erscheinen mit Name „ntfy-native“; `Passive` ohne Ton; Buttons und Klick drucken die Payload – auch nach dem Verschieben ins Info-Center und bei beendeter App (Neustart über COM); „Nicht stören“ unterdrückt alles, außer die App steht auf der Prioritätsliste und `Urgent` ist aktiv. **Erscheinen unter Windows 10 keine Toasts**, als Fallback eine Startmenü-Verknüpfung mit `System.AppUserModel.ID` und `System.AppUserModel.ToastActivatorCLSID` anlegen (gehört dann auch in den Installer in Teil 3) und das hier vermerken.
- [ ] **Step 6: Commit** – `git commit -m "feat: native Windows toasts with COM activation"`

---

### Task 15: App-Shell – Tray, Single-Instance, Logging, Verdrahtung

**Files:**
- Create: `src/app.rs`, `src/tray.rs`, `src/icons.rs`, `src/logging.rs`, `src/instance_lock.rs`
- Modify: `src/main.rs`, `README.md`

**Interfaces:**
- Consumes: `CliAction`, `AppPaths`, `load_json`, `Loaded`, `Settings`, `State`, `import_file`, `KeyringStore`, `platform_notifier`, `SystemOpener`, `spawn_engine_thread`, `EngineDeps`, `Command`, `TrayState`, `strings`, `fill`
- Produces:
  - `pub enum IconKind { Ok, Problem, Paused, NotConfigured }`; `icons::rgba(kind: IconKind, template: bool) -> (Vec<u8>, u32, u32)` – 32×32 RGBA, prozedural: `Ok` gefüllter Kreis, `Problem` Ring mit gefülltem Punkt unten rechts, `Paused` zwei senkrechte Balken, `NotConfigured` Ring. `template = true` (macOS): Schwarz mit Alpha; sonst Farben `Ok` #2A8A3A, `Problem` #C62828, `Paused`/`NotConfigured` #888888.
  - `instance_lock::acquire(path: &Path) -> anyhow::Result<Option<File>>` – `None`, wenn eine andere Instanz läuft (`File::try_lock`)
  - `logging::init(paths: &AppPaths)` – `tracing-subscriber` + `tracing-appender` (täglich rotierend in `log_dir`, 7 Dateien), in Debug-Builds zusätzlich stderr; Level per `NTFY_NATIVE_LOG`, Standard `info`
  - `app::run(cli: Cli) -> anyhow::Result<()>`
  - `enum UserEvent { Tray(TrayState), Activated(ActivationPayload), Exit }` (intern in `app.rs`)

Ablauf `app::run`:
1. `AppPaths::platform()`, `ensure_dirs`, `logging::init`; `instance_lock::acquire` → `None` ⇒ Info loggen und mit 0 beenden.
2. Settings laden: `Loaded::Ok` → nutzen. `--import` gesetzt → `import_file(pfad, vorhandene Settings)` (Re-Import). Sonst bei `Missing`/`Corrupt`: `--config` oder `default_import_file()` vorhanden → `import_file`, sonst „nicht eingerichtet“ (leere Settings). Import-Fehler → loggen und als nicht eingerichtet weiter, Datei bleibt liegen.
3. State laden (`Missing`/`Corrupt` → `State::default()`, Warnung).
4. `tao::EventLoop::<UserEvent>::with_user_event()`; Proxy klonen. `platform_notifier` mit `ActivationSink`, der `UserEvent::Activated` an den Proxy schickt (macOS-Delegate vor dem Start der Schleife registrieren).
5. Tray (`tray-icon`): Icon nach `TrayState`; Menü: deaktivierter Statuseintrag (`tray_ok`, `fill(tray_problem, instance)`, `fill(tray_auth_failed, instance)`, `tray_paused`, `fill(tray_not_configured, path)`), Trenner, `menu_quit`. Auf macOS das Tray-Icon erst bei `StartCause::Init` anlegen (Vorgabe von `tray-icon`).
6. `spawn_engine_thread` mit `on_tray` → `UserEvent::Tray`, `on_exit` → `UserEvent::Exit`, `enable_triggers: true`, `tune: |_| {}`.
7. Schleife: `UserEvent::Tray` → Icon und Statustext aktualisieren; `Activated` → `Command::Activated` an die Engine; Menü „Beenden“ → `Command::Quit`; `UserEvent::Exit` → Schleife beenden, Engine-Thread joinen.

- [ ] **Step 1: Failing tests schreiben**

```rust
#[test] fn icon_dimensions() { let (px, w, h) = rgba(IconKind::Ok, true); assert_eq!((w, h, px.len()), (32, 32, 32 * 32 * 4)); }
#[test] fn ok_icon_center_opaque_problem_center_transparent() { /* Alpha des Mittelpixels: Ok 255, Problem 0 */ }
#[test] fn second_lock_fails() { /* acquire zweimal auf denselben Pfad → Some, dann None */ }
```

- [ ] **Step 2: Tests ausführen** → FAIL
- [ ] **Step 3: Implementieren** (`tao`, `tray-icon`, `tracing-subscriber`, `tracing-appender`)
- [ ] **Step 4: Tests + Lint** – `cargo test && cargo clippy --all-targets -- -D warnings` → PASS
- [ ] **Step 5: Manueller End-to-End-Test macOS.** Test-Server starten, `config.yaml` mit zwei Instanzen (`http://127.0.0.1:18080` und `http://localhost:18080`, zusammen 10 Topics) nach `default_import_file()` legen, Bundle bauen und starten. Erwartet:
  - Tray „Verbunden“; `config.yaml` ist zu `config.imported.yaml` geworden.
  - `curl -d "Hallo" -H "Title: Test" -H "Tags: warning" -H "Click: https://ntfy.sh" http://127.0.0.1:18080/<topic>` → Banner „⚠️ Test“, Klick öffnet den Browser.
  - Docker-Container stoppen → nach ≤ 15 s Tray „Verbindungsproblem“. Wieder starten und dabei 5 Nachrichten publizieren → eine Sammel-Benachrichtigung.
  - Rechner 2 Minuten schlafen legen, dabei publizieren → Nachricht kommt nach dem Aufwachen.
  - Zweiter Start der App → beendet sich sofort.
- [ ] **Step 6: Speicher-Check.** Nach 1 Minute Ruhe: `footprint -p $(pgrep -x ntfy-native)` → Zeile `phys_footprint`. Expected: **< 30 MB**. Wert im Commit-Text und in der README notieren. Liegt er darüber: Ursache suchen (z. B. Größe des reqwest-Connection-Pools, Logging-Puffer) und den Auftraggeber informieren, bevor es weitergeht.
- [ ] **Step 7: Manueller End-to-End-Test Windows** auf dem Rechner des Auftraggebers (Release-Build aus der CI oder lokal): dieselben Schritte wie Step 5 ohne Schlaftest bzw. mit Energiesparmodus; Speicher im Task-Manager (privater Arbeitssatz) < 30 MB.
- [ ] **Step 8: README** – Abschnitt „Starten“ (Bundle bzw. `.exe`, Konfiguration über `config.yaml` bis zum Wizard in Teil 2, Logs unter `log_dir`, `NTFY_NATIVE_LOG`).
- [ ] **Step 9: Commit** – `git commit -m "feat: tray app shell wiring engine, notifier and import"`
