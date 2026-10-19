# ntfy-native

Schlanke Tray-App für macOS und Windows, die ntfy-Server abonniert und jede Nachricht als native Betriebssystem-Benachrichtigung zustellt. Verpasste Nachrichten werden nachgeholt bzw. Lücken gemeldet; das Tray-Icon zeigt den Verbindungsstatus.

## Starten

- macOS: `scripts/macos-dev-bundle.sh` baut `target/ntfy-native.app`; per Doppelklick oder `open target/ntfy-native.app` starten. Die App erscheint nur in der Menüleiste (kein Dock-Symbol).
- Windows: `ntfy-native.exe` starten; die App erscheint im Infobereich der Taskleiste.

Beim ersten Start (noch kein Server eingerichtet) öffnet sich der [Einrichtungsassistent](#einrichtungsassistent). Alternativ, etwa für eine automatische Installation, lässt sich die App über eine `config.yaml` einrichten (siehe [Konfiguration](#konfiguration)): Datei am angegebenen Ort anlegen und die App (neu) starten. Eine vorhandene `config.yaml` wird beim Start nur importiert, solange noch keine Instanz eingerichtet ist; danach nur noch explizit mit `--import <pfad>`. Ohne Konfiguration zeigt das Tray-Menü „Nicht eingerichtet“ mit dem erwarteten Pfad. Schlägt der Import fehl (z. B. Tippfehler oder fehlendes `url:`), zeigt es stattdessen „Konfiguration fehlerhaft – siehe Logs“, und eine Benachrichtigung „Konfiguration konnte nicht gelesen werden“ nennt Datei und Fehler (etwa ``config.yaml: instance 1: missing field `url` ``); die Datei bleibt dann unverändert liegen und kann korrigiert werden; der Assistent öffnet sich in diesem Fall nicht von selbst.

Ein Linksklick auf das Tray-Icon öffnet das [Popover](#popover), ein zweiter schließt es wieder. Das Menü per Rechtsklick enthält:

- den Verbindungsstatus (nicht anklickbar),
- „Einrichten …“ (nur solange nichts eingerichtet ist): öffnet den Einrichtungsassistenten,
- „Einstellungen …“: öffnet das [Einstellungsfenster](#einstellungen),
- „Konfigurationsordner öffnen“: öffnet den Ordner für die `config.yaml` im Finder bzw. Explorer (legt ihn bei Bedarf an),
- „Logs öffnen“: öffnet den Log-Ordner,
- „Beenden“.

Es läuft immer nur eine Instanz; ein zweiter Start beendet sich sofort (mit `--import` als Fehler, siehe [Konfiguration](#konfiguration)) und schreibt das nur ins Log. **Vor einem Neustart die laufende App daher über „Beenden“ im Tray-Menü schließen**, sonst bleibt die alte Instanz aktiv und eine neue `config.yaml` wird nicht gelesen.

Logs liegen in `log_dir` (Tray-Menü „Logs öffnen“), eine Datei pro Tag, die letzten 7 werden behalten:

- macOS: `~/Library/Application Support/dev.hemp0r.ntfy-native/logs/`
- Windows: `%APPDATA%\hemp0r\ntfy-native\data\logs\`

Die Ausführlichkeit steuert die Umgebungsvariable `NTFY_NATIVE_LOG` (Standard `info`; Syntax wie `RUST_LOG`, z. B. `debug` oder `info,ntfy_native=debug`). Debug-Builds schreiben zusätzlich auf stderr.

Speicherbedarf (macOS, Intel, Release-Build, zwei Instanzen mit zusammen 10 Topics, nach 1 Minute Ruhe): `phys_footprint` rund 13,5 MB (gemessen: 14 118 912 Byte nach fünfmaligem Öffnen und Schließen des Popovers, Ziel < 30 MB). Bei geschlossenem Popover, Assistent und Einstellungsfenster läuft nur dieser eine Prozess.

## Popover

Das Popover erscheint direkt am Tray-Icon (macOS unter der Menüleiste, Windows über der Taskleiste, auch auf einem zweiten Monitor) und schließt sich bei Esc, bei einem Klick daneben oder einem erneuten Klick auf das Icon. Es zeigt:

- je Server den Verbindungsstatus und die Topics mit einem Schalter zum Stummschalten; „+ Topic“ fügt ein Topic hinzu,
- „Pause“ für alle Benachrichtigungen (15 Minuten, 1 Stunde, bis morgen früh, bis zum Fortsetzen) und „Fortsetzen“,
- die zuletzt eingegangenen Nachrichten; ein Klick öffnet die Click-URL der Nachricht, falls sie eine hat,
- Hinweise „Nachrichten möglicherweise verpasst“ (siehe [unten](#hinweis-nachrichten-möglicherweise-verpasst)) mit „Ausblenden“,
- „Einstellungen“ und „Beenden“.

Das Popover läuft als eigener, kurzlebiger Prozess (`ntfy-native --ui popover`), der sich beim Schließen vollständig beendet; im Ruhezustand belegt nur die schlanke Tray-App Speicher. Stürzt das Fenster ab oder hängt es, läuft die Tray-App unbeeinflusst weiter, ein hängendes Fenster wird nach einer Sekunde beendet, und der nächste Klick startet ein frisches.

## Einrichtungsassistent

Der Assistent (beim ersten Start automatisch, sonst über „Einrichten …“ im Tray-Menü) führt in wenigen Schritten durch die Einrichtung eines Servers:

1. **Server-Adresse**, z. B. `https://ntfy.sh` oder ein eigener Server. Mit „Weiter“ prüft die App, ob dort ein ntfy-Server erreichbar ist. Verlangt der Server schon dafür eine Anmeldung (etwa hinter einem Proxy mit Basic-Auth), geht es mit dem Hinweis „Der Server verlangt eine Anmeldung.“ direkt zur Anmeldung.
2. **Anmeldung**: ohne Anmeldung, Access-Token oder Benutzername und Passwort. Falsche Zugangsdaten meldet der Assistent sofort. Beim Bearbeiten behält ein leeres Feld das gespeicherte Token bzw. Passwort – aber nur, solange Schema, Host und Port der Adresse gleich bleiben; für eine neue Server-Adresse müssen die Zugangsdaten neu eingegeben werden, damit sie nie an einen anderen Server gehen.
3. **Topics**: ein oder mehrere Topics; Topics ohne Leserecht werden markiert.
4. **Benachrichtigungen**: ob dringende Nachrichten (Priorität 5) „Nicht stören“ durchbrechen dürfen, und „Test-Benachrichtigung senden“ zum Ausprobieren (die Test-Benachrichtigung wird sofort gesendet). Erscheint nichts, die Mitteilungen für ntfy-native in den Systemeinstellungen erlauben. Bei der Ersteinrichtung steht hier außerdem „Beim Anmelden automatisch starten“ (standardmäßig an, siehe [Autostart](#autostart)).

Erst „Fertig“ speichert den Server und übernimmt die Einstellung für dringende Nachrichten sowie den Autostart; bei einem Fehler oder „Abbrechen“ bleiben Server und Einstellungen unverändert. Während des Speicherns sind „Zurück“ und „Abbrechen“ gesperrt. Zugangsdaten landen wie beim Import im Schlüsselbund bzw. in der Anmeldeinformationsverwaltung, nie in `settings.json` oder im Log.

## Einstellungen

Das Einstellungsfenster („Einstellungen …“ im Tray-Menü oder im Popover) verwaltet alle Server: „Server hinzufügen“, „Bearbeiten“ (Adresse, Anmeldung und Topics im Assistenten; ein leer gelassenes Token- bzw. Passwortfeld behält das gespeicherte, solange die Adresse auf denselben Server zeigt) und „Entfernen“. Die Server-Cache-Dauer wählt man je Server direkt in der Liste (12 Std., 24 Std., 72 Std., 7 Tage). Je Topic lassen sich „Stumm“ und „Dringend“ (Standard, An, Aus) einstellen. Unter „Allgemein“ stehen die globale Einstellung für dringende Nachrichten, der Schalter „Beim Anmelden automatisch starten“ (siehe [Autostart](#autostart)), die Ordner für Konfiguration und Logs sowie die Version. Änderungen gelten sofort, ohne Neustart.

Ist das Einstellungsfenster oder der Assistent schon offen, holt jeder weitere Aufruf (Tray-Klick, „Einstellungen …“, „Einrichten …“) dieses Fenster nach vorn, statt es zu ersetzen; nicht gespeicherte Eingaben bleiben erhalten. Nur das Popover wird durch ein anderes Fenster ersetzt.

## Autostart

ntfy-native kann beim Anmelden automatisch starten (nur das Tray-Icon, kein Fenster). Ob der Autostart aktiv ist, entscheidet allein das Betriebssystem; ntfy-native speichert dafür keinen eigenen Wert und schaltet ihn nie ungefragt ein.

- **Assistent:** Bei der Ersteinrichtung (Assistent beim ersten Start bzw. über „Einrichten …“) bietet der Schritt „Benachrichtigungen“ die Option „Beim Anmelden automatisch starten“ an, standardmäßig eingeschaltet – außer der Eintrag wurde zuvor im Task-Manager bzw. in den Anmeldeobjekten deaktiviert; dann ist sie zunächst aus und wird nur durch ausdrückliches Ankreuzen wieder eingeschaltet. Sie wird erst nach erfolgreichem Speichern mit „Fertig“ angewendet. Klappt das nicht oder verlangt macOS eine Bestätigung, bleibt der Assistent auf der Abschlussseite stehen, zeigt den Hinweis und schließt sich erst mit „Schließen“; der Server ist trotzdem gespeichert. Beim Hinzufügen weiterer Server über die Einstellungen erscheint die Option nicht.
- **Einstellungen:** Der Schalter unter „Allgemein“ zeigt den tatsächlichen Zustand im Betriebssystem und wirkt sofort. Fehler erscheinen direkt darunter. Ohne App-Bundle (z. B. `cargo run` auf macOS) ist er ausgegraut mit dem Hinweis „Nur in der installierten App verfügbar“.
- **Bestehende Installationen** sehen keinen Assistenten; der Autostart bleibt aus, bis man ihn in den Einstellungen einschaltet.

**Windows:** Der Eintrag ist der Wert `ntfy-native` unter `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` mit dem Pfad zur laufenden `ntfy-native.exe`. Sichtbar und abschaltbar ist er im Task-Manager › Autostart (bzw. Einstellungen › Apps › Autostart). Dort deaktiviert, zeigen die Einstellungen „aus“ mit dem Hinweis „Im Task-Manager deaktiviert – zum Aktivieren hier einschalten“; erst ein ausdrückliches Einschalten des Schalters aktiviert ihn wieder. Wird die `.exe` an einen anderen Ort verschoben (etwa ein neu entpacktes ZIP), korrigiert ntfy-native beim Start einen aktiven Eintrag auf den neuen Pfad; ein abgeschalteter bleibt aus.

**macOS:** ntfy-native meldet sich als Anmeldeobjekt an (Systemeinstellungen › Allgemein › Anmeldeobjekte, Liste „Beim Anmelden öffnen“). Verlangt macOS dort eine Bestätigung, zeigt die App „macOS verlangt eine Bestätigung: Systemeinstellungen › Allgemein › Anmeldeobjekte“. Lässt macOS das Anmeldeobjekt nicht zu, weil die App nicht mit einer Developer ID signiert ist (ad-hoc signierte Releases), legt ntfy-native stattdessen den LaunchAgent `~/Library/LaunchAgents/dev.hemp0r.ntfy-native.plist` an; er erscheint in den Anmeldeobjekten unter „Im Hintergrund erlauben“. Wird der LaunchAgent dort abgeschaltet, zeigt der Schalter „aus“ mit dem Hinweis „In den Systemeinstellungen › Allgemein › Anmeldeobjekte deaktiviert – dort wieder erlauben“; ein abgeschaltetes Anmeldeobjekt meldet macOS als „Bestätigung nötig“ (Schalter aus, mit dem Bestätigungs-Hinweis – die App startet so nicht beim Anmelden). Wieder erlauben lässt sich beides nur in den Systemeinstellungen, nicht aus der App; Ausschalten in der App entfernt einen aktiven Eintrag ganz.

**Entfernen:** Vor dem Löschen der App den Schalter in den Einstellungen ausschalten; das entfernt das Anmeldeobjekt bzw. den LaunchAgent und den `Run`-Wert. Ist die App schon gelöscht, lässt sich ein übrig gebliebener Eintrag von Hand entfernen: unter Windows mit `reg delete HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v ntfy-native /f`, unter macOS durch Löschen der genannten `.plist` bzw. über „–“ in den Anmeldeobjekten.

## Konfiguration

Die Datei `config.yaml` ist eine Alternative zum Einrichtungsassistenten, vor allem für eine automatische Installation (z. B. per Softwareverteilung): Liegt sie beim ersten Start am Standardort, wird sie importiert und der Assistent öffnet sich nicht.

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

Speicherort der Datei (`config.yaml`; „Konfigurationsordner öffnen“ im Tray-Menü öffnet den Ordner):

- macOS: `~/Library/Application Support/dev.hemp0r.ntfy-native/config.yaml`
- Windows: `%APPDATA%\hemp0r\ntfy-native\config\config.yaml`

Fehlt `config.yaml`, wird am selben Ort auch `config.yml` akzeptiert (nur am Standardort, nicht für `--config`); gibt es beide, gilt `config.yaml`. Die Datei muss als UTF-8 gespeichert sein (nicht UTF-16).

Windows blendet bekannte Dateiendungen standardmäßig aus: Eine im Editor als „config.yaml“ gespeicherte Datei heißt dann womöglich `config.yaml.txt`. Im Explorer die Dateinamenerweiterungen einblenden (Windows 11: „Anzeigen“ → „Einblenden“ → „Dateinamenerweiterungen“, Windows 10: „Ansicht“ → „Dateinamenerweiterungen“) und die Datei umbenennen. Findet die App keine Konfiguration, aber eine Datei wie `config.yaml.txt`, `config.yml.txt` oder `config.txt`, nennt eine Warnung im Log den gefundenen und den erwarteten Namen.

Ein anderer Pfad lässt sich mit `--config <pfad>` angeben; `--import <pfad>` importiert eine Datei explizit. **App vor `--import` beenden:** Läuft ntfy-native bereits, bricht `--import` mit einer Fehlermeldung (stderr und Log) und einem Exit-Code ungleich 0 ab, ohne etwas zu importieren.

### Ohne Anmeldung

Für einen Server ohne Zugangsschutz `token`, `username` und `password` einfach weglassen:

```yaml
instances:
  - url: https://ntfy.example.org
    topics:
      - backups
```

Bei einem erneuten Import behält eine Instanz ohne diese Felder allerdings ihre gespeicherten Zugangsdaten; um sie zu entfernen, `auth: none` angeben.

### Import und Secrets

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

### Popover-Prozess testen

Nur für die Entwicklung: Mit `NTFY_NATIVE_DEV_POPOVER_CYCLES=<n>` öffnet und schließt die App nach dem Start das Popover `n`-mal (je 1,5 Sekunden, ohne Position, also mittig), etwa um Speicher und das Aufräumen der UI-Prozesse ohne Klicks zu prüfen:

```sh
NTFY_NATIVE_DEV_POPOVER_CYCLES=5 target/ntfy-native.app/Contents/MacOS/ntfy-native
pgrep -f "ntfy-native --ui"   # danach leer
```

### Windows: Toast-Benachrichtigungen

Benachrichtigungen laufen über WinRT-Toasts mit der AppUserModelID `dev.hemp0r.ntfy-native`. Beim Start registriert sich die App (bzw. das gestartete Beispiel) selbst, nur unter `HKEY_CURRENT_USER` und bei jedem Start neu (der Pfad der `.exe` kann sich ändern):

- `HKCU\Software\Classes\AppUserModelId\dev.hemp0r.ntfy-native`: `DisplayName` = `ntfy-native`, `CustomActivator` = `{AE88E831-148B-400C-8273-6ABC4FDEA102}`, `IconUri` = Pfad zu `icon.png` im Datenordner (`%APPDATA%\hemp0r\ntfy-native\data\icon.png`, bei jedem Start der App geschrieben, falls fehlend oder veraltet; das Beispiel setzt `IconUri` nicht)
- `HKCU\Software\Classes\CLSID\{AE88E831-148B-400C-8273-6ABC4FDEA102}\LocalServer32`: Standardwert `"<Pfad der exe>" -ToastActivated`

Zusätzlich legt die App beim Start die Startmenü-Verknüpfung `%APPDATA%\Microsoft\Windows\Start Menu\Programs\ntfy-native.lnk` an (Ziel: die laufende `.exe`, mit `System.AppUserModel.ID` = `dev.hemp0r.ntfy-native` und `System.AppUserModel.ToastActivatorCLSID` = obige CLSID). Ohne diese Verknüpfung zeigt Windows die Toasts zwar an, behält sie aber nicht im Info-Center und führt die App nicht unter „Einstellungen“ → „System“ → „Benachrichtigungen“. Die Verknüpfung wird nur neu geschrieben, wenn sie fehlt oder Ziel bzw. Eigenschaften abweichen (etwa nach dem Verschieben der `.exe`). Auch die Demo schreibt sie (auf ihre eigene `.exe`); der nächste Start der App korrigiert das wieder.

Wer den App-Ordner einfach löscht, hinterlässt Verknüpfung, Registry-Einträge und Daten. Vollständig entfernen (App vorher über „Beenden“ schließen):

```powershell
Remove-Item "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\ntfy-native.lnk"
Remove-Item -Recurse "HKCU:\Software\Classes\AppUserModelId\dev.hemp0r.ntfy-native"
Remove-Item -Recurse "HKCU:\Software\Classes\CLSID\{AE88E831-148B-400C-8273-6ABC4FDEA102}"
Remove-Item -Recurse "$env:APPDATA\hemp0r\ntfy-native"   # Konfiguration, Einstellungen, Logs, icon.png
Remove-Item -Recurse "$env:LOCALAPPDATA\hemp0r\ntfy-native"   # Cache (Bilder)
```

Gespeicherte Zugangsdaten liegen in der Anmeldeinformationsverwaltung (Windows-Anmeldeinformationen) und lassen sich dort löschen.

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

### App-Icon

Quelle ist `assets/icon.svg` (weiße Glocke auf grünem, abgerundetem Quadrat). `scripts/gen-icons.sh` (nur macOS: `sips`, `iconutil`, `python3`) erzeugt daraus die eingecheckten Dateien `assets/AppIcon.icns` (macOS-Bundle, `CFBundleIconFile`), `assets/icon.ico` (16, 32, 48 und 256 px; per `build.rs` und `winresource` in die `.exe` eingebettet, nur bei einem Build auf Windows für Windows) und `assets/icon-256.png` (Toast- und Einstellungs-Icon unter Windows). Nach einer Änderung am SVG das Skript ausführen und die erzeugten Dateien mit committen. Die Tray-Icons sind davon unabhängig.

## Manuelle Prüfung Windows

Diese Schritte lassen sich nicht automatisieren und werden einmal pro Release auf einem Windows-11-Rechner durchgeführt (Release-Build, `ntfy-native.exe` aus dem ZIP). Vorbereitung: App über „Beenden“ schließen und den Zustand zurücksetzen (siehe die PowerShell-Befehle im Abschnitt [Windows: Toast-Benachrichtigungen](#windows-toast-benachrichtigungen)), sodass weder `config.yaml` noch `settings.json` existieren. Logs: `%APPDATA%\hemp0r\ntfy-native\data\logs\`.

**Erststart und Assistent**

1. `ntfy-native.exe` ohne Konfiguration starten. Erwartet: Der Einrichtungsassistent öffnet sich von selbst, das Tray-Icon erscheint im Infobereich.
2. Assistent mit `https://ntfy.sh`, ohne Anmeldung und einem frei gewählten, öffentlichen Topic durchlaufen, „Test-Benachrichtigung senden“ drücken, „Fertig“. Erwartet: Der Test-Toast erscheint; nach „Fertig“ schließt sich das Fenster und das Tray-Menü zeigt „Verbunden“.
3. Per `curl -d hallo ntfy.sh/<topic>` eine Nachricht senden. Erwartet: Toast mit „hallo“.

**Popover**

4. Linksklick auf das Tray-Icon (Taskleiste unten). Erwartet: Das Popover erscheint direkt am Icon, vollständig sichtbar über der Taskleiste, und hat sofort den Fokus (Tastatureingabe geht hinein).
5. Optional: Dasselbe mit der Taskleiste oben bzw. links (unter Windows 11 ist unten die einzige native Position; oben/links nur mit Drittwerkzeugen oder unter Windows 10 über Einstellungen › Personalisierung › Taskleiste) sowie, falls vorhanden, auf einem zweiten Monitor. Erwartet: Das Popover liegt am Icon, nie unter der Taskleiste und nie außerhalb des Bildschirms.
6. Popover öffnen, daneben klicken. Erwartet: Es schließt sich. Erneut öffnen, Esc drücken. Erwartet: Es schließt sich. Erneut öffnen, nochmals auf das Tray-Icon klicken. Erwartet: Es schließt sich und öffnet sich nicht sofort wieder.
7. Topic-Schalter (Stumm) umlegen, dann eine Nachricht senden. Erwartet: Bei stummem Topic kein Toast, nach erneutem Umlegen wieder Toast.
8. „Pause“ › „15 Minuten“. Erwartet: Das Tray-Icon bzw. -Menü zeigt „Pausiert“, gesendete Nachrichten erzeugen keinen Toast; „Fortsetzen“ stellt „Verbunden“ wieder her.
9. Nachricht mit Click-URL senden (`curl -H "Click: https://example.org" -d Klick ntfy.sh/<topic>`) und im Popover anklicken. Erwartet: Der Standardbrowser öffnet `https://example.org`.
10. „+ Topic“, einen Namen eingeben. Erwartet: Das Topic erscheint in der Liste und empfängt Nachrichten.

**Einstellungen**

11. „Einstellungen …“ › Server „Bearbeiten“: Adresse oder Topics ändern, Token- bzw. Passwortfeld leer lassen, speichern. Erwartet: Die Verbindung bleibt authentifiziert (bei einem geschützten Server weiterhin „Verbunden“, keine Anmeldefehler im Log).
12. Zweiten Server hinzufügen, dann „Entfernen“. Erwartet: Eine Bestätigung erscheint; nach Bestätigung ist der Server samt Topics weg, nach „Abbrechen“ bleibt er.
13. Je Topic „Dringend“ auf Standard, An und Aus stellen und je eine Nachricht mit Priorität 5 senden (`-H "Priority: 5"`). Erwartet: Die Auswahl wird gespeichert (Fenster schließen und neu öffnen), „An“ durchbricht „Nicht stören“ bzw. Fokus-Assistent, „Aus“ nicht, „Standard“ folgt der globalen Einstellung.

**Fenster und Fokus**

14. Einstellungen öffnen, ein Eingabefeld ausfüllen (ohne zu speichern), ein anderes Fenster in den Vordergrund holen, dann auf das Tray-Icon klicken und „Einstellungen …“ wählen. Erwartet: Dasselbe Fenster kommt nach vorn, die Eingaben sind erhalten, es öffnet sich kein zweites.
15. Dasselbe mit minimiertem Fenster. Erwartet: Es wird wiederhergestellt und in den Vordergrund geholt.
16. Wiederholen mit dem Assistenten (Zustand zurücksetzen, nicht abschließen, „Einrichten …“ im Tray-Menü). Erwartet: gleiches Verhalten wie bei den Einstellungen.

**Speicher**

17. Alle Fenster schließen (Popover, Einstellungen, Assistent), im Task-Manager (Registerkarte „Details“, Spalte „Arbeitsset (privat)“ einblenden) 1 Minute warten. Erwartet: Genau ein Prozess `ntfy-native.exe`, privater Arbeitssatz unter 30 MB. Danach Popover bzw. Einstellungen öffnen: Solange das Fenster offen ist, läuft ein zweiter `ntfy-native.exe` (der UI-Prozess, gestartet mit `--ui …`; es wird kein WebView verwendet). Nach dem Schließen bleibt innerhalb weniger Sekunden genau ein `ntfy-native.exe` übrig, dessen privater Arbeitssatz unter 30 MB liegt. Mehrmals wiederholen.

**Startmenü und Info-Center**

18. Nach dem Erststart im Log nach `start menu shortcut written` suchen, App beenden und erneut starten. Erwartet: `%APPDATA%\Microsoft\Windows\Start Menu\Programs\ntfy-native.lnk` existiert, die Logzeile steht nur im Log des ersten Starts, beim zweiten Start nicht erneut.
19. Einstellungen › System › Benachrichtigungen öffnen. Erwartet: „ntfy-native“ steht in der Liste (mit Icon) und lässt sich ein- und ausschalten.
20. Mehrere Toasts erzeugen, das Info-Center (Win+N) öffnen. Erwartet: Die Toasts bleiben dort stehen, ein Klick öffnet die Click-URL; nach „Beenden“ der App startet ein Klick auf einen älteren Toast die App neu; erwartet: kein zweites Tray-Icon, es läuft nur eine Instanz, und die Click-URL öffnet sich.

**macOS-Entsprechungen** (Bundle über `scripts/macos-dev-bundle.sh`)

21. Erststart ohne Konfiguration. Erwartet: Der Assistent öffnet sich von selbst und hat den Fokus; nach „Fertig“ zeigt das Menü „Verbunden“, der Test-Toast erscheint (Mitteilungen beim ersten Start erlauben).
22. Linksklick auf das Menüleisten-Icon. Erwartet: Das Popover erscheint direkt unter dem Icon (auch auf einem zweiten Monitor), hat den Fokus, schließt sich bei Esc, bei einem Klick daneben und bei einem zweiten Klick auf das Icon.
23. Schritte 7 bis 16 sinngemäß wiederholen (Stumm, Pause, Click-URL, „+ Topic“, Einstellungen, Fenster nach vorn, auch wenn das Einstellungsfenster minimiert ist). Erwartet: gleiches Verhalten wie unter Windows.
24. Aktivitätsanzeige bei geschlossenen Fenstern. Erwartet: Genau ein Prozess `ntfy-native`, Speicher (Spalte „Speicher“ bzw. `phys_footprint`) unter 30 MB.

## Release

Releases entstehen vollautomatisch beim Merge auf `main`:

1. `.github/workflows/version.yml` bestimmt die nächste Version aus den Commits seit dem letzten Release (Regeln siehe [Versionen und Release Notes](#versionen-und-release-notes)). Muss die Version steigen, schreibt der Workflow sie in `Cargo.toml` und `Cargo.lock` und committet als `chore(release): vX.Y.Z`.
2. Der Workflow setzt das annotierte Tag `vX.Y.Z` und pusht Commit und Tag gemeinsam (ohne Force-Push). Existiert das Tag bereits oder ist `main` zwischenzeitlich weitergelaufen, bricht der Lauf ab; der nächste Merge holt das nach.
3. Anschließend ruft er `.github/workflows/release.yml` auf: Prüfung (fmt, clippy, Tests auf macOS und Windows), universelle macOS-DMG (Apple Silicon + Intel) sowie Windows-ZIPs (x64, ARM64), alles samt `SHA256SUMS.txt` und den aus den Commits erzeugten Release Notes im GitHub-Release. Schlägt nur das Release fehl, genügt „Re-run failed jobs“ oder „Run workflow“ auf dem Workflow „Release“ (Eingabe `tag`, z. B. `v0.1.0`). „Re-run all jobs“ auf dem Version-Lauf bricht bewusst ab, weil der Commit bereits einem Release-Tag zugeordnet ist.
4. Branch Protection bzw. Rulesets auf `main` müssen Pushes von `github-actions[bot]` erlauben, sonst scheitert der Push von Versions-Commit und Tag.

Optionale Repository-Secrets (ohne sie wird die DMG ad-hoc signiert und der Lauf gibt Warnungen aus):

| Secret | Wirkung |
| --- | --- |
| `MACOS_CERT_P12_BASE64`, `MACOS_CERT_PASSWORD`, `MACOS_SIGN_IDENTITY` | alle drei: Signatur mit Developer-ID-Zertifikat (Hardened Runtime, Zeitstempel) statt ad-hoc |
| `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` | zusätzlich zur Signatur: Notarisierung und Stapling der DMG |
| `MACOS_PROVISION_PROFILE_BASE64` | setzt die Zertifikats-Secrets voraus (ohne Signatur wird das Profil ignoriert); bettet das Provisioning-Profil ein und aktiviert das zeitkritische Entitlement. Die Entitlements werden aus dem Profil übernommen; dessen App-ID muss auf `.dev.hemp0r.ntfy-native` enden, sonst bricht der Lauf ab. Ohne Profil startet macOS die App mit diesem Entitlement nicht, daher wird es dann weggelassen |

Die Windows-Builds sind nicht signiert (SmartScreen warnt beim ersten Start). Ein MSI-Installer folgt in Teil 3 des Plans.

## Versionen und Release Notes

Version und Release Notes ergeben sich aus den Commit-Nachrichten nach [Conventional Commits](https://www.conventionalcommits.org/de/): `typ(scope)!: beschreibung`, z. B. `feat(autostart): offer autostart in first-run wizard` oder `fix: reconnect after sleep`. Scope und `!` sind optional; Groß-/Kleinschreibung des Typs spielt keine Rolle (`Feat:` zählt wie `feat:`); erlaubte Typen sind `feat`, `fix`, `docs`, `ci`, `test`, `refactor`, `perf`, `build`, `style`, `chore` und `revert`.

**Nächste Version** (`scripts/bump-kind.sh` und `scripts/next-version.sh`, in der CI getestet über `scripts/test-bump-kind.sh` und `scripts/test-next-version.sh`), betrachtet werden die Commits seit dem neuesten Tag `vX.Y.Z`:

- mindestens ein `feat` → Minor: `X.(Y+1).0` (z. B. `v0.1.2` → `0.2.0`),
- sonst (nur `fix`, `docs`, Commits ohne Typ …) → Patch: `X.Y.(Z+1)`,
- ein Breaking Change (`!` vor dem Doppelpunkt, z. B. `feat(api)!: …`, oder eine Zeile `BREAKING CHANGE:` bzw. `BREAKING-CHANGE:` im Commit-Text) → vor 1.0 Minor, ab 1.0 Major (`(X+1).0.0`),
- gibt es noch kein Tag `vX.Y.Z`, wird die Version aus `Cargo.toml` unverändert veröffentlicht,
- ist die Version in `Cargo.toml` höher als das neueste Tag (numerischer Vergleich), wird sie unverändert veröffentlicht – so lässt sich etwa `1.0.0` bewusst setzen,
- Merge-Commits und `chore(release):`-Commits zählen nicht; Tags, die nicht exakt `vMAJOR.MINOR.PATCH` sind (z. B. `v1.0.0-rc1`), werden ignoriert. Pre-Releases gibt es nicht.

**Release Notes** (`scripts/release-notes.sh`, getestet über `scripts/test-release-notes.sh`) umfassen die Commits vom nächstälteren Tag `vX.Y.Z` bis zum veröffentlichten Tag – auch beim erneuten Ausführen eines alten Releases. Sie sind gruppiert in „Features“ (`feat`), „Fehlerbehebungen“ (`fix`) und „Sonstiges“ (alles andere, auch Commits ohne Typ); leere Gruppen entfallen. Jeder Eintrag ist die erste Zeile der Commit-Nachricht ohne Typ, mit dem Scope fett davor, „⚠️ Breaking:“ bei Breaking Changes und dem Kurz-SHA als Link auf den Commit. Am Ende steht ein Vergleichslink zum vorherigen Release, sofern es eines gibt. Eine nicht konforme Commit-Nachricht lässt das Release nie scheitern, sie landet unter „Sonstiges“.

**PR-Titel:** Wird ein PR per Squash gemergt, wird sein Titel zur Commit-Zeile. Der Workflow `.github/workflows/pr-title.yml` prüft daher mit `scripts/check-commit-subject.sh`, dass jeder PR-Titel dem Schema `typ(scope)!: beschreibung` mit einem der obigen Typen folgt. Bei normalen Merges zählen die einzelnen Commits des PRs, der Merge-Commit selbst nicht.
