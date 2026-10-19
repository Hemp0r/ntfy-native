# Release Notes aus Commits und Autostart – Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Automatische Releases bekommen ihre Version aus den Commit-Typen (feat → Minor, sonst Patch) und gruppierte Release Notes; ntfy-native kann beim Anmelden automatisch starten (Option im Erst-Assistenten, Schalter in den Einstellungen).

**Architecture:** Versions- und Notes-Logik als kleine, mit Testdaten geprüfte Bash-Skripte (Muster: `scripts/next-version.sh`), eingebunden in `version.yml`/`release.yml`; dazu ein PR-Titel-Check. Autostart als Kern-Modul `src/autostart.rs` hinter einem Trait (macOS: `SMAppService.mainApp` mit LaunchAgent-Rückfall; Windows: `HKCU\…\Run` + `StartupApproved`), Zustand im UI-Snapshot, Umschalten per UI-Anfrage an die Engine.

**Tech Stack:** Bash + GitHub Actions; Rust, objc2-service-management (macOS), windows-Crate (Registry), bestehendes IPC/egui aus Teil 2.

**Spec:** `docs/superpowers/specs/2026-10-19-release-notes-autostart-design.md` (Hauptspec: `docs/superpowers/specs/2026-10-18-ntfy-native-design.md`, 5.7).

## Global Constraints

- Release-Regeln (Spec 4.1): mind. ein `feat` seit letztem Tag → `X.(Y+1).0`; sonst `X.Y.(Z+1)`; Breaking (`!` oder `BREAKING CHANGE:`/`BREAKING-CHANGE:`-Zeile) → vor 1.0 Minor, ab 1.0 Major; kein Tag → Cargo-Version; Cargo-Version > letzter Tag → Cargo-Version; Merge-Commits und `chore(release):` zählen nicht.
- Release Notes (Spec 4.2/4.5): Gruppen in dieser Reihenfolge, leere weglassen: `### Features` (feat), `### Fehlerbehebungen` (fix), `### Sonstiges` (alles andere, auch ohne Typ). Eintrag = erste Commit-Zeile ohne Typ-Präfix, Scope fett vorangestellt (`**import:** …`), Breaking mit `⚠️ Breaking: ` davor, Kurz-SHA (7) als Link `https://github.com/<repo>/commit/<sha>`. Abschluss `Alle Änderungen: https://github.com/<repo>/compare/vVORHER...vNEU` nur wenn es einen Vorgänger gibt. Bereich: nächstälterer `vX.Y.Z`-Tag bis zum veröffentlichten Tag. `generate_release_notes` entfällt.
- Ein Release darf nie an einem nicht-konformen Commit scheitern (→ „Sonstiges“).
- Autostart (Spec 4.3/4.4/4.5): Zustand gehört dem OS, kein eigener Settings-Wert; nie ungefragt einschalten; im OS deaktiviert → „aus“ anzeigen, nur durch ausdrückliches Einschalten im Schalter wieder aktivieren; Reparatur (Pfad) nur wenn aktiv.
- Autostart-Option im Assistenten nur im eigenständigen Assistenten-Fenster (`UiMode::Wizard`, = Ersteinrichtung), Standard an, angewendet nach erfolgreichem Speichern; Fehler/Bestätigung nötig → Assistent bleibt auf der Abschlussseite mit Hinweis.
- Alle sichtbaren Texte über `src/i18n.rs` (DE/EN). Windows-Ziel muss bauen (`PATH="$HOME/.rustup/toolchains/stable-x86_64-apple-darwin/bin:$PATH" cargo clippy --all-targets --target x86_64-pc-windows-msvc -- -D warnings`). Keine Abhängigkeiten ohne OSI-Lizenz/mit kommerzieller Doppellizenz.
- Kern blockiert nie auf die UI; Autostart-Aufrufe sind kurz (Registry/SMAppService) und laufen im Kern.

## Review Focus

