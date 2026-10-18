# ntfy-native Teil 1 – Entscheidungen und offene Punkte

Aus dem Umsetzungs-Ledger (Subagent-Driven Development, 2026-10-18). Ergänzt Spec und Plan; bei Widerspruch gelten diese Entscheidungen.

## Entscheidungen (Rulings)

- Ruling: Arbeit auf Feature-Branch im Haupt-Checkout statt separatem Worktree — frisches Repo, main bleibt unberührt — Kosten falls falsch: keine, Branch jederzeit verschiebbar.
- Ruling R1: TrayState::AuthFailed nutzt IconKind::Problem — beides ist ein Verbindungsfehler aus Nutzersicht — Kosten falls falsch: ein weiteres Icon später.
- Ruling R2: rustup-Installation ist Toolchain-Änderung außerhalb des Repos → Auftraggeber wird gefragt (parallel zu Task 1); bis zur Antwort laufen Tasks 1–13 weiter — Kosten falls falsch: Task 14 wartet.
- Task 3: Ruling: keyring-core + apple-native-keyring-store/windows-native-keyring-store statt keyring-Metacrate akzeptiert — vermeidet Linux-zbus-Abhängigkeit, gleiche Backends — Kosten falls falsch: Austausch der Abhängigkeit, API hinter SecretStore gekapselt.
- Task 3: Ruling: Auth bekommt manuelles, schwärzendes Debug statt derive (Plan schrieb derive vor) — Logs liegen auf der Platte, Spec verbietet Klartext-Secrets dort — Kosten falls falsch: keine, Debug-Ausgabe nur weniger detailliert.
- Task 4: Ruling: Minor #2 (Zugangsdaten in URL landen im Klartext in settings.json) auf Fix hochgestuft — verletzt Global Constraint "keine Klartext-Secrets" — Kosten falls falsch: URLs mit user:pw@ werden abgelehnt.
- Task 4: Ruling: Re-Import übernimmt `muted` gleichnamiger Topics — muted steht nicht in der YAML, ist also kein "betroffener Eintrag" (Spec 5.9) — Kosten falls falsch: Stummschaltung bleibt nach Re-Import erhalten.
- Task 4: Ruling: doppelte (normalisierte) URLs in einer Datei werden abgelehnt — stilles Überschreiben ist überraschend — Kosten falls falsch: Nutzer muss Datei bereinigen.
- Task 7: Ruling: Spec §5.3 nennt `since=<id>`; Plan nutzt Server-Zeitstempel, weil `since=<id>` über mehrere Topics in einer Verbindung nicht trägt — Kosten falls falsch: Umbau auf eine Verbindung pro Topic.
- Task 7: Ruling: Dedupe nur noch per ID-Ring, die Zeit-Abschneideregel in accept entfällt (überschreibt Brief-Test old_message_rejected…) — Poll beginnt bei ≥ last_message−Marge, alle dort zugestellten IDs liegen im Ring; damit kann ein Server-Uhrsprung keine Nachricht mehr still verwerfen — Kosten falls falsch: >200 Nachrichten in der Marge könnten doppelt kommen.
- Task 7: Ruling: observe_time folgt Uhr-Rücksprüngen (t < watermark−Marge → watermark = t, last_message.time gedeckelt) (überschreibt Brief-Test watermark_monotonic) — sonst zielt der Catch-up nach Server-Uhrkorrektur in die Zukunft — Kosten falls falsch: etwas größere Catch-up-Fenster.
- Task 7: Ruling: neue Methode rebaseline_after_gap(server_now): watermark = max(w, server_now), last_message = None; Task 8 ruft sie nach erkannter Lücke vor accept auf — Spec verlangt einmaligen Hinweis, keine Flut — Kosten falls falsch: eine echte zweite Lücke direkt danach würde nur über (a) erkannt.
- Task 7: Ruling: Lücken-Sentinel (b) nutzt das `expires`-Feld der Nachricht statt cache_duration; Nachrichten ohne expires sind kein Sentinel (Message/SeenMessage bekommen expires: Option<i64>) — präziser, unabhängig von falsch konfigurierter cache_duration, keine Dauer-Fehlalarme bei Cache:no — Kosten falls falsch: (a) bleibt von cache_duration abhängig.
- Task 7: Ruling: DEDUP_MARGIN_SECS 10 → 30 und saturating_sub — ntfy delayed sender bis ~10 s, Duplikate fängt der ID-Ring — Kosten falls falsch: minimal größere Overlaps.
- Task 7: Ruling: README korrigieren: ohne cache-file kann ein Server-Neustart Nachrichten unerkennbar verlieren, wenn keine Nachricht mehr im Cache-Fenster liegt → cache-file ist Voraussetzung der Garantie, nicht nur Empfehlung — Kosten falls falsch: keine.
- Task 7: Ruling (für Task 8): Stream-since = catch_up_since(state) nach accept/rebaseline (Server-Event-Zeiten statt Date-Header); Date-Header (Fallback lokale Uhr) nur für detect_gap; Antwort-Header X-Messages-Truncated: 1 beim Poll ⇒ Lücke.
- Task 7: parked — sentinel per instance while gap flags per topic; removed topic of last_message → one spurious gap (rebaseline prevents repeats) — Ruling: real, harmless, deferred.
- Task 7: Ruling: accept hebt die watermark nur an (max), die Rücksprung-Regel gilt nur in observe_time (Keepalives) — behebt Rücknahme des Rebaseline und Fehlauslösung durch alte ungesehene Nachrichten — Kosten falls falsch: Rücksprung wird erst beim nächsten Keepalive erkannt.
- Task 7: Ruling: detect_gap bekommt Bedingung (c) server_now < watermark − Marge (Server-Uhr wurde offline zurückgestellt) ⇒ Hinweis statt stillem Verlust; rebaseline_after_gap setzt watermark = server_now (nicht max) — kein Wiederholungs-Poll mit since=all, weil das alte Nachrichten erneut melden würde — Kosten falls falsch: in diesem seltenen Fall nur Hinweis, keine Nachholung.
- Task 7: Ruling: Reihenfolge wird gekapselt — neue Methode InstanceState::apply_catch_up(polled, server_now: Option<i64>, cache_duration) -> CatchUpResult { gap, accepted }: detect → ggf. rebaseline → accept → observe_time(server_now); Task 8 nutzt nur diese — verhindert Aufruf-Reihenfolgefehler — Kosten falls falsch: keine.
- Task 7: Ruling: fehlt der Date-Header (server_now None), keine Lückenprüfung und kein Rebaseline (Warnung loggen), keine lokale Uhr — ntfy/Go und RFC-konforme Proxies senden Date immer; ersetzt die frühere Fallback-Ruling für Task 8 — Kosten falls falsch: bei Date-losen Proxys keine Lückenhinweise.
- Task 8: Ruling: Backoff-Reset und Status(Connected) erst beim ersten geparsten Stream-Event (ntfy sendet immer zuerst `open`) statt bei Antwort-Headern (Plan schrieb Reset nach subscribe vor) — sonst Endlosschleife mit 1 Hz und falschem „Verbunden“ — Kosten falls falsch: Status „Verbunden“ erscheint einen Moment später.
- Task 8: Ruling: Stream ohne since (keine Historie): `open`-Zeit setzt die erste watermark (observe_time + State) — ohne Replay gilt der Grund für „open verschiebt nichts“ nicht; schließt stillen Verlust bei Abbruch vor erstem Keepalive — Kosten falls falsch: keine.
- Task 8: Ruling: HTTP 429 → Retry-After (Sekunden) beachten, sonst backoff_max — schont Rate-Limits — Kosten falls falsch: längere Pause nach 429.
- Task 12: Ruling: Status-Zeitstempel nur bei Kategoriewechsel (verbunden / getrennt / Auth-Fehler), nicht bei jedem Retry-Zyklus (präzisiert Note 3) — sonst erschiene „Verbindungsproblem“ nie — Kosten falls falsch: keine erkennbaren.
- Task 12: Ruling: Ctrl-C/SIGTERM-Handler nur bei enable_triggers — Tests installieren keine prozessweiten Handler — Kosten falls falsch: keine, Produktion unverändert.
- Task 12: Ruling: Medien laden + show in eine FIFO-Worker-Task auslagern; bei Quit Worker-Kanal schließen und ausstehende Benachrichtigungen ohne Medien zeigen (begrenzt) — keine Verluste, keine Blockade — Kosten falls falsch: bei Quit fehlen bei wenigen Benachrichtigungen die Bilder.
- Task 12: Ruling: Absturz zwischen gespeichertem State und Anzeige (Medien-Worker bis ~10 s) kann eine Benachrichtigung verlieren — akzeptiert, nur bei Prozessabsturz, normales Beenden zeigt alles — Kosten falls falsch: seltener Verlust bei Crash; Abhilfe wäre State erst nach Anzeige zu bestätigen.
- Task 13: Ruling: Time-Sensitive-Entitlement nur bei gesetzter MACOS_SIGN_IDENTITY — ad-hoc-signierte Bundles mit eingeschränkten Entitlements startet macOS nicht (exit 137) — Kosten falls falsch: Urgent fällt im Dev-Build auf „active“ zurück; echte Signatur/Profil in Teil 3.
- Ruling: Reihenfolge getauscht — Task 15 vor Task 14, weil Task 14 auf die rustup-Entscheidung des Auftraggebers wartet und Task 15 auf macOS vollständig prüfbar ist — Kosten falls falsch: keine, beide ändern main.rs nur additiv.
- Task 15: Ruling: config.yaml wird auch importiert, wenn settings.json existiert aber keine Instanzen hat — die Engine schreibt beim Beenden eine leere settings.json, sonst wäre der Hinweis „Nicht eingerichtet – config.yaml anlegen“ wirkungslos — Kosten falls falsch: keine.
- Task 14: Ruling: COM-Factory auf eigenem geparkten MTA-Thread statt Main-Thread (weicht von Note 4 ab) — tao setzt COM auf dem Main-Thread erst beim Fenster an; STA bräuchte Message-Loop, MTA auf Main würde tao in Teil 2 crashen — Kosten falls falsch: ein Thread (256 KB Stack) mehr.
- Ruling: Ein Fix-Durchlauf für die Final-Review-Punkte (Important 1–3 + Minor 1,3,4,5,6,7); Windows-CI-Lauf braucht ein GitHub-Remote → bleibt beim Auftraggeber — Kosten falls falsch: Windows-Testlaufzeitprobleme fallen erst dort auf.
- Ruling: Fehlen beim Re-Import die Zugangsdaten einer Instanz, die bereits Auth hat, bleiben gespeichertes Secret und AuthKind erhalten; Entfernen nur mit explizitem `auth: none` — der bereinigte config.imported.yaml-Stand muss gefahrlos re-importierbar sein — Kosten falls falsch: Auth-Entfernung braucht eine explizite Zeile.
- Ruling: Release-Workflow liefert macOS .dmg (universal) und Windows .zip (x64/arm64); MSI folgt mit dem Installer-Design in Teil 3 — Spec 5.8 nennt .msi, aber Installer-Details (Verknüpfung mit AUMID, Autostart) sind Teil 3 — Kosten falls falsch: MSI kommt später.
- Ruling: Time-Sensitive-Entitlement im Release nur mit Provisioning-Profil-Secret — eingeschränktes Entitlement ohne Profil verhindert den App-Start — Kosten falls falsch: Urgent fällt ohne Profil auf active zurück.
- Ruling: Beide Rest-Befunde werden trotz „kein zweiter Fix-Durchlauf“ behoben — Fix ist 3 Zeilen, der Auftraggeber hat ohnehin neue Arbeit (Release-Workflow) angefordert, und ein falsches Passwort unter neuem Benutzernamen ist ein echter Fehler — Kosten falls falsch: ein kleiner zusätzlicher Commit ohne eigenen Volltest-Review (wird im Release-Workflow-Review mitgeprüft).
- Ruling: Entitlements im Profil-Pfad werden zur Signierzeit aus dem Provisioning-Profil extrahiert (security cms -D | plutil -extract Entitlements) und die application-identifier gegen die Bundle-ID geprüft — passt automatisch zu Team/Profil — Kosten falls falsch: Profil-Pfad schlägt beim Signieren laut fehl statt still.

