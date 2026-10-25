# Release Notes aus Commits und Autostart – Design

Stand: 2026-10-19 · Basis: `main` nach v0.1.2 · gehört zu ntfy-native (Spec `2026-10-18-ntfy-native-design.md`, dort 5.7 Autostart)

## 1. Absicht und Warum

Zwei kleine, unabhängige Erweiterungen auf dem Weg zu 1.0:

1. **Lesbare Release Notes und Versionen nach Bedeutung.** Bisher erhöht jeder Merge auf `main` die letzte Versionsstelle, und die Release Notes sind GitHubs automatische PR-Liste. Der Auftraggeber möchte im Release sehen, *was* sich geändert hat – gruppiert nach Features und Fixes – und dass ein neues Feature auch eine neue Minor-Version bekommt.
2. **Autostart.** ntfy-native soll beim Anmelden automatisch starten können. Gefragt wird beim ersten Start im Einrichtungsassistenten (es gibt keinen Installer – `.dmg` und Windows-Zip); später umschaltbar in den Einstellungen.

## 2. Ziele, Nicht-Ziele, Anti-Ziele

**Ziele**
- Version des automatischen Releases nach Commit-Typen seit dem letzten Release (Conventional Commits).
- Release Notes mit Gruppen **Features**, **Fixes**, **Sonstiges**, je Eintrag Beschreibung + Link auf den Commit.
- Autostart beim Anmelden für macOS und Windows: Option im Assistenten (Standard an), Schalter in den Einstellungen, der den **tatsächlichen** OS-Zustand zeigt.

**Nicht-Ziele**
- Kein Installer (MSI/pkg), keine rechnerweite Installation (Spec 4.6).
- Keine Changelog-Datei im Repo (nur Release Notes auf GitHub).
- Keine Übersetzung der Release Notes (Commit-Texte bleiben englisch).

**Anti-Ziele**
- Autostart wird bei bestehenden Installationen **nie ungefragt eingeschaltet**.
- Ein im Betriebssystem abgeschalteter Autostart darf von ntfy-native nicht still wieder eingeschaltet werden.
- Ein Release darf nicht scheitern, nur weil ein Commit nicht dem Schema folgt.

## 3. Vom Auftraggeber entschieden

- Release Notes: Liste der Änderungen, **gruppiert nach Features und Fixes**.
- Versionierung: **`feat` erhöht die mittlere Stelle** (Patch → 0), **`fix` erhöht die letzte Stelle**.
- Autostart: als **Option**, abgefragt **im Einrichtungsassistenten beim ersten Start**.
- Bisheriges Prinzip bleibt: jeder Merge auf `main` erzeugt automatisch ein Release (wie seit v0.1.0).

## 4. Vom Designer entschieden (im Gespräch vorgeschlagen, vom Auftraggeber mit „passt so“ bestätigt)

### 4.1 Versionsregeln
Maßgeblich sind alle Commits seit dem letzten Release-Tag `vX.Y.Z` bis `HEAD` (ohne Merge-Commits und ohne `chore(release):`):
- mindestens ein `feat` (auch mit Scope `feat(x):`) → `X.(Y+1).0`
- sonst → `X.Y.(Z+1)` (Fixes, aber auch nur Doku/CI/Tests/Aufräumen/unbekannte Typen)
- `feat!:` / `fix!:` bzw. `BREAKING CHANGE:` im Commit-Text → vor 1.0 wie ein Feature (Minor); ab 1.0 → Major (`(X+1).0.0`).
- Bestehende Regeln bleiben: kein Tag → Cargo-Version unverändert; Cargo-Version höher als der letzte Tag → genau diese Version (so setzt der Auftraggeber `1.0.0` bewusst); Commit schon in einem Release-Tag → Abbruch (Schutz gegen „Re-run all jobs“).