1. **Exe-Pfad mit Leerzeichen/Umlauten** (z. B. `C:\Users\Jörg Ä\Downloads\ntfy native\ntfy-native.exe`) → Run-Wert korrekt gequotet und als „aktiviert“ erkannt. Test: Task 3 (`run_value_quotes_path`, `run_value_matches_current_exe`).
2. **Im Task-Manager bzw. in den Anmeldeobjekten deaktiviert** → Schalter „aus“, Reparatur beim Start schaltet nicht ein. Test: Task 3 (`repair_never_enables_user_disabled`), Task 4 (`snapshot_reports_disabled_by_user`).
3. **Unsaubere Commit-Nachrichten** (ohne Typ, Großschreibung `Feat:`, Scope mit Bindestrich, mehrzeilig, `BREAKING CHANGE` im Text, `Revert "…"`) → sinnvolle Einordnung, Skript bricht nie ab. Test: Task 1 (`classify_*`), Task 2 (`notes_*`).
4. **Erstes Release / altes Tag neu ausführen / bewusst gesetztes 1.0.0** → richtiger Bereich, Vergleichslink nur mit Vorgänger. Test: Task 2 (`prev_tag_*`, `notes_without_previous_tag`).
5. **macOS ohne Bundle oder ohne ordentliche Signatur** → kein Absturz, LaunchAgent-Rückfall bzw. „nicht verfügbar“. Test: Task 3 (`mac_mechanism_choice`).

---

### Task 1: Commit-Einordnung und Versionssprung

**Files:**
- Create: `scripts/commits-lib.sh` (gemeinsame Parser-Funktionen, wird per `source` genutzt), `scripts/bump-kind.sh`, `scripts/test-bump-kind.sh`
- Modify: `scripts/next-version.sh`, `scripts/test-next-version.sh`, `.github/workflows/version.yml`, `.github/workflows/ci.yml` (neue Testskripte im Job `scripts`)

**Interfaces:**
- Produces:
  - Eingabeformat für Commits (stdin, von Task 2 wiederverwendet): Datensätze `"<sha>\x1f<vollständige Nachricht>\x1e"`, erzeugt mit `git log --no-merges --format='%H%x1f%B%x1e' <bereich>`.
  - `commits-lib.sh`: `parse_subject <erste-zeile>` setzt `CC_TYPE` (klein geschrieben, leer wenn kein Schema), `CC_SCOPE`, `CC_BREAKING` (0/1), `CC_DESC`; `is_breaking_body <nachricht>` → Exit 0, wenn eine Zeile mit `BREAKING CHANGE:` oder `BREAKING-CHANGE:` beginnt; `is_release_commit <erste-zeile>` → Exit 0 für `chore(release):`. Schema: `^([A-Za-z]+)(\(([^)]+)\))?(!)?: (.+)$`.
  - `bump-kind.sh <aktuelle-version>` (Commits auf stdin) → druckt `major|minor|patch`.
  - `next-version.sh <cargo-version> <kind>` (Tags auf stdin) → wie bisher `"<version> <bump|keep>"`, Sprung nach `kind`.

- [ ] **Step 1: Failing tests** (`scripts/test-bump-kind.sh` im Stil von `test-next-version.sh`; Commit-Fixtures per `printf 'a1\x1ffeat: x\x1e'`)
  - `classify_feat_is_minor`: `feat: x` → `minor` (Version 0.1.2)
  - `classify_scoped_feat`: `feat(ui): x` → `minor`
  - `classify_uppercase_type`: `Feat: x` → `minor`
  - `classify_fix_only_is_patch`: `fix: a`, `fix(import): b` → `patch`
  - `classify_other_only_is_patch`: `docs: a`, `ci: b`, `Update README` → `patch`
  - `classify_breaking_bang_pre_1`: `fix!: x`, Version 0.4.0 → `minor`
  - `classify_breaking_body_post_1`: `refactor: x\n\nBREAKING CHANGE: y`, Version 1.2.3 → `major`
  - `classify_release_commit_ignored`: nur `chore(release): v0.1.2` → `patch`
  - `classify_empty_range`: keine Commits → `patch`
  - `next-version.sh`-Fälle anpassen/ergänzen: `0.1.2` + Tag `v0.1.2` + `minor` → `0.2.0 bump`; `+ patch` → `0.1.3 bump`; `1.2.3` + Tag `v1.2.3` + `major` → `2.0.0 bump`; Cargo höher als Tag bleibt `keep` unabhängig vom kind; ungültiges kind → Exit 2.
