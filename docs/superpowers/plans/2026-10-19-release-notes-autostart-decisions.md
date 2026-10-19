# Release Notes und Autostart – Entscheidungen und offene Punkte

Aus dem Umsetzungs-Ledger (Subagent-Driven Development, 2026-10-19). Ergänzt Design und Plan; bei Widerspruch gelten diese Entscheidungen.

## Entscheidungen (Rulings)

- Ruling: Feature-Branch im Haupt-Checkout wie bei Teil 2 — main bleibt unberührt — Kosten falls falsch: keine.
- Ruling R1: `Wizard::on_save_result` gibt künftig `Vec<UiRequest>` zurück (Urgent und Autostart können beide nötig sein); Aufrufer leiten alle weiter — Kosten falls falsch: keine.
- Ruling: Commit-Eingabeformat wird auf NUL-getrennte Datensätze umgestellt (`git log -z --no-merges --format='%H%x1f%B'`, Lesen mit `read -d ''`) — ein \x1e im Commit-Text konnte Datensätze zerteilen; Git kann kein NUL speichern — umgesetzt in Task 2 für beide Skripte — Kosten falls falsch: keine.
- Ruling: Der Brief-Satz „vorhandene Plist → Enabled“ wird durch die globale Regel „im OS deaktiviert → aus“ ersetzt: LaunchAgent-Zustand über `SMAppService::statusForLegacyURL`; Rückfall nur bei Signatur-Fehler (kSMErrorInvalidSignature) bzw. NotFound, nicht bei Ablehnung durch den Nutzer — Spec-Anti-Ziel wiegt schwerer als der Brief — Kosten falls falsch: auf manchen Systemen kein LaunchAgent-Rückfall, dann Hinweis statt Autostart.
- Ruling: choose_mechanism-Signatur geändert (SmError mit Code/Status, Rückgabe Result) akzeptiert — nötig für die Fehlercode-Unterscheidung — Kosten falls falsch: keine.
- Ruling: NeedsApproval wird als „aus“ mit dem Bestätigungs-Hinweis angezeigt (Abweichung vom Brief „NeedsApproval → an“) und ein im Assistenten abgewählter Autostart sendet auch bei NeedsApproval `false` — globale Regel „im OS deaktiviert → aus“ hat Vorrang — Kosten falls falsch: keine, macOS schaltet ohne Nutzer nie ein.
- Ruling: LaunchAgent-Rückfall bei Signaturfehler ODER Status NotFound ist korrekt (so in Task-3-Fix entschieden); README wird angepasst, nicht der Code — Kosten falls falsch: keine.
- Ruling: Ein Fix-Durchlauf für Minor 1–4 plus zwei billige PR-Titel-Verbesserungen (Revert-Titel erlauben, leere Beschreibung ablehnen) — kurz vor den manuellen Prüfungen — Kosten falls falsch: keine.
- Ruling: unberührte, abgewählte Option bei NeedsApproval sendet nichts (kein stilles Abmelden eines wartenden Anmeldeobjekts); neuer Text autostart_not_applied für abweichendes Ergebnis — Kosten falls falsch: keine.

## Zurückgestellte Kleinigkeiten

- Task 2: minor (deferred): prev-tag.sh duplicates tag loop; commits-lib header; check-commit-subject accepts whitespace-only description; default revert PR title rejected; leading "# " subject renders as heading; release job checkout persist-credentials; prev-tag exits 2 for pre-release tag
- Task 3: minor (deferred): plist present but legacy status NotRegistered/NotFound → shown Disabled although launchd may still load it; probe --launch-agent --enable can double-register
- Task 4: minor (deferred): home-path check case-sensitive on Windows; failure test starts from Disabled; DisabledByUser test waits on real 5 s tick
- Task 5: minor (deferred): Finish before first snapshot silently skips autostart
- Task 6: minor (deferred): step 32 quotes the LaunchAgent DisabledByUser hint; for an SM login item the shown hint is autostart_needs_approval
- Final fix wave: all addressed (commits 5e06e44..b66e426); residual minor: double-revert PR title rejected

## Offen für manuelle Prüfung

- README „Manuelle Prüfung Windows“, Schritte 25–37 (Windows-Registry zur Laufzeit, macOS-Anmeldeobjekte/LaunchAgent in den Systemeinstellungen, v0.2.0-Release-Notes).
