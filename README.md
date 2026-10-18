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

## Entwicklung