- [ ] **Step 2: Run** `scripts/test-bump-kind.sh; scripts/test-next-version.sh` → FAIL
- [ ] **Step 3: Implementieren.** `version.yml`: letzten `vX.Y.Z`-Tag ermitteln (numerisch, wie in `next-version.sh`), Commits `git log --no-merges --format='%H%x1f%B%x1e' "$latest..HEAD"` (ohne Tag: `HEAD`) → `bump-kind.sh "$latest_or_cargo"` → `next-version.sh "$current" "$kind"`. Die bestehende „schon veröffentlicht“-Sperre bleibt. `ci.yml`: Job `scripts` ruft beide Testskripte auf.
- [ ] **Step 4: Run** Testskripte → PASS; `docker run --rm -v "$PWD":/repo -w /repo rhysd/actionlint:latest -color` und `docker run --rm -v "$PWD":/mnt -w /mnt koalaman/shellcheck:stable scripts/*.sh` → sauber
- [ ] **Step 5: Commit** `feat(release): bump minor for features, patch otherwise`

---

### Task 2: Release Notes, Vorgänger-Tag und PR-Titel-Check

**Files:**
- Create: `scripts/prev-tag.sh`, `scripts/release-notes.sh`, `scripts/check-commit-subject.sh`, `scripts/test-release-notes.sh`, `.github/workflows/pr-title.yml`
- Modify: `.github/workflows/release.yml` (Job `release`), `.github/workflows/ci.yml` (neues Testskript)

**Interfaces:**
- Consumes: `commits-lib.sh`, Commit-Eingabeformat aus Task 1
- Produces:
  - `prev-tag.sh <tag>` (Tags auf stdin) → druckt den numerisch nächstälteren `vX.Y.Z`-Tag oder nichts.
  - `release-notes.sh <repo> <tag> [<prev-tag>]` (Commits auf stdin) → Markdown auf stdout gemäß Global Constraints.
  - `check-commit-subject.sh <titel>` → Exit 0, wenn der Titel dem Schema entspricht und der Typ in `feat fix docs ci test refactor perf build style chore revert` liegt; sonst Exit 1 mit Meldung.

- [ ] **Step 1: Failing tests** (`scripts/test-release-notes.sh`)
  - `prev_tag_numeric`: Tags `v0.1.9 v0.1.10 v0.2.0`, Eingabe `v0.2.0` → `v0.1.10`
  - `prev_tag_first_release`: `v0.1.0` → leer
  - `prev_tag_ignores_non_semver`: `v1.0.0-rc1` wird übersprungen
  - `notes_grouped_in_order`: je ein feat/fix/docs-Commit → Ausgabe enthält genau in dieser Reihenfolge `### Features`, `### Fehlerbehebungen`, `### Sonstiges`
  - `notes_skip_empty_groups`: nur fix → keine `### Features`-Zeile
  - `notes_scope_and_link`: `a1b2c3d4…\x1ffix(import): robust redaction` → Zeile `- **import:** robust redaction ([a1b2c3d](https://github.com/Hemp0r/ntfy-native/commit/a1b2c3d4…))`
  - `notes_breaking_mark`: `feat!: drop x` → beginnt mit `⚠️ Breaking: `
  - `notes_subject_only`: mehrzeilige Nachricht → nur erste Zeile
  - `notes_untyped_goes_to_other`: `Revert "feat: x"` und `Update README` → unter Sonstiges, unverändert
  - `notes_excludes_release_commit`: `chore(release): v0.1.2` erscheint nicht
  - `notes_compare_link`: mit Vorgänger → letzte Zeile `Alle Änderungen: https://github.com/Hemp0r/ntfy-native/compare/v0.1.2...v0.2.0`
  - `notes_without_previous_tag`: ohne Vorgänger kein Vergleichslink
  - `notes_empty_range`: keine Commits → Zeile `Keine Änderungen.` (statt leerer Notes)
  - `subject_check`: `feat(ui): x` ok; `Feat: x` ok (Typ case-insensitive); `added stuff` fehlschlagend; `feat:x` (ohne Leerzeichen) fehlschlagend; `wip: x` fehlschlagend
