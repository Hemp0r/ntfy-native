# ntfy-native Teil 2 – Entscheidungen und offene Punkte

Aus dem Umsetzungs-Ledger (Subagent-Driven Development, 2026-10-19). Ergänzt Spec und Plan; bei Widerspruch gelten diese Entscheidungen.

## Entscheidungen (Rulings)

- Ruling: Arbeit auf feat/ui über dem noch offenen PR #2 (feat/open-folders); nach dessen Merge auf main rebasen — Teil 2 baut auf Tray-Menü/Icon auf — Kosten falls falsch: ein Rebase.
- Ruling R1: Task 3 legt Strings.test_title/test_body (DE/EN wie Tabelle in Task 6) selbst an; Task 6 ergänzt den Rest — Abhängigkeitsreihenfolge — Kosten falls falsch: keine.
- Ruling R2: Task 2 liefert zusätzlich `pub fn draft_from_view(v: &InstanceView) -> InstanceDraft` (Secrets None = behalten) in snapshot.rs; Task 8 (+ Topic) und Task 9 (Wizard::edit) nutzen ihn — vermeidet Doppelcode — Kosten falls falsch: keine.
- Ruling R3: Popover-Signatur wird `popover_ui(ui, snap, state: &mut PopoverState, s, now, out, next_id: &mut u64)` mit `PopoverState { add_topic: String, add_instance: Option<String>, pending: Option<u64>, error: Option<String> }` und `PopoverState::on_save_result(request_id, &Result<String,String>)` — nötig für Inline-Fehler beim Topic-Hinzufügen — Kosten falls falsch: keine.
- Ruling: `health` bekommt `auth: Option<&Auth>` (Abweichung von der Brief-Signatur) und sendet die Draft-Zugangsdaten; 403 auf dem Fallback-Probe gilt als „ist ntfy“, Lesbarkeit entscheiden die Topic-Prüfungen — sonst falsche „Anmeldung abgelehnt“-Meldung hinter Basic-Auth-Proxy — Kosten falls falsch: keine, ntfy authentifiziert jede Anfrage ohnehin.
- Ruling: UiArgs bekommt zusätzlich `scale: Option<f32>` (CLI `--scale`), das der Host aus tao `MonitorHandle::scale_factor()` des Icon-Monitors übergibt; die UI berechnet damit die Position vor dem Anzeigen — die UI kennt den Ziel-Monitor vor dem ersten Frame nicht, gemischte DPI-Setups sonst falsch — Kosten falls falsch: ein zusätzliches Argument.
- Ruling: eframe ohne `default_fonts` (Ubuntu-Light steht unter der nicht OSI-anerkannten Ubuntu Font Licence → SignPath-Konflikt); die UI lädt zur Laufzeit die System-UI-Schrift (macOS SF/Helvetica, Windows Segoe UI) und als Emoji-Fallback eine System-Symbolschrift; Emoji-Darstellung im Popover ist best effort (die OS-Benachrichtigungen zeigen Emoji nativ) — umgesetzt in Task 8 — Kosten falls falsch: einige Emoji erscheinen im Popover als Ersatzzeichen.
- Ruling: rust-version 1.89 → 1.95 akzeptiert (eframe/egui_kittest 0.37) — Homebrew 1.98 und CI stable erfüllen das — Kosten falls falsch: keine.
- Ruling: Kern-Fehlertexte beim Speichern bleiben englisch, werden aber mit einem lokalisierten Präfix („Speichern fehlgeschlagen: …“) angezeigt; häufige Fehler fängt die lokale Prüfung vorher mit übersetzten Texten ab — strukturierte Fehlerarten wären ein Umbau des Kerns — Kosten falls falsch: seltene Fehlermeldungen teilweise englisch.
- Ruling: UI-Fehlerzustand als Option<Problem> (Enum, Übersetzung beim Zeichnen) statt Option<String> akzeptiert — die festgelegten Signaturen bekommen keine UiStrings — Kosten falls falsch: keine.
- Ruling: Die Task-9-Minors 1, 2, 3, 6, 7 werden sofort vom selben Implementer behoben (statt zurückgestellt) — 1 lässt Nutzer mit irreführender Meldung festhängen, 3 betrifft eine Klartext-Kopie des Passworts im Speicher; alle klein und in denselben Dateien — Kosten falls falsch: ein zusätzlicher kleiner Review-Durchgang.
- Ruling: Ist ein Einstellungs- oder Assistentenfenster offen, ersetzt keine Anfrage (Tray-Linksklick, Menü „Einstellungen …“/„Einrichten …“, OpenSettings/OpenWizard aus dem Popover) es — das offene Fenster wird nur nach vorne geholt; nur das Popover wird ersetzt (es hält keine wichtigen Eingaben) — Kosten falls falsch: Nutzer muss das Fenster erst schließen, um ein anderes zu öffnen.
- Ruling: Re-Review der reinen Formulierungskorrektur in der README durch Sichtprüfung des Diffs durch den Controller statt eines weiteren Subagenten — 5 Textzeilen, alle Punkte im Diff eindeutig erfüllt; das Gesamt-Review deckt sie zusätzlich ab — Kosten falls falsch: keine.
- Ruling: Ein Fix-Durchlauf für alle Final-Review-Punkte inkl. CJK-Schrift-Fallbacks und lokalisierter Netzwerk-/URL-Fehlergründe — klein, nutzerrelevant — Kosten falls falsch: keine.
- Ruling: Ein gespeichertes Secret wird nur behalten bzw. für Tests genutzt, wenn der Host (Schema+Host+Port) der neuen URL dem gespeicherten entspricht; sonst muss es neu eingegeben werden — verhindert Versand an Tippfehler-Hosts — Kosten falls falsch: bei Pfadwechsel auf demselben Host bleibt es erhalten (gewollt).
- Final: parked — CJK fallback font (Hiragino Sans GB 23.5 MB / msyh) read fully into every UI process → ~24 MB more and extra latency per popover open — Ruling: akzeptiert für jetzt (Speicher bei offenem UI erlaubt, Prozess endet beim Schließen); später lazy laden/memory-mappen — Kosten falls falsch: spürbar langsameres Öffnen des Popovers auf langsamen Rechnern.
- Final: parked — edit-mode rejected same-origin credentials on Server step show "login required" instead of "login rejected" — Ruling: akzeptiert (Nutzer landet beim Login-Feld, erneuter Versuch zeigt die genaue Meldung) — Kosten falls falsch: leicht ungenaue Meldung.

