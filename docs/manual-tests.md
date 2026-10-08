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

- [x] Start per Doppelklick ohne Konsolenfenster.
- [x] Scan von rund 200.000 Dateien, Abbrechen mittendrin: Fenster bleibt bedienbar, Abbruch wirkt zügig.
- [x] Fenster schließen während Scan: Rückfrage, danach kein hängender Prozess. Gleiches während Apply (Lauf wird sauber beendet, Undo möglich).
- [ ] Review-Liste mit 500 Einträgen in unter 5 Minuten sichten (nur mit echten Daten sinnvoll, offen).
- [x] 20 Einträge abwählen und anwenden (Teilplan, Original-Plan bleibt unverändert): geprüft mit 11 abgewählten Einträgen, 98 von 109 angewendet, Original-Plan unverändert.
- [x] Undo aus der GUI und aus der CLI.
- [ ] Purge über „Quarantäne leeren“ (GUI) und `purge --older-than`: nur mit künstlich gealtertem `started` im Journal geprüft, nicht über den normalen Weg. Offen, bis ein frischer Lauf ohne Eingriff geleert werden kann (siehe Auffälligkeit 2).
- [ ] 100.000 Zeilen im Review-Fenster: Scrollen ohne Ruckeln. Mit 110.000 Zeilen in Ordnung: Laden, Suche, Alle abwählen. Leichtes Ruckeln beim schnellen Ziehen des Scrollbalkens, das Kriterium „ohne Ruckeln“ ist damit nicht erfüllt. Entscheiden, ob das akzeptiert wird (dann abhaken und das Kriterium umformulieren), siehe Auffälligkeit 4.
- [ ] DPI 150 % und 200 %: Text und Tabellen nicht abgeschnitten.
- [ ] VM und Remotedesktop: GUI startet (wgpu oder Rückfall glow), sonst erscheint die MessageBox mit Hinweis auf die CLI.
- [x] `netstat -ano` während die GUI läuft: kein lauschender Port, keine Verbindung des Prozesses (90 s lang alle 2 s gemessen, TCP und UDP, keine Kindprozesse).

## 5. GUI Teil B (Stufe 2)

- [ ] Regel per Formular anlegen, Vorschau prüfen, speichern, `plan rules` in der CLI nutzt sie.
- [ ] Roh-TOML mit absichtlichem Fehler: Fehlerzeile markiert, Speichern gesperrt.
- [ ] Config extern ändern, während sie in der GUI offen ist: Konflikthinweis beim Speichern, `.bak` vorhanden.
- [ ] Profil und Zeitplan aus der GUI anlegen, `schedule list` in der CLI zeigt ihn.
- [ ] Vorlage kopieren und bearbeiten.
- [ ] Verknüpfung im Startmenü anlegen, Start darüber.

## 5a. GUI-Bedienung, Stufe 1 (Kopfleiste)

Spec: [`features/gui-bedienung.md`](features/gui-bedienung.md). Mit einem frischen Datenordner (`ORDNER_CLEANUP_HOME`) und danach mit dem echten starten.

- [ ] Frischer Datenordner: Übersicht zeigt „Noch nichts gescannt“ mit dem Knopf „Ordner wählen und scannen“; der Knopf öffnet den Ordnerdialog und startet nach der Wahl den Scan.
- [ ] Kopfleiste ist auf Übersicht, Analyse, Aufräumen und Verlauf sichtbar, auf den Einstellungen nicht.
- [ ] Dropdown: zuletzt gewählte Ordner stehen oben, danach die gescannten Wurzeln ohne Dubletten (auch mit anderer Groß-/Kleinschreibung), Profile in einer eigenen Gruppe. Nach Auswahl eines Profils zeigt der Kopf den Profilnamen.
- [ ] Mehr als 8 verschiedene Ordner nacheinander wählen: die Liste „zuletzt gewählt“ hält höchstens 8. Nach Neustart ist die Auswahl wieder da.
- [ ] Scan aus der Kopfleiste: Fortschritt mit „Abbrechen“ im Kopf, Seitenwechsel während des Scans, danach Ergebnistext, Übersicht lädt neu.
- [ ] Ordner umbenennen oder Laufwerk trennen, während er gewählt ist: Warnsymbol im Dropdown, „Ordner nicht gefunden“, Scannen und Klassifizieren gesperrt; die Oberfläche hängt nicht (auch bei getrenntem Netzlaufwerk).
- [ ] „Optionen …“: zwei Abschnitte (Scannen, Klassifizieren); alle bisherigen Optionen sind da; „Text-Cache löschen“ schließt den Dialog und läuft.
- [ ] Zweiter Scan (Geplanter Lauf oder zweites Fenster) während ein Scan läuft: Meldung „Ein anderer Scan läuft“, „Erneut versuchen“ funktioniert.
- [ ] Fenster mit gewähltem Ziel schließen und neu öffnen, mit einem in der Config entfernten Profil als gemerktem Ziel: kein Fehlerdialog, das Profil fehlt in der Liste.

## 5b. GUI-Bedienung, Stufe 2 (lesbare Ausgaben)