### 4.2 Release Notes
- Gruppen in dieser Reihenfolge, leere Gruppen weglassen: **Features** (`feat`), **Fixes** (`fix`), **Sonstiges** (alle anderen Typen und Commits ohne erkennbaren Typ).
- Eintrag: Beschreibung ohne Typ-Präfix (Scope bleibt sichtbar, z. B. „**import:** robust value redaction …“), Kurz-SHA als Link auf den Commit; Breaking Changes mit Markierung „⚠️ Breaking“.
- Ausgeschlossen: Merge-Commits, `chore(release): …`.
- Am Ende: Link „Alle Änderungen“ auf den GitHub-Vergleich `vVORHER...vNEU`.
- Überschriften englisch (Features / Fixes / Other) oder deutsch? → **Deutsch** („Features“, „Fehlerbehebungen“, „Sonstiges“), passend zur README; Einträge bleiben Commit-Text (englisch).
- Die bisherige GitHub-Automatik (`generate_release_notes`) entfällt.

### 4.3 Autostart – Verhalten
- **Assistent:** Im Schritt „Benachrichtigungen“ (letzter Schritt vor „Fertig“) eine Option „Beim Anmelden automatisch starten“, **standardmäßig an** (Spec 5.7). Angewendet wird sie erst bei „Fertig“ zusammen mit dem Speichern (wie der Urgent-Schalter seit Teil 2).
- Der Assistent fragt nur beim **ersten Einrichten** (keine Instanz vorhanden). Beim Hinzufügen weiterer Server über die Einstellungen erscheint die Option nicht.
- **Einstellungen:** Schalter „Beim Anmelden automatisch starten“, zeigt den tatsächlichen OS-Zustand; Umschalten wirkt sofort.
- **Bestehende Installationen** (Settings schon vorhanden): kein Assistent → Autostart bleibt aus, bis der Nutzer ihn einschaltet.
- **Zustand gehört dem Betriebssystem:** ntfy-native speichert keinen eigenen „Autostart an“-Wert. Hat der Nutzer den Autostart im OS deaktiviert (macOS Anmeldeobjekte, Windows Task-Manager › Autostart), zeigt der Schalter „aus“ und ntfy-native schaltet ihn nicht wieder ein.
- **Verschobene App** (Windows-Zip an anderen Ort entpackt): ist der Autostart aktiv und zeigt auf einen anderen Pfad als die laufende `.exe`, korrigiert ntfy-native den Eintrag beim Start (das ist kein Einschalten, sondern Reparieren).
- **Fehler** beim Ein-/Ausschalten (z. B. fehlende Berechtigung, macOS verlangt Bestätigung): klare Meldung in der UI, Logeintrag; der Schalter zeigt danach wieder den tatsächlichen Zustand.

### 4.4 Autostart – Mechanik
- **macOS:** offizielle Anmeldeobjekt-Schnittstelle `SMAppService.mainApp` (ab macOS 13; Ziel ist 15+). Status `enabled` / `notRegistered` / `notFound` / `requiresApproval`. Bei `requiresApproval` Hinweis, dass der Nutzer es in Systemeinstellungen › Allgemein › Anmeldeobjekte erlauben muss. Funktioniert nur, wenn die App als Bundle läuft; ohne Bundle (Entwicklung per `cargo run`) ist der Schalter deaktiviert mit kurzem Hinweis.
- **Windows:** Wert `ntfy-native` unter `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` mit `"<Pfad zur exe>"`. Deaktivierung durch den Nutzer erkennt ntfy-native am Eintrag unter `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run` (erstes Byte ungerade = deaktiviert) und behandelt sie als „aus“.
- Kern (nicht der UI-Prozess) führt Lesen/Setzen aus; die UI bekommt den Zustand im Snapshot und schickt Anfragen (wie alle anderen Einstellungen in Teil 2).

