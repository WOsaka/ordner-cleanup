---
title: "Manuelle Tests (gesammelt)"
created: 2026-10-05
---

# Manuelle Tests

Alle Punkte, die in den Plänen noch offen sind und nur von Hand oder auf echter Hardware prüfbar sind. Pro Block: Ergebnis eintragen, danach den zugehörigen Plan auf `status: implemented` setzen (vorher `verification-before-completion`).

Vorbereitung: `cargo build --release`, dann `target\release\ordner-cleanup.exe` und `ordner-cleanup-gui.exe`. Für gefahrlose Läufe einen Testordner mit Kopien und einen eigenen Datenordner nutzen (`$env:ORDNER_CLEANUP_HOME = "D:\ordner-test-home"`).

## 1. Cloud-only und OneDrive (Phase 2, 3, GUI)

- [ ] OneDrive-Ordner mit mindestens einer echten Cloud-only-Datei (Platzhalter „Speicherplatz freigeben“) scannen: Datei wird weder gelesen noch gehasht, es startet kein Download.
- [ ] `plan dedupe`/`plan junk` auf diesem Ordner: Platzhalter tauchen nicht als Aktion auf.
- [ ] In der GUI „Öffnen“ auf eine Cloud-only-Datei: Meldung statt Download.
- [ ] OneDrive-Ordner mit mehr als der Obergrenze: Apply verlangt in der CLI `--allow-large`, in der GUI den Haken im Dialog.

## 2. Geplante Läufe (Phase 5)

- [ ] `schedule add <profil> --daily 09:00`, `schedule list` zeigt die Aufgabe, in der Windows-Aufgabenplanung ist sie sichtbar.
- [ ] Aufgabe manuell starten (`schtasks /run`): Lauf erscheint in `runs`/Übersicht, Bericht und Pläne unter `plans\<profil>`.
- [ ] Toast erscheint, wenn mehr neue Dateien „zum Prüfen“ da sind als beim letzten Lauf.
- [ ] Geplanter Lauf parallel zu einem GUI-Scan: GUI meldet „Ein anderer Scan läuft“, Lauf wird als übersprungen protokolliert.
- [ ] `schedule remove` räumt die Aufgabe weg.

## 3. Klassifikation (Phase 6a)

- [ ] `classify` auf einem echten Downloads-/Scans-Ordner mit PDFs, Office-Dateien, Fotos: Kategorien plausibel, „Zum Prüfen“ nicht absurd groß.
- [ ] OCR-Lauf auf einem Scan (`ocr-worker` startet, bricht bei Abbruch sauber ab).
- [ ] Lokales LLM an und aus (CLI und GUI „Verbindung testen“): aus = keine Netzverbindung, an = nur das konfigurierte lokale Ziel.
- [ ] `plan rules` nutzt die Kategorien; unsichere Dateien stehen als `skipped: low-confidence`.

## 4. GUI Teil A (Stufe 1)

- [ ] Start per Doppelklick ohne Konsolenfenster.
- [ ] Scan von rund 200.000 Dateien, Abbrechen mittendrin: Fenster bleibt bedienbar, Abbruch wirkt zügig.
- [ ] Fenster schließen während Scan: Rückfrage, danach kein hängender Prozess. Gleiches während Apply (Lauf wird sauber beendet, Undo möglich).
- [ ] Review-Liste mit 500 Einträgen in unter 5 Minuten sichten; 20 Einträge abwählen und anwenden (Teilplan, Original-Plan bleibt unverändert).
- [ ] Undo aus der GUI und aus der CLI; Purge über „Quarantäne leeren“.
- [ ] 100.000 Zeilen im Review-Fenster: Scrollen ohne Ruckeln.
- [ ] DPI 150 % und 200 %: Text und Tabellen nicht abgeschnitten.
- [ ] VM und Remotedesktop: GUI startet (wgpu oder Rückfall glow), sonst erscheint die MessageBox mit Hinweis auf die CLI.
- [ ] `netstat -ano` während die GUI läuft: kein lauschender Port, keine Verbindung des Prozesses.

## 5. GUI Teil B (Stufe 2)

- [ ] Regel per Formular anlegen, Vorschau prüfen, speichern, `plan rules` in der CLI nutzt sie.
- [ ] Roh-TOML mit absichtlichem Fehler: Fehlerzeile markiert, Speichern gesperrt.
- [ ] Config extern ändern, während sie in der GUI offen ist: Konflikthinweis beim Speichern, `.bak` vorhanden.
- [ ] Profil und Zeitplan aus der GUI anlegen, `schedule list` in der CLI zeigt ihn.
- [ ] Vorlage kopieren und bearbeiten.
- [ ] Verknüpfung im Startmenü anlegen, Start darüber.

## 6. Nutzertest

- [ ] Person ohne CLI-Kenntnisse: „Duplikate in Ordner X finden, prüfen, in die Quarantäne verschieben und zurückholen“, ohne Hilfe. Stolpersteine hier notieren:

## Ergebnisse

Datum, Rechner, Auffälligkeiten (Fehler als Bug-Analyse unter `docs/bugs/` anlegen):