## Zurückgestellte Kleinigkeiten (für Teil 2/3)

- Task 1: minor (deferred): COM-arg filter also strips values of --config/--import (cli.rs)
- Task 1: minor (deferred): ensure_dirs/platform() untested
- Task 1: minor (deferred): .gitignore cargo-appended blank lines/comment
- Task 1: minor (deferred): CI without pinned toolchain/cache
- Task 2: minor (deferred): corrupt-backup name 1s resolution could overwrite earlier backup (store.rs)
- Task 2: minor (deferred): State::default().version == 0 vs Settings 1
- Task 2: minor (deferred): stale .tmp on failed write; no dir fsync
- Task 2: minor (deferred): save_is_atomic test weak (plan-mandated assertion)
- Task 2: minor (deferred): redundant field-level serde default for cache_duration_secs
- Task 3: minor (deferred): KeyringStore should be constructed once and shared as Arc<dyn SecretStore>
- Task 4: minor (deferred): failure after settings saved leaves plaintext original + returns Err — caller (Task 15) must surface error clearly
- Task 4: minor (deferred): partial secret writes on multi-instance re-import failure -> keychain/settings inconsistent until retry
- Task 4: minor (deferred): duplicate-URL check is case/port sensitive
- Task 4: minor (deferred): several rejects_* tests only assert is_err(); whitespace-only token accepted
- Task 5: minor (deferred): float priority 4.0 -> 3; attachment name may be empty for trailing-slash URL; Attachment serde uses `mime` not wire `type` (local only)
- Task 6: minor (deferred): literal "attachment" fallback bypasses i18n (controller clarification caused it)
- Task 6: minor (deferred): title Some("") not falling back to topic; percent-encoded filenames; gen_emoji fetches master (unpinned); LogNotifier eprintln until tracing
- Task 7: parked — sentinel per instance while gap flags per topic; removed topic of last_message → one spurious gap (rebaseline prevents repeats) — Ruling: real, harmless, deferred.
- Task 7: minor (deferred): rebaseline warning log for missing Date header belongs to Task 8
- Task 8: minor (deferred): poll body buffered whole (up to 10 MB/topic) + fixed 30 s total timeout — check in Task 15 memory measurement
- Task 8: minor (deferred): http_client() panics on TLS init failure; no X-Messages-Truncated check on stream; no system-proxy; no tracing subscriber until Task 15; ConnStatus::Disconnected{error} raw English text — must only be logged, not shown (Task 10/15)
- Task 9: minor (deferred): TempFile Drop deletes path after commit (concurrent same-stem race); stem collisions a/b vs a_b; default redirect policy (same-origin redirect outside base path keeps auth, LAN hops); no timeout/svg/subpath-auth tests; blocking std::fs in async
- Task 10: minor (deferred): interleaved multi-topic batch → recent ordered by topic group not arrival; no paused-bundle test; set_topic_muted cosmetic
- Task 12: minor (deferred): in-flight HTTP actions dropped on Quit; no engine test for Gap→show and media base_url/auth; action_failed id ms collisions; real-time test windows may flake on CI; main thread must not hang joining runtime (Task 15: don't join or shutdown_timeout)
- Task 12: minor (deferred): head-of-line blocking in delivery worker under tarpit (N×10 s); flush deadline 2 s exceeded → notifications lost on graceful quit after big backlog; net changes during 60 s watcher retry gap missed; if-watch macOS CFRunLoop thread per re-create; worker panic in notifier.show → silent loss afterwards
- Task 13: minor (deferred): narrow race when category already known but concurrent registration in flight
- Task 15: minor (deferred): log guard not dropped on macOS terminate path (last log lines lost); re-import edge-case wrong message; chmod test fails as root; --import while running silently dropped (should warn); icon tests cover 2 shapes; engine failure not visible in tray; Windows tray creation may fail at login before Explorer ready → app without UI
- Task 14: minor (deferred): raw file:/// URIs ('#','%' in profile path) — verify manually; no IconUri in AUMID key (Teil 3); HTTP headers persisted in Windows toast DB (privacy note Teil 3); show loop untested on Windows
- Release work: fix round 1/5 (all addressed; 4 minors open: app-id suffix check, profile-ignored warning not shown in CI, dead rc capture, temp-file leaks on failure) — complete (commits 38ed4a9..c223209)

## Offen für manuelle Prüfung

- Task 13: manual verification pending for user: banners visible, banner/button click activation, Focus/DND behaviour
- Task 13: complete (commits fda13d6..309e0d0, review clean; GUI checks pending for user — permission must be enabled in System Settings)
- Task 15: pending for user: banners/click-through, tray appearance + Quit menu, sleep/wake, Windows run
- Task 14: complete (commits 8442cee..0baa7cb, review clean; Windows runtime pending for user)
