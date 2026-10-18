# ntfy Desktop-Bridge – Design

Stand: 2026-10-18 · Name: **ntfy-native**

## 1. Absicht und Warum

ntfy liefert Desktop-Benachrichtigungen offiziell über Browser-Notifications. Das hat zwei Probleme:

- **Subscriptions hängen am Browser-Profil.** Wird ein Profil oder werden Browser-Einstellungen gelöscht (passiert regelmäßig), sind alle Subscriptions weg.
- **Der Browser ist als Zustellweg überdimensioniert.** Für das reine Zustellen von Nachrichten muss ein Browser laufen – das kostet Ressourcen und ist ein unnötig allgemeines System für eine einfache Aufgabe.

Gewünscht ist stattdessen eine **sehr leichtgewichtige, native Bridge zwischen ntfy-API und OS-Benachrichtigungssystem**: Sie abonniert Topics auf einer oder mehreren ntfy-Instanzen und reicht jede Nachricht an das native Benachrichtigungssystem von macOS bzw. Windows weiter. Dadurch greifen automatisch die System-Mechanismen wie Fokus, „Nicht stören“ und Stummschaltung pro App. Die Konfiguration gehört der App, nicht dem Browser, und übersteht damit jeden Browser-Reset.

Erster Nutzer ist der Auftraggeber selbst (macOS und Windows). Das Projekt ist von Anfang an **Open Source** und soll für jeden nutzbar sein, der ntfy einsetzt.

## 2. Ziele, Nicht-Ziele, Anti-Ziele

**Ziele (v1)**
- Native Benachrichtigungen auf **macOS** und **Windows** für Nachrichten von ntfy-Instanzen.
- Mehrere ntfy-Instanzen und beliebig viele Topics gleichzeitig.
- Einrichtung per **Wizard** nach der Installation; alternativ **unattended** per `config.yaml`.
- **Tray-Icon mit Popover-Fenster** für Status, Topics, Pausieren, letzte Nachrichten.
- **Keine verpassten Nachrichten** – auch nach Schlafmodus, Netzwechsel oder Server-Neustart.
- **Unter 50 MB RAM im Ruhezustand** (Popover geschlossen).

**Nicht-Ziele (v1)**
- Linux.
- Nachrichten aus der App heraus senden/publishen.
- Ein Nachrichtenarchiv über die letzten Einträge im Popover hinaus.
- Mobile Plattformen.

**Anti-Ziele** – selbst wenn alles technisch funktioniert, wäre das ein Fehlschlag:
- **Hoher RAM-Verbrauch.** Liegt die App im Ruhezustand über 50 MB, ist sie gescheitert.
- **Verpasste Nachrichten.** Eine stille Verbindungsunterbrechung, nach der Nachrichten fehlen, ist der schlimmste Fehler.

## 3. Rahmenbedingungen

- **Sprache: Rust** (Vorgabe des Auftraggebers).
- **Plattformen:** macOS (aktuelle und vorherige Hauptversion, Apple Silicon + Intel) und Windows 10/11 (x64, idealerweise auch ARM64).
- **Native OS-APIs** für Benachrichtigungen – keine Eigenbau-Pop-ups, damit Fokus/Mute/„Nicht stören“ vom System beachtet werden.
- **Signierung:** Der Auftraggeber hat ein **Apple-Developer-Abo** → macOS-Builds werden signiert und notarisiert. Windows startet unsigniert (siehe 5.8).
- Open Source, Code auf GitHub.

## 4. Vom Auftraggeber entschieden

### 4.1 Einrichtung
- Nach der Installation startet ein **Konfigurations-Wizard**: ntfy-Instanz(en), Zugangsdaten, Topics.
- Für **unattended** Installationen liest die App stattdessen eine `config.yaml`; der Wizard erscheint dann nicht.