- [ ] **Step 2: Run** → FAIL
- [ ] **Step 3: Implementieren.** `release.yml` Job `release`: `actions/checkout@v4` mit `fetch-depth: 0` und `ref: ${{ inputs.tag }}`; `prev=$(git tag --list 'v*.*.*' | scripts/prev-tag.sh "$TAG")`; Commits `git log --no-merges --format='%H%x1f%B%x1e' "${prev:+$prev..}$TAG"`; `scripts/release-notes.sh "$GITHUB_REPOSITORY" "$TAG" "$prev" > notes.md`; `softprops/action-gh-release` mit `body_path: notes.md`, ohne `generate_release_notes`. Werte nur über `env:`, kein `${{ }}` in `run:`. `pr-title.yml`: `on: pull_request: types: [opened, edited, synchronize, reopened]`, `permissions: contents: read`, Titel über `env: TITLE: ${{ github.event.pull_request.title }}` an `check-commit-subject.sh`.
- [ ] **Step 4: Run** Testskripte → PASS; actionlint + shellcheck (Docker) → sauber; Probe: `git log --no-merges --format='%H%x1f%B%x1e' v0.1.1..v0.1.2 | scripts/release-notes.sh Hemp0r/ntfy-native v0.1.2 v0.1.1` → sinnvolle Notes (Ausgabe im Report)
- [ ] **Step 5: Commit** `feat(release): grouped release notes from commits, PR title check`

---

### Task 3: Autostart-Modul (Kern, ohne UI)

**Files:**
- Create: `src/autostart/mod.rs`, `src/autostart/windows.rs`, `src/autostart/macos.rs`
- Modify: `src/lib.rs`, `Cargo.toml` (macOS: `objc2-service-management` mit nötigen Features; Windows: vorhandenes `windows`-Crate), `src/app.rs` (Reparatur beim Start)