### 4.5 (Designer-Entscheidung nach Builder-Check) Präzisierungen
- **PR-Merges:** Bisher werden PRs mit Merge-Commit gemergt; die einzelnen `feat:`/`fix:`-Commits liegen dann im Bereich und zählen, Merge-Commits werden ignoriert. Damit auch Squash-Merges funktionieren (dann ist der PR-Titel die Commit-Zeile), prüft ein CI-Check, dass PR-Titel dem Schema `typ(scope)!: beschreibung` folgen.
- **Bereich der Release Notes:** vom nächstälteren `vX.Y.Z`-Tag (numerisch) bis zum veröffentlichten Tag – auch beim manuellen Neu-Ausführen eines alten Tags und im „keep“-Fall (z. B. bewusst gesetztes `1.0.0`). Gibt es keinen älteren Tag: alle Commits bis dahin, ohne Vergleichslink.
- **Breaking Changes:** erkannt an `!` vor dem Doppelpunkt oder an einer Zeile `BREAKING CHANGE:` bzw. `BREAKING-CHANGE:` irgendwo im Commit-Text. In den Notes steht nur die erste Commit-Zeile.
- **Skripte:** Commit-Texte werden den Skripten als Eingabe übergeben (nicht von ihnen selbst per `git` gelesen), damit sie mit Testdaten geprüft werden können. Die „Commit schon veröffentlicht“-Sperre bleibt im Workflow.
- **macOS-Mechanik mit Rückfall:** Primär `SMAppService.mainApp`. Schlägt die Registrierung fehl, weil die App nicht ordentlich signiert ist (z. B. ad-hoc-signierter Release ohne Apple-Secrets), nutzt ntfy-native stattdessen einen LaunchAgent (`~/Library/LaunchAgents/dev.hemp0r.ntfy-native.plist`, startet das Bundle-Programm beim Anmelden). Der Schalter zeigt „an“, wenn einer der beiden Wege aktiv ist; Ausschalten entfernt beide. Gestartet wird die App beim Anmelden ganz normal (nur Tray, kein Fenster – der Assistent erscheint nur ohne Konfiguration).
- **Erstes Einrichten:** Die Autostart-Option erscheint, wenn der Assistent automatisch beim Start ohne Konfiguration geöffnet wurde (bzw. über „Einrichten …“ ohne Instanzen) – nicht bei „Server hinzufügen“ in den Einstellungen.
- **Fehler beim Autostart nach „Fertig“:** nicht fatal. Der Server wird gespeichert; gelingt der Autostart nicht oder verlangt macOS eine Bestätigung, bleibt der Assistent auf der Abschlussseite und zeigt den Hinweis (statt sich sofort zu schließen). Speichern und Autostart sind zwei getrennte Anfragen.

## 5. Rahmenbedingungen

- Rust, bestehende Architektur aus Teil 1/2 (Kern = Engine, UI = eigener Prozess, IPC über stdin/stdout, Snapshot).
- Versions- und Notes-Logik als testbare Skripte (wie `scripts/next-version.sh`), in der CI getestet; GitHub Actions wie bisher.
- Alle sichtbaren Texte über `src/i18n.rs` (DE/EN).
- Windows-Ziel muss bauen; macOS-only/Windows-only Code hinter `cfg`.
- Keine Abhängigkeiten mit nicht-OSI-Lizenz oder kommerzieller Doppellizenz (SignPath-Bedingung).

## 6. Dem Builder überlassen

- Ob die Release-Notes-Erzeugung ein Shell-Skript, ein kleines Rust-`xtask` oder ein Skript in einer anderen vorhandenen Sprache ist (Shell bevorzugt, da `next-version.sh` es vormacht).
- Exakte Markdown-Formatierung der Notes innerhalb der Regeln aus 4.2.
- Wie der Autostart-Zustand im Snapshot/IPC modelliert wird (z. B. `autostart: AutostartView { Enabled, Disabled, NeedsApproval, Unavailable }`) und welche UiRequests dazukommen.
- Rust-Bindings für `SMAppService` (objc2-service-management o. ä.) und für die Registry (vorhandenes `windows`-Crate).

## 7. Erfolg

- Nach dem Merge dieses Features erscheint automatisch **v0.2.0** (weil es `feat`-Commits enthält) mit Release Notes in den Gruppen Features / Fehlerbehebungen / Sonstiges.
- Ein späterer Merge nur mit Fixes ergibt **v0.2.1**.
- Neuinstallation: Assistent bietet Autostart an (an); nach Abmelden/Anmelden startet ntfy-native. In den Einstellungen lässt er sich aus- und wieder einschalten; der Schalter spiegelt Änderungen im OS wider.
