# ntfy-native

Schlanke Tray-App für macOS und Windows, die ntfy-Server abonniert und jede Nachricht als native Betriebssystem-Benachrichtigung zustellt. Verpasste Nachrichten werden nachgeholt bzw. Lücken gemeldet; das Tray-Icon zeigt den Verbindungsstatus.

## Konfiguration

`config.yaml` ist ein einmaliger Import. Danach ist der interne App-Zustand (`settings.json`) die einzige Quelle der Wahrheit.

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

Regeln:

- `url` muss mit `http://` oder `https://` beginnen; ein abschließender `/` wird entfernt.
- Topic-Namen: 1 bis 64 Zeichen aus `A-Z a-z 0-9 - _`.
- `token` und `username`/`password` schließen sich aus; `username` und `password` gehören zusammen.
- Ein erneuter Import ersetzt Topics und Zugangsdaten der betroffenen Instanz (erkannt an der URL); andere Instanzen bleiben unverändert. `urgent_breaks_dnd` ändert sich nur, wenn es in der Datei steht.

Speicherort der Datei (`config.yaml`):

- macOS: `~/Library/Application Support/dev.hemp0r.ntfy-native/config.yaml`
- Windows: `%APPDATA%\hemp0r\ntfy-native\config\config.yaml`

Ein anderer Pfad lässt sich mit `--config <pfad>` angeben; `--import <pfad>` importiert eine Datei explizit.

Secrets wandern in den Schlüsselbund (macOS) bzw. die Anmeldeinformationsverwaltung (Windows), die Datei wird zu `config.imported.yaml` ohne Secrets umgeschrieben (Benutzername bleibt erhalten) und das Original gelöscht.

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