### 4.2 Tray + Popover (Variante B)
Klick auf das Tray-Icon (macOS-Menüleiste / Windows-Infobereich) öffnet ein **kleines Popover-Fenster** mit:
- Verbindungsstatus (pro Instanz),
- Liste der Topics mit Schalter zum Pausieren einzelner Topics,
- den letzten Nachrichten,
- „Alle pausieren“ (mit Dauer),
- „+ Topic“ (Topic hinzufügen),
- Einstellungen,
- Beenden.

Das Tray-Icon selbst zeigt den Zustand an (verbunden / Verbindungsproblem / pausiert), sodass ein Verbindungsverlust sofort sichtbar ist.

### 4.3 Benachrichtigungsinhalt
- Titel und Text **so, wie sie von ntfy kommen**.
- Klick auf die Benachrichtigung öffnet die **Click-URL**, falls die Nachricht eine hat.
- **Priorität 5 und „Nicht stören“:** Ob dringende Nachrichten „Nicht stören“ durchbrechen dürfen, **entscheidet der Nutzer** per Einstellung. Standard: aus.

### 4.4 Verteilung
- macOS signiert + notarisiert (Apple-Developer-Abo vorhanden).

### 4.5 Name
- Die App heißt **ntfy-native**.

### 4.6 Unattended-Installation (Umfang v1)
- Zielgruppe sind **einzelne Nutzer, die per Skript installieren** – kein Firmen-Rollout per MDM/Intune in v1.
- Installation erfolgt **pro Benutzer**; keine rechnerweite Installation, kein `.pkg`, kein MDM-Profil.
- `config.yaml` ist ein **einmaliger Import**, keine dauerhaft verwaltete Konfiguration (Details in 5.9).

## 5. Vom Designer entschieden

Diese Punkte sind Entscheidungen des Designers. Die meisten wurden im Gespräch vorgeschlagen und vom Auftraggeber in der Zusammenfassung bestätigt; Punkte mit **(Designer-Entscheidung)** wurden nicht einzeln besprochen.

### 5.1 Ressourcen-Architektur: schlanker Kern, UI nur bei Bedarf
Der Kern (Verbindungen, Zustellung, Tray-Icon) läuft dauerhaft und bleibt schlank. Das Popover und der Wizard werden **erst beim Öffnen aufgebaut und nach dem Schließen wieder vollständig freigegeben**. Beim Öffnen darf der Verbrauch kurz steigen; nach dem Schließen muss er wieder unter 50 MB fallen.

### 5.2 Weitere ntfy-Features
- **Tags** werden wie bei ntfy als Emojis vor dem Titel dargestellt (ntfy-Emoji-Mapping), unbekannte Tags als Text.
- **Action-Buttons** (`view`, `http`) erscheinen als native Buttons in der Benachrichtigung, soweit das OS es erlaubt. `broadcast` (Android-spezifisch) wird ignoriert.
- **Anhänge/Bilder** werden als Vorschaubild in der Benachrichtigung gezeigt (Bildtypen); andere Anhänge als Link/Hinweis.
- **Icon** (`icon`-Feld) wird verwendet, wo das OS es unterstützt.
- **Priorität → Auffälligkeit:** 1–2 still (kein Ton), 3–4 mit Ton, 5 mit Ton und – nur wenn der Nutzer es erlaubt – als zeitkritisch/dringend.