## Zurückgestellte Kleinigkeiten

- Task 1: minor (deferred): token/username trimmed for emptiness check but stored untrimmed; password only is_empty; no test for Token↔Basic switch with keep; secret written before settings persisted (Task 4 must handle)
- Task 2: minor (deferred): no chunked-reader / exact MAX_LINE boundary tests; snapshot_never_contains_secrets partly vacuous (structural); status↔snapshot module cycle
- Task 3: minor (deferred): snapshot built every loop even without UI attached; ResendSnapshot before settle may be briefly stale
- Task 5: minor (deferred): DNS/TLS kinds still matched on source Display text, untested; chunked body cap untested; credentials sent before ntfy confirmed (ruled); aborted test sends no TestResult → UI must discard stale ids (Task 9)
- Task 6: minor (deferred): format_paused says "morgen" for any non-today date; fields() public only for tests; margin vs monitor not work area (left/right-docked taskbar overlap); chrono linked into core binary (check in memory measurement)
- Task 7: minor (deferred): --probe-ignore-close accepted outside probe; duplicate objc2/windows-sys versions (size); reader panic during handshake exits 0
- Task 8: minor (deferred): pending stays if no SaveResult; colour emoji/CJK render as boxes (add CJK fonts later); ▾ glyph unverified on Windows; fonts read fully (~8 MB) while UI open
- Task 9: minor (deferred): Done screen never drawn (close immediately); remove lock may release on an already-queued snapshot
- Task 10: minor (deferred): mixed-DPI overlapping monitor rects on macOS may pick wrong monitor; writer coalescing ok; Windows reaped() check vacuous; leftover `let _ = host`
- Final: parked — CJK fallback font (Hiragino Sans GB 23.5 MB / msyh) read fully into every UI process → ~24 MB more and extra latency per popover open — Ruling: akzeptiert für jetzt (Speicher bei offenem UI erlaubt, Prozess endet beim Schließen); später lazy laden/memory-mappen — Kosten falls falsch: spürbar langsameres Öffnen des Popovers auf langsamen Rechnern.
- Final: parked — edit-mode rejected same-origin credentials on Server step show "login required" instead of "login rejected" — Ruling: akzeptiert (Nutzer landet beim Login-Feld, erneuter Versuch zeigt die genaue Meldung) — Kosten falls falsch: leicht ungenaue Meldung.

## Offen für manuelle Prüfung

- Siehe README, Abschnitt „Manuelle Prüfung Windows“ (inkl. macOS-Entsprechungen): Klicks, Fokus, Optik, Popover-Position, Fenster nach vorn, Speicher im Task-Manager.
