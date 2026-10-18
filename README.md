# ntfy-native

Schlanke Tray-App für macOS und Windows, die ntfy-Server abonniert und jede Nachricht als native Betriebssystem-Benachrichtigung zustellt. Verpasste Nachrichten werden nachgeholt bzw. Lücken gemeldet; das Tray-Icon zeigt den Verbindungsstatus.

## Starten

- macOS: `scripts/macos-dev-bundle.sh` baut `target/ntfy-native.app`; per Doppelklick oder `open target/ntfy-native.app` starten. Die App erscheint nur in der Menüleiste (kein Dock-Symbol).
- Windows: `ntfy-native.exe` starten; die App erscheint im Infobereich der Taskleiste.

Bis zum Einrichtungsassistenten (Teil 2) erfolgt die Einrichtung über eine `config.yaml` (siehe [Konfiguration](#konfiguration)): Datei am angegebenen Ort anlegen und die App (neu) starten. Eine vorhandene `config.yaml` wird beim Start nur importiert, solange noch keine Instanz eingerichtet ist; danach nur noch explizit mit `--import <pfad>`. Ohne Konfiguration zeigt das Tray-Menü „Nicht eingerichtet“ mit dem erwarteten Pfad. Das Tray-Menü zeigt den Verbindungsstatus und enthält „Beenden“.

Es läuft immer nur eine Instanz; ein zweiter Start beendet sich sofort (mit `--import` als Fehler, siehe [Konfiguration](#konfiguration)).

Logs liegen in `log_dir`, eine Datei pro Tag, die letzten 7 werden behalten:

- macOS: `~/Library/Application Support/dev.hemp0r.ntfy-native/logs/`
- Windows: `%APPDATA%\hemp0r\ntfy-native\data\logs\`

Die Ausführlichkeit steuert die Umgebungsvariable `NTFY_NATIVE_LOG` (Standard `info`; Syntax wie `RUST_LOG`, z. B. `debug` oder `info,ntfy_native=debug`). Debug-Builds schreiben zusätzlich auf stderr.

Speicherbedarf (macOS, Intel, Release-Build, zwei Instanzen mit zusammen 10 Topics, nach 1 Minute Ruhe): `phys_footprint` rund 12 MB (gemessen: 12 124 160 Byte, Ziel < 30 MB).

## Konfiguration

`config.yaml` ist ein einmaliger Import. Danach ist der interne App-Zustand (`settings.json`) die einzige Quelle der Wahrheit.

```yaml
urgent_breaks_dnd: false          # optional, global
instances:
  - url: https://ntfy.example.org
    token: tk_xxxxxxxx            # ODER username + password, ODER auth: none, ODER nichts
    cache_duration: 12h           # optional, humantime; muss zur Server-Einstellung passen
    topics:
      - backups                   # Kurzform
      - name: frigate
        urgent_breaks_dnd: true   # optional, pro Topic
```

Regeln:

- `url` muss mit `http://` oder `https://` beginnen. Die URL wird normalisiert (Hostname klein, Standard-Port wie `:443` entfernt, kein abschließender `/`), sodass `https://NTFY.example.org:443/` und `https://ntfy.example.org` dieselbe Instanz sind.
- Topic-Namen: 1 bis 64 Zeichen aus `A-Z a-z 0-9 - _`.
- `token`, `username`/`password` und `auth: none` schließen sich gegenseitig aus; `password` braucht `username`.
- Ein erneuter Import ersetzt Topics der betroffenen Instanz (erkannt an der URL); andere Instanzen bleiben unverändert. `urgent_breaks_dnd` ändert sich nur, wenn es in der Datei steht.
- Zugangsdaten bei einem erneuten Import: `token` bzw. `username` + `password` ersetzen die gespeicherten. Ohne Zugangsdaten-Felder bleiben die gespeicherten erhalten; nur `username` (ohne `password`) behält das gespeicherte Passwort einer Basic-Auth-Instanz, solange der Benutzername unverändert ist; bei geändertem Benutzernamen ist ein `password` Pflicht. `auth: none` entfernt gespeicherte Zugangsdaten.

Speicherort der Datei (`config.yaml`):

- macOS: `~/Library/Application Support/dev.hemp0r.ntfy-native/config.yaml`
- Windows: `%APPDATA%\hemp0r\ntfy-native\config\config.yaml`

Ein anderer Pfad lässt sich mit `--config <pfad>` angeben; `--import <pfad>` importiert eine Datei explizit. **App vor `--import` beenden:** Läuft ntfy-native bereits, bricht `--import` mit einer Fehlermeldung (stderr und Log) und einem Exit-Code ungleich 0 ab, ohne etwas zu importieren.

Secrets wandern in den Schlüsselbund (macOS) bzw. die Anmeldeinformationsverwaltung (Windows), die Datei wird zu `config.imported.yaml` ohne Secrets umgeschrieben (Benutzername bleibt erhalten, ein Kommentar am Anfang erklärt das) und das Original gelöscht. `config.imported.yaml` kann bearbeitet und mit `--import` erneut importiert werden; sie wird dann an Ort und Stelle neu geschrieben, die gespeicherten Zugangsdaten bleiben erhalten.

## Server-Empfehlungen

ntfy-native holt verpasste Nachrichten beim Start und nach Verbindungsabbrüchen vom Server-Cache nach. Das setzt einen passend konfigurierten ntfy-Server voraus:

- `cache-file` setzen (z. B. `/var/cache/ntfy/cache.db`). Das ist Voraussetzung für die Garantie, keine Nachrichten zu verpassen: Ohne Cache-Datei liegt der Cache nur im Speicher, und ein Server-Neustart kann Nachrichten verlieren, ohne dass die App das bemerken kann, sobald keine aktuelle Nachricht der Instanz mehr im Server-Cache liegt.
- `cache-duration` ausreichend lang wählen, z. B. `72h`, damit auch ein Wochenende ohne laufende App überbrückt wird. Der Standardwert beträgt 12 Stunden.
- In der App den Wert `cache_duration` passend zur Server-Einstellung setzen. Die App nutzt ihn, um lange Offline-Zeiten zu erkennen (war sie länger offline als `cache_duration`, sind ältere Nachrichten auf dem Server bereits gelöscht).

### Hinweis „Nachrichten möglicherweise verpasst“

Der Hinweis erscheint in drei Fällen:

- Die App war länger offline als `cache_duration`.
- Die zuletzt empfangene Nachricht müsste laut Server noch im Cache liegen, wird aber nicht mehr geliefert (Server-Neustart ohne `cache-file`, solange noch eine aktuelle Nachricht im Cache gewesen wäre).
- Die Serveruhr wurde zurückgestellt, während die App offline war.

Das Ausbleiben des Hinweises ist kein Beweis für Vollständigkeit. Nicht erkennbar ist ein verlorener Cache insbesondere, wenn die letzte bekannte Nachricht regulär abgelaufen ist oder keine Ablaufzeit (`expires`) hat (`Cache: no` oder Zustand einer älteren App-Version). Verglichen werden ausschließlich Serverzeiten (Event-Zeiten und Date-Header des Servers), nie die lokale Uhr. Liefert der Server (oder ein Proxy davor) keinen `Date`-Header, kann kein Lückenhinweis berechnet werden.

## Entwicklung

### macOS: App-Bundle für Benachrichtigungen

Native Benachrichtigungen (`UNUserNotificationCenter`) funktionieren nur, wenn die App aus einem signierten App-Bundle mit Bundle-ID läuft: macOS ordnet Berechtigung, Einstellungen und Klicks auf Benachrichtigungen dem Bundle `dev.hemp0r.ntfy-native` zu. Ein nacktes `cargo run` schreibt Benachrichtigungen deshalb nur ins Log.

```sh
scripts/macos-dev-bundle.sh                         # baut target/ntfy-native.app (Release)
scripts/macos-dev-bundle.sh --example notify_demo   # Bundle mit der Demo statt der App
target/ntfy-native.app/Contents/MacOS/ntfy-native   # starten, Ausgabe im Terminal
```

Beim ersten Start fragt macOS nach der Erlaubnis für Mitteilungen. Die Demo zeigt je eine Benachrichtigung pro Stufe (passiv, aktiv mit Buttons und Bild, dringend), gibt Klicks auf Banner und Buttons auf stdout aus und beendet sich nach 30 Sekunden.

Signatur: Ohne `MACOS_SIGN_IDENTITY` wird ad hoc signiert (`codesign --sign -`). macOS startet ad hoc signierte Programme mit dem eingeschränkten Entitlement `com.apple.developer.usernotifications.time-sensitive` nicht, daher wird es nur mit einer echten Identität eingebettet:

```sh
MACOS_SIGN_IDENTITY="Apple Development: …" scripts/macos-dev-bundle.sh
```

Ohne das Entitlement (oder ohne erlaubte zeitkritische Mitteilungen in den Systemeinstellungen) stuft macOS dringende Benachrichtigungen auf „aktiv“ herab; sie durchbrechen dann keinen Fokus („Nicht stören“). Für eine echte Identität ist voraussichtlich ein passendes Provisioning-Profil nötig; die endgültige Signatur und Notarisierung folgen mit dem Packaging (Teil 3).

### Windows: Toast-Benachrichtigungen

Benachrichtigungen laufen über WinRT-Toasts mit der AppUserModelID `dev.hemp0r.ntfy-native`. Beim Start registriert sich die App (bzw. das gestartete Beispiel) selbst, nur unter `HKEY_CURRENT_USER` und bei jedem Start neu (der Pfad der `.exe` kann sich ändern):

- `HKCU\Software\Classes\AppUserModelId\dev.hemp0r.ntfy-native`: `DisplayName` = `ntfy-native`, `CustomActivator` = `{AE88E831-148B-400C-8273-6ABC4FDEA102}`
- `HKCU\Software\Classes\CLSID\{AE88E831-148B-400C-8273-6ABC4FDEA102}\LocalServer32`: Standardwert `"<Pfad der exe>" -ToastActivated`

Klicks auf einen Toast oder dessen Buttons erreichen die laufende App über einen COM-Server mit dieser CLSID (fest, nie ändern). Läuft die App nicht mehr (Toast im Info-Center), startet Windows die eingetragene `.exe` mit `-ToastActivated` neu und liefert den Klick nach. Dringende Nachrichten nutzen `scenario="urgent"`; ältere Windows-10-Builds lehnen das ab, dann wird der Toast ohne dieses Szenario erneut angezeigt.

Manuelle Prüfung mit der Demo:

```powershell
cargo run --example notify_demo
```

Die Demo zeigt je einen Toast pro Stufe (passiv ohne Ton, aktiv mit Buttons, Bild und App-Logo, dringend), gibt Klicks auf stdout aus und beendet sich nach 30 Sekunden. Sie trägt sich dabei selbst als `LocalServer32` ein; ein Klick im Info-Center nach dem Beenden startet sie mit `-ToastActivated` neu, sie zeigt dann keine neuen Toasts, sondern gibt nur die Aktivierung aus. Der nächste Start der App trägt wieder deren eigene `.exe` ein.

Zusätzlich prüfen, ob Bilder aus Pfaden mit Sonderzeichen angezeigt werden (die App bettet Pfade unkodiert als `file:///<pfad>` ein):

```powershell
$env:NTFY_NATIVE_DEMO_DIR = 'C:\Temp\Jörg Ä #1'
cargo run --example notify_demo
```

Die Demo legt ihr Bild dann in diesem Ordner ab. Fehlen Bild und App-Logo im aktiven Toast, muss der Pfad prozentkodiert werden.

## Release

Ein Release entsteht automatisch über `.github/workflows/release.yml`:

1. Version in `Cargo.toml` anheben und committen.
2. `git tag vX.Y.Z && git push --tags` (das Tag muss exakt der Version entsprechen, sonst bricht der Lauf ab). Vorabversionen wie `v0.2.0-rc1` werden als Pre-Release veröffentlicht.
3. Der Workflow prüft (fmt, clippy, Tests auf macOS und Windows), baut eine universelle macOS-DMG (Apple Silicon + Intel) sowie Windows-ZIPs (x64, ARM64) und legt alles samt `SHA256SUMS.txt` im GitHub-Release ab. Ein bestehendes Tag lässt sich über „Run workflow“ (Eingabe `tag`) erneut veröffentlichen.

Optionale Repository-Secrets (ohne sie wird die DMG ad-hoc signiert und der Lauf gibt Warnungen aus):

| Secret | Wirkung |
| --- | --- |
| `MACOS_CERT_P12_BASE64`, `MACOS_CERT_PASSWORD`, `MACOS_SIGN_IDENTITY` | alle drei: Signatur mit Developer-ID-Zertifikat (Hardened Runtime, Zeitstempel) statt ad-hoc |
| `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` | zusätzlich zur Signatur: Notarisierung und Stapling der DMG |
| `MACOS_PROVISION_PROFILE_BASE64` | bettet das Provisioning-Profil ein und aktiviert das zeitkritische Entitlement (ohne Profil startet macOS die App mit diesem Entitlement nicht, daher wird es sonst weggelassen) |

Die Windows-Builds sind nicht signiert (SmartScreen warnt beim ersten Start). Ein MSI-Installer folgt in Teil 3 des Plans.