### 5.3 Keine verpassten Nachrichten
- Pro Subscription wird die **ID der zuletzt zugestellten Nachricht persistent gespeichert**.
- Beim (Wieder-)Verbinden werden verpasste Nachrichten über ntfys `since=<id>` nachgeholt.
- Verbindungen werden aktiv überwacht (ntfy sendet Keepalive-Events); bleibt der Keepalive aus, gilt die Verbindung als tot und wird neu aufgebaut. Ebenso nach Aufwachen aus dem Schlafmodus und bei Netzwechsel.
- Wiederverbinden mit Backoff, aber ohne lange Pausen.
- **Bündelung:** Kommen beim Nachholen viele Nachrichten eines Topics auf einmal, werden sie zu einer zusammenfassenden Benachrichtigung pro Topic gebündelt, statt das System zu fluten. Im Popover sind sie einzeln sichtbar.
- Verbindungsprobleme sind im Tray-Icon und Popover sofort sichtbar.
- **(Designer-Entscheidung) Grenzen des Nachholens:** `since=` funktioniert nur, solange der ntfy-Server die Nachrichten noch im Cache hat (Standard ca. 12 h, ohne Cache-Datei gehen sie bei einem Server-Neustart verloren). Daher:
  - Die README dokumentiert als Empfehlung für Server-Betreiber einen persistenten Cache (`cache-file`) und eine ausreichende `cache-duration`.
  - Die App **erkennt Lücken**: Liefert der Server die gespeicherte letzte ID nicht mehr zurück bzw. liegt die letzte Verbindung länger zurück als der Server-Cache reicht, zeigt sie für das betroffene Topic einen gut sichtbaren Hinweis „Nachrichten möglicherweise verpasst“ (Popover + einmalige Benachrichtigung). Still verlorene Nachrichten darf es nicht geben – entweder sie kommen an oder der Nutzer erfährt, dass etwas fehlen könnte.

### 5.4 Pausieren
- Einzelne Topics an/aus im Popover.
- „Alle pausieren“ mit Dauer (z. B. 15 Min, 1 Std, bis morgen, unbegrenzt).
- **(Designer-Entscheidung)** **Pausieren unterdrückt die Benachrichtigung, nicht das Empfangen**: Nachrichten werden weiter empfangen (und erscheinen in den letzten Nachrichten), damit nach dem Pausieren nichts fehlt.

### 5.5 Mehrere Instanzen und Zugangsdaten
- Mehrere ntfy-Instanzen gleichzeitig, jede mit eigenen Topics.
- Authentifizierung: keine, **Access-Token** oder **Benutzername + Passwort**.
- Geheimnisse liegen im **macOS-Schlüsselbund** bzw. in der **Windows-Anmeldeinformationsverwaltung**, nie dauerhaft im Klartext auf der Platte. Eine unattended `config.yaml` darf Geheimnisse einmalig mitbringen (siehe 5.9).

### 5.6 Nicht-Ziele bestätigt
Linux, Senden, Archiv – siehe Abschnitt 2.

### 5.7 Autostart und Updates
- Autostart beim Login, standardmäßig an (abschaltbar).
- Update-Prüfung gegen GitHub Releases; neue Version wird im Popover angezeigt. Kein automatisches Installieren.

### 5.8 Verteilung
- GitHub Releases mit `.dmg` (macOS, Universal oder getrennt pro Architektur) und `.msi` (Windows).
- Später: Homebrew Cask und winget.
- Windows zunächst **unsigniert**, SmartScreen-Hinweis in der README erklärt. Signierung (z. B. Microsofts Cloud-Signing-Dienst) später nachrüstbar.

### 5.9 (Designer-Entscheidung) `config.yaml` als einmaliger Import
- Beim Start ohne vorhandenen App-Zustand sucht die App eine `config.yaml` (am plattformüblichen Konfigurationsort oder per `--config <pfad>`). Ist eine vorhanden, wird sie importiert und der Wizard übersprungen; sonst startet der Wizard.
- Beim Import werden Geheimnisse in Schlüsselbund/Anmeldeinformationsverwaltung verschoben und **aus der Datei entfernt** (Datei wird ohne Geheimnisse neu geschrieben bzw. umbenannt zu `config.imported.yaml`).
- Danach ist der **App-Zustand die einzige Quelle der Wahrheit**; Änderungen über Popover/Einstellungen werden dort gespeichert. Es gibt keine dauerhaft überwachte, extern verwaltete Konfiguration und keine gesperrten Einstellungen.
- Ein expliziter Re-Import (z. B. `ntfy-native --import config.yaml`) ist erlaubt und überschreibt die betroffenen Einträge.

### 5.10 (Designer-Entscheidung) Identifikatoren
- macOS Bundle-ID und Windows AppUserModelID: **`dev.hemp0r.ntfy-native`**. Diese Werte, Keychain-/Credential-Service-Namen, Konfigurationspfade und Autostart-Einträge leiten sich alle vom Namen `ntfy-native` ab und werden **vor dem ersten öffentlichen Build festgeschrieben** und danach nicht mehr geändert.