**Interfaces:**
- Produces:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
  pub enum AutostartState { Enabled, Disabled, DisabledByUser, NeedsApproval, Unavailable }
  pub trait Autostart: Send + Sync {
      fn state(&self) -> AutostartState;
      fn set(&self, enabled: bool) -> anyhow::Result<AutostartState>; // Ergebnis = Zustand danach
      fn repair(&self) -> anyhow::Result<()>; // nur wenn Enabled und Ziel ≠ aktuelle Programmdatei
  }
  pub fn platform_autostart(program: PathBuf) -> Arc<dyn Autostart>;   // sonst/Fehler: Unavailable-Implementierung
  pub struct MemoryAutostart { /* Mutex<AutostartState>, fail_next: bool */ }   // pub, für Tests
  // windows.rs (reine Helfer, plattformunabhängig kompiliert und getestet)
  pub fn run_value(program: &Path) -> String;                      // "\"C:\\…\\ntfy-native.exe\""
  pub fn run_value_targets(value: &str, program: &Path) -> bool;    // Quotes/Groß-Kleinschreibung tolerant
  pub fn startup_approved_disabled(bytes: &[u8]) -> bool;          // erstes Byte ungerade = deaktiviert
  // macos.rs (reine Helfer, plattformunabhängig kompiliert und getestet)
  pub fn launch_agent_plist(label: &str, program: &Path) -> String; // RunAtLoad true, ProgramArguments [program]
  pub enum SmStatus { Enabled, NotRegistered, NotFound, RequiresApproval }
  pub enum Mechanism { ServiceManagement, LaunchAgent }
  pub fn choose_mechanism(in_bundle: bool, register: Result<SmStatus, String>) -> (Mechanism, AutostartState);
  ```
Windows: Wert `ntfy-native` unter `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`; Zustand: kein Wert → `Disabled`; Wert vorhanden, aber `StartupApproved\Run\ntfy-native` deaktiviert → `DisabledByUser`; sonst `Enabled`. `set(true)` schreibt Wert und setzt einen vorhandenen `StartupApproved`-Eintrag auf aktiviert (`02 00 00 00 00 00 00 00 00 00 00 00`) — ausdrückliches Einschalten. `set(false)` löscht den Wert (Fehlen ist ok). macOS: Label `dev.hemp0r.ntfy-native`, Plist-Pfad `~/Library/LaunchAgents/dev.hemp0r.ntfy-native.plist`, Programm = `std::env::current_exe()` im Bundle. `state()`: SMAppService-Status (`Enabled` → Enabled, `RequiresApproval` → NeedsApproval) oder vorhandene Plist → Enabled; ohne Bundle → Unavailable. `set(true)`: `register` versuchen; schlägt es fehl (Signatur, `NotFound`) → Plist schreiben; `set(false)`: `unregister` (Fehler ignorieren, wenn nicht registriert) und Plist löschen. Reparatur ändert nie `Disabled`/`DisabledByUser`/`NeedsApproval`. App-Start (`src/app.rs`): `platform_autostart(current_exe)` einmal anlegen, `repair()` aufrufen (Fehler → warn), als `Arc` an die Engine (Task 4).

- [ ] **Step 1: Failing tests**
```rust
#[test] fn run_value_quotes_path() { /* C:\Users\Jörg Ä\Downloads\ntfy native\ntfy-native.exe → mit Anführungszeichen */ }
#[test] fn run_value_matches_current_exe() { /* mit/ohne Quotes, andere Groß-Klein-Schreibung → true; anderer Pfad → false */ }
#[test] fn startup_approved_bytes() { /* [0x02,..] → false; [0x03,..] → true; [] → false */ }
#[test] fn launch_agent_plist_is_valid() { /* enthält Label, RunAtLoad true, Programm-Pfad XML-escaped (& < >) */ }
#[test] fn mac_mechanism_choice() { /* !in_bundle → (_, Unavailable); Ok(Enabled) → (ServiceManagement, Enabled); Ok(RequiresApproval) → NeedsApproval; Err(..) → (LaunchAgent, Enabled) */ }
#[test] fn repair_never_enables_user_disabled() { /* MemoryAutostart in DisabledByUser / Disabled: repair() lässt Zustand unverändert */ }
#[test] fn memory_autostart_set_and_fail() {}
```
- [ ] **Step 2: Run** → FAIL · **Step 3: Implementieren** · **Step 4:** `cargo test autostart`, Windows-clippy → PASS
- [ ] **Step 5: Manuell (macOS, Dev-Bundle):** `scripts/macos-dev-bundle.sh`, kleiner Aufruf per Test-Hook oder `examples/autostart_probe.rs` (`--enable`/`--disable`/`--state`): ad-hoc-signiert → LaunchAgent-Plist entsteht, Zustand Enabled; `--disable` entfernt sie. Danach aufräumen. Ergebnis im Report.
- [ ] **Step 6: Commit** `feat(autostart): login item / Run key with LaunchAgent fallback`

---

### Task 4: Engine, Snapshot und IPC

**Files:**
- Modify: `src/snapshot.rs`, `src/ipc.rs`, `src/engine.rs`, `src/app.rs`, `tests/engine_fake.rs`

**Interfaces:**
- Consumes: `Autostart`, `AutostartState`, `MemoryAutostart`
- Produces:
  - `UiSnapshot` + `pub autostart: AutostartState`
  - `UiRequest::SetAutostart { request_id: u64, enabled: bool }`
  - `CoreMsg::AutostartResult { request_id: u64, result: Result<AutostartState, String> }`
  - `EngineDeps` + `pub autostart: Arc<dyn Autostart>`

Verhalten: Zustand beim Start und bei jedem 5-s-Tick neu lesen (Änderungen im OS erscheinen so im Snapshot); `SetAutostart` → `set()`, Zustand übernehmen, `AutostartResult` senden (Fehlertext = äußerste Meldung, ohne Pfade aus dem Nutzerverzeichnis), Snapshot aktualisieren.

- [ ] **Step 1: Failing tests**
```rust
#[tokio::test] async fn snapshot_carries_autostart_state() {}
#[tokio::test] async fn set_autostart_reports_result_and_updates_snapshot() {}
#[tokio::test] async fn set_autostart_failure_is_reported() { /* MemoryAutostart fail_next → Err, Snapshot unverändert */ }
#[tokio::test] async fn snapshot_reports_disabled_by_user() { /* Zustand extern auf DisabledByUser → nach Tick im Snapshot */ }
```
- [ ] **Step 2–4:** RED → implementieren → `cargo test --test engine_fake`, `cargo test`, Windows-clippy PASS
- [ ] **Step 5: Commit** `feat(autostart): engine state, snapshot and UI request`

---

### Task 5: Oberfläche – Assistent und Einstellungen

**Files:**
- Modify: `src/ui/wizard.rs`, `src/ui/settings_view.rs`, `src/ui/app.rs`, `src/i18n.rs`, `README.md`

**Interfaces:**
- Consumes: `AutostartState`, `UiRequest::SetAutostart`, `CoreMsg::AutostartResult`
- Produces:
  - `Wizard::first_run() -> Wizard` (wie `new()`, plus `offer_autostart: true`, `autostart_choice: true`); `UiMode::Wizard` nutzt `first_run()`, eingebettete Assistenten in den Einstellungen nutzen `new()`/`edit()` (keine Option).
  - `Wizard::on_save_result` liefert bei Erfolg zusätzlich `SetAutostart{enabled: autostart_choice}`, falls `offer_autostart` (nur wenn `true` gewählt oder der aktuelle Zustand `Enabled` ist und `false` gewählt wurde – sonst nichts senden).
  - `Wizard::on_autostart_result(request_id, &Result<AutostartState, String>) -> bool` (true = Fenster darf schließen: Ergebnis `Enabled`/`Disabled`; bei `NeedsApproval`/`Err` bleibt `Done` mit Hinweis).

Neue UiStrings (DE / EN): `autostart` „Beim Anmelden automatisch starten“ / „Start automatically at login“; `autostart_needs_approval` „macOS verlangt eine Bestätigung: Systemeinstellungen › Allgemein › Anmeldeobjekte“ / „macOS needs your approval: System Settings › General › Login Items“; `autostart_disabled_by_user` „Im Task-Manager deaktiviert – zum Aktivieren hier einschalten“ / „Disabled in Task Manager – switch on here to enable“; `autostart_unavailable` „Nur in der installierten App verfügbar“ / „Only available in the installed app“; `autostart_failed` „Autostart konnte nicht eingerichtet werden: {reason}“ / „Could not set up autostart: {reason}“; `close` „Schließen“ / „Close“.

Assistent: Schritt „Benachrichtigungen“ zeigt bei `offer_autostart` die Checkbox (Standard an). Nach „Fertig“: Speichern → bei Ok `SetAutostart` → bei `Enabled`/`Disabled` schließen; sonst Abschlussseite mit Hinweis und Knopf „Schließen“. Einstellungen: Schalter mit aktuellem Zustand aus dem Snapshot (`DisabledByUser` → aus + Hinweis; `NeedsApproval` → an mit Hinweis; `Unavailable` → deaktiviert mit Hinweis); Umschalten sendet `SetAutostart`, Fehler aus `AutostartResult` inline. README (Deutsch): Abschnitt „Autostart“ (Assistent, Einstellungen, wo man ihn im OS sieht/abschaltet, LaunchAgent-Rückfall auf macOS, Entfernen) und Abschnitt „Versionen und Release Notes“ (Regeln, PR-Titel-Schema).

- [ ] **Step 1: Failing tests** (Logik + egui_kittest)
```rust
#[test] fn first_run_offers_autostart_default_on() {}
#[test] fn settings_wizard_does_not_offer_autostart() { /* Wizard::new()/edit() → offer_autostart false, keine Checkbox */ }
#[test] fn autostart_sent_only_after_successful_save() { /* Err bei SaveResult → kein SetAutostart */ }
#[test] fn autostart_unchecked_sends_nothing_when_off() {}
#[test] fn needs_approval_keeps_done_page_with_hint() {}
#[test] fn settings_toggle_reflects_disabled_by_user_and_sends_request() {}
#[test] fn settings_toggle_disabled_when_unavailable() {}
#[test] fn ui_strings_complete_in_both_languages() { /* bestehender Test deckt neue Felder ab */ }
```
- [ ] **Step 2–4:** RED → implementieren → `cargo test`, fmt, clippy, Windows-clippy PASS
- [ ] **Step 5: Commit** `feat(autostart): offer autostart in first-run wizard and settings`

---

### Task 6: Prüfliste ergänzen

**Files:** Modify: `README.md` (Abschnitt „Manuelle Prüfung Windows“)

- [ ] **Step 1:** Schritte ergänzen: Erstassistent zeigt Autostart (an) → nach Ab-/Anmelden startet ntfy-native; Task-Manager › Autostart zeigt „ntfy-native“, dort deaktivieren → Einstellungen zeigen „aus“ mit Hinweis, Neustart der App schaltet nicht wieder ein; Schalter in Einstellungen ein → aktiv; `.exe` verschieben und starten → Eintrag zeigt auf neuen Pfad. macOS: Anmeldeobjekte zeigen ntfy-native (signiert) bzw. LaunchAgent (ad-hoc), Abschalten in den Systemeinstellungen spiegelt sich wider. Release: nach dem Merge erscheint v0.2.0 mit gruppierten Notes.
- [ ] **Step 2: Commit** `docs: manual checks for autostart and release notes`