- [ ] Nach einem Scan zeigt die Kopfleiste Kacheln (Dateien · Ordner · Größe · Fehler · Score) statt eines Textblocks; „Score“ trägt die Einordnung („72 – mittel“) in Ampelfarbe; „Schließen“ blendet das Ergebnis aus.
- [ ] Scan mit Fehlern (z. B. gesperrte Datei): die Kachel „Fehler“ ist anklickbar und springt in „Analyse → Probleme & Struktur“.
- [ ] Abgebrochener Scan: Titel „Scan abgebrochen“, kein Score, Hinweis „unvollständig“.
- [ ] Klassifizieren: Kacheln (Geprüft · Analysiert · Aus dem Cache · Zum Prüfen · Übersprungen), „Details“ klappt den bisherigen Text auf; „Text-Cache löschen“ zeigt „Text-Cache geleert“.
- [ ] Übersicht, gescannte Ordner: kein Debug-Text mehr (kein „Complete“), Status mit Symbol („✔ Vollständig“, „⚠ Scan unvollständig, 3 Fehler“), Zeit relativ mit genauem Wert im Tooltip, lange Pfade in der Mitte gekürzt (Tooltip voller Pfad, Rechtsklick kopiert).
- [ ] Übersicht, Letzte Läufe: Tabelle mit Wann · Ordner · Was · Ergebnis · Größe; Lauf-ID nur im Tooltip; „Rückgängig …“ dort fragt nach, dreht den Lauf zurück und ist bei zurückgedrehten oder geleerten Läufen gesperrt.
- [ ] Übersicht, Quarantäne: „4,2 GB in 3 Läufen warten auf Löschung“ mit Knopf „Quarantäne leeren …“ (springt in den Verlauf); ohne offene Quarantäne der Satz „Keine Quarantäne wartet auf Löschung“.
- [ ] Profile: „Öffnen“ setzt die Kopfleiste; „n Dateien zum Prüfen“ ist anklickbar und öffnet „Analyse → Inhalte“ für das Profil.
- [ ] Verlauf: dieselbe Lauf-Tabelle plus „Läuft ab“, „Details“ und „Rückgängig …“; Index-Wurzeln mit Status statt `Complete`.
- [ ] Uhr des PCs um einen Tag vorstellen und einen Lauf anlegen: keine negativen oder „vor …“-Texte, sondern das Datum.

## 6. Nutzertest

- [ ] Person ohne CLI-Kenntnisse: „Duplikate in Ordner X finden, prüfen, in die Quarantäne verschieben und zurückholen“, ohne Hilfe. Stolpersteine hier notieren:

## Ergebnisse

Datum, Rechner, Auffälligkeiten (Fehler als Bug-Analyse unter `docs/bugs/` anlegen):

### 05.10.2026, Windows 11 Pro (10.0.26300), Release-Build, GUI Teil A (teilweise)

Geprüft: Scan und Abbruch (200.000 Dateien), Schließen während Scan und Apply, Teilplan, Undo (GUI und CLI), Purge, `netstat`, Review mit 110.000 Zeilen. Doppelklick-Start ohne Konsolenfenster: in Ordnung. Offen in Teil A: Review-Sichtung mit echten Daten, DPI 150 % und 200 %, VM/RDP.

Auffälligkeiten:

1. **Fehler:** „Zuletzt erzeugte Pläne“ (Aufräumen) zeigt Pläne aus dem GUI-Ordner (`data\plans\_gui`) doppelt; Pläne direkt in `data\plans` und in `plans\<profil>` erscheinen einmal. Ursache: `plan_dirs()` liefert `data\plans\_gui` und `data\plans`, `recent_plans()` liest je Ordner auch eine Ebene Unterordner, die Liste wird nicht entdoppelt (`src/gui/views/cleanup.rs`). Analyse und Fix: `docs/bugs/recent-plans-duplicates-analysis.md`, `…-fix-plan.md` (behoben).
2. **Verbesserungsidee:** „Quarantäne leeren“ und `purge --older-than` lehnen `0d` ab (`parse_old_after` in `src/analysis/age.rs` verlangt > 0); direkt nach dem Apply lässt sich die Quarantäne nicht leeren. Test ging nur mit künstlich zurückgesetztem `started` im Journal; der Purge-Punkt in Abschnitt 4 bleibt deshalb offen.
3. **Verbesserungsidee:** Review zeigt bei aktivem Filter keine Trefferzahl („Ausgewählt: N von M“ zählt den ganzen Plan); ein Suchtext, der schon die obersten Zeilen trifft, wirkt wie ein Fehler.
4. **Beobachtung:** Bei 110.000 Zeilen leichtes Ruckeln beim schnellen Ziehen des Scrollbalkens, keine auffällige CPU-Last, Mausrad unauffällig.
5. **Bedienung:** Beim ersten Teilplan-Test wurde nichts abgewählt (109 von 109 angewendet); kein Fehler.
6. **Build:** Beim ersten `cargo build --release` stürzte rustc beim Kompilieren von `ordner-cleanup-bg` mit `STATUS_HEAP_CORRUPTION` ab, der zweite Lauf war erfolgreich (Compiler-Absturz, kein Codefehler). Ein Heap-Absturz kann auch auf defekten Arbeitsspeicher hindeuten; als unauffällig gilt er erst nach Nachprüfung. In der Folgesitzung am selben Tag liefen auf demselben Rechner viele Builds und rund 1.500 Tests ohne erneuten Absturz. Offen: einmal den Windows-Speicherdiagnose-Test (`mdsched`) laufen lassen, bis dahin die übrigen Beobachtungen dieses Eintrags mit Vorbehalt lesen. Am selben Tag brach außerdem einmal `cargo clippy` (clippy-driver, Exit-Code 101, Meldung nicht erfasst) ab; drei Wiederholungen liefen sauber. Zwei sporadische, nicht reproduzierbare Abstürze von Rust-Werkzeugen auf einem Rechner sprechen eher für ein Hardware- oder Umgebungsproblem als für einen Codefehler. Zeigt sich ein Absturz erneut, Meldung sichern und Bug-Analyse unter `docs/bugs/` anlegen.