### 5.11 (Designer-Entscheidung) „Nicht stören“-Durchbruch pro Topic
- Die Einstellung aus 4.3 gibt es **global als Standard und pro Topic überschreibbar** (wie in der ntfy-Android-App). Standard global: aus.

### 5.12 (Designer-Entscheidung) Messung des 50-MB-Ziels
- Gemessen wird, was der Nutzer sieht: **macOS „Speicher“ in der Aktivitätsanzeige (physical footprint)**, **Windows „Arbeitsspeicher (privater Arbeitssatz)“ im Task-Manager** – jeweils als **Summe aller Prozesse der App**.
- Referenzlast: 2 Instanzen, 10 Topics, volle „letzte Nachrichten“-Liste, Popover einmal geöffnet und wieder geschlossen, danach 1 Minute Ruhe.
- Dieser Wert wird als automatisierter bzw. dokumentierter Release-Check gemessen.

## 6. Dem Builder überlassen

- Wahl der Rust-Crates für Tray, Benachrichtigungen, HTTP/Streaming, Keychain, UI.
- **UI-Technik für Popover und Wizard**, unter den Bedingungen aus 5.1. Hinweis: Eine WebView-basierte UI (z. B. WebView2 unter Windows) liegt allein schon deutlich über dem Budget und kommt höchstens in Frage, wenn sie wirklich nur bei geöffnetem Fenster existiert. Eine robuste Art, das „vollständig freigeben“ zu garantieren, ist ein separater UI-Prozess derselben Binary, der nur bei Bedarf läuft und per IPC mit dem Kern spricht – das ist ein Vorschlag, keine Vorgabe.
- Transport zur ntfy-API (JSON-Stream, SSE oder WebSocket); Zusammenlegen mehrerer Topics einer Instanz in eine Verbindung ist erwünscht.
- Plattform-Details der Benachrichtigungen:
  - macOS: `UNUserNotificationCenter` setzt ein signiertes App-Bundle voraus; „zeitkritisch“ braucht das Time-Sensitive-Entitlement. Critical Alerts (Apple-Sondergenehmigung) werden **nicht** verwendet.
  - Windows: Toasts brauchen eine registrierte AppUserModelID (über die Startmenü-Verknüpfung des Installers); Klicks/Buttons auch aus dem Info-Center müssen die App erreichen. „Nicht stören“-Durchbruch nur im Rahmen dessen, was Windows erlaubt (Prioritätsliste, ggf. „urgent“-Szenario).
- Speicherort und genaues Schema von `config.yaml` und Zustandsdaten (plattformüblich, z. B. `~/Library/Application Support/…`, `%APPDATA%\…`).
- Wie viele „letzte Nachrichten“ das Popover zeigt (klein halten).
- Visuelles Design von Popover und Wizard (orientiert am Mockup Variante B: Kopf mit Status, Topic-Liste mit Schaltern, „Zuletzt“-Liste, Button-Leiste).
- Logging, Teststrategie (z. B. gegen einen lokal gestarteten ntfy-Server), CI-Pipeline für Builds, Signierung und Notarisierung.
- Lizenz (Vorschlag: MIT OR Apache-2.0, wie im Rust-Ökosystem üblich).

## 7. Erfolg

- Auf dem Mac und dem Windows-Rechner des Auftraggebers ersetzt die App die Browser-Notifications vollständig.
- Ruhezustand < 50 MB RAM, gemessen wie in 5.12.
- Nach Schlafmodus, WLAN-Wechsel und ntfy-Neustart kommen alle Nachrichten an (ggf. gebündelt), sofern der Server sie noch hat – andernfalls erhält der Nutzer einen sichtbaren Lücken-Hinweis (5.3).
- Fokus/„Nicht stören“/Stummschaltung des Systems werden beachtet.
- Ein Fremder kann die App von GitHub installieren und per Wizard in wenigen Minuten einrichten.
