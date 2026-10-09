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

## 1a. Doppelte Ordner (Phase 7.1)

- [ ] Echter Ordner mit einer Kopie („Kopie von …“ oder „… (2)“): `plan dedupe-dirs` wählt die Kopie zum Entfernen, der Originalordner bleibt; Review, Apply und `undo` stellen alles her (Pfade, Inhalt, Zeiten).
- [ ] Namens-Heuristik an echten Ordnern prüfen: Fehltreffer (z. B. „Altbau“, „Backup-Strategie“) notieren; Muster ggf. in `dedupe_dirs_copy_patterns` ergänzen oder die Liste in `analysis/folder_dups.rs` anpassen.
- [ ] OneDrive-Ordner mit einer Cloud-only-Datei in einer der Kopien: der Ordner taucht nicht als Aktion auf, es startet kein Download.
- [ ] Bericht: Abschnitt „Ordner-Duplikate“ zeigt die Gruppen und teilweise gleiche Ordner plausibel (Schwelle in den Einstellungen ändern und neu bauen).
- [ ] GUI: Karte „Doppelte Ordner“ steht vor „Duplikate“, zeigt die Zahl nach dem Scan, Review zeigt Dateianzahl und behaltenen Ordner; Teilauswahl anwenden und zurückdrehen.
- [ ] Laufzeit: `plan dedupe-dirs` auf einem großen echten Index (> 500.000 Dateien) bleibt unter etwa 15 s.

## 1b. Plan-Integrität (Phase 7.0)

- [ ] Plan erzeugen, in Notepad öffnen und unverändert speichern (auch „Speichern unter“ mit CRLF): `apply` läuft ohne Fehlalarm durch.
- [ ] Plan in Notepad ändern (einen Wert oder eine Aktion löschen): `apply` bricht mit „Plan wurde nach dem Erstellen verändert“ ab, `$LASTEXITCODE` ist `3`, `runs` zeigt keinen neuen Lauf, im Ordner hat sich nichts geändert.
- [ ] Danach `plan seal <plan.json>`: Übersicht stimmt (Aktionen, alte und neue Prüfsumme), „N“ lässt die Datei unverändert (Exit-Code `1`), „j“ versiegelt; `apply` läuft wieder durch.
- [ ] GUI: Plan öffnen, Datei extern in Notepad ändern, „Anwenden“ drücken: Dialog „Plan wurde verändert“ mit „Plan neu laden“, nichts ausgeführt; „Plan neu laden“ zeigt den geänderten Plan bzw. eine Fehlermeldung.
- [ ] Alter Plan aus einem früheren Release (Format 2): `apply` und GUI zeigen „Ungeschützter Plan (Format 2) …“, der Lauf geht weiter.

## 1c. Quarantäne-Ansicht (Phase 7.4)

- [ ] Nach einem echten Aufräumlauf (Duplikate, Müll) zeigt die Seite „Quarantäne“ (`Strg+6`, Seitenleiste) genau die verschobenen Dateien: Name, ursprünglicher Ort, Größe, Lauf, „Läuft ab“, Status „in Quarantäne“. Summenzeile (Anzahl, Größe, nächster Ablauf) stimmt mit „Verlauf“ überein.
- [ ] Optik prüfen: hell und dunkel, 100 % und 150 % Skalierung, schmales Fenster (Spalten werden abgeschnitten, nichts überlappt), Tabelle scrollt flüssig.
- [ ] Suche (Name und Pfadteil, Groß-/Kleinschreibung egal), Wurzel-Auswahl und Klick auf die Spaltenköpfe (Name, Größe, Lauf, Läuft ab; zweiter Klick kehrt um).
- [ ] Eine Datei auswählen, **Zurückholen …**: Rückfrage nennt das Ziel; nach „Zurückholen“ liegt die Datei am alten Ort, der Eintrag verschwindet aus der Liste, „Verlauf“ zeigt die geringere Größe und unveränderten Status „Vollständig“.
- [ ] Ursprungspfad vorher von Hand neu belegen (andere Datei mit demselben Namen): Rückfrage zeigt „→ name (2).ext“, die vorhandene Datei bleibt unverändert, die zurückgeholte hat den neuen Namen. In „Verlauf“ → Details steht „zurückgeholt als …“.
- [ ] Ordner aus „Doppelte Ordner“ zurückholen: Der Ordner kommt als Ganzes zurück, in der Liste steht „(Ordner)“ und im Tooltip die Dateianzahl.
- [ ] Datei in der Quarantäne im Explorer verändern, dann zurückholen: Die Rückfrage warnt („wurde in der Quarantäne verändert“), nur bei Dateien aus „Duplikate“.
- [ ] Datei in der Quarantäne im Explorer löschen und „Neu laden“ (`Strg+R`): Eintrag zeigt „fehlt“ (Warnfarbe), ist nicht auswählbar, der Hinweis über der Liste nennt die Zahl.
- [ ] Rechtsklick auf den Ort: „Im Explorer zeigen“ öffnet den Ablageort **in der Quarantäne**; „Pfad kopieren“ kopiert ihn.
- [ ] „Verlauf“ → **Inhalt ansehen** bei einem Lauf: Sprung auf „Quarantäne“, Filter-Etikett „Nur Lauf …“ (Klick entfernt den Filter); bei einem Lauf ohne Quarantäne-Inhalt oder nach „Quarantäne leeren“ ist der Knopf deaktiviert.
- [ ] Nach teilweisem Zurückholen: „Rückgängig“ des Laufs holt den Rest, die schon zurückgeholten Dateien bleiben unangetastet (auch die mit „(2)“).
- [ ] Echte Cloud-only-Datei (OneDrive „Nur online“) in der Quarantäne: Status „nur online“, Zurückholen lädt nichts herunter (Datei bleibt „Nur online“), Rückfrage nennt den Hinweis.
- [ ] `F1` auf der Seite öffnet den Hilfetext „Quarantäne“; `?` zeigt „Strg+1 … Strg+6“; während ein Scan oder Apply läuft, bleibt die Liste bedienbar, „Zurückholen …“ meldet bei laufendem Schreib-Task eine Meldung statt zu starten.
- [ ] Mit vielen Einträgen (z. B. `purge`-Frist verlängern und mehrere Läufe sammeln) bleibt die Seite flüssig; Ladezeit bei sehr vielen Journalen notieren (Messung: `tests/perf_quarantine.rs`).

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

## 5c. GUI-Bedienung, Stufe 3 (Aufräumen mit Karten)

- [ ] Aufräumen zeigt sechs Karten (3 × 2), gleich hoch, die gewählte mit Akzentrahmen; ein Klick irgendwo auf die Karte (auch auf den Text) wählt sie und zeigt die Optionen darunter.
- [ ] Gescannter Ordner: Duplikate „37 Gruppen, 4,2 GB könnten frei werden“, Müll „1.204 Dateien, 860 MB“, Leere Ordner „58 Ordner“ stimmen mit Analyse → Übersicht überein; Archivieren, Versionen und Nach Regeln zeigen keine Zahl.
- [ ] Ungescannter Ordner: Hinweis „Keine Zahlen: Dieser Ordner ist noch nicht gescannt.“, Karten nur mit Beschreibung.
- [ ] Scan älter als 7 Tage (Systemuhr vorstellen): Hinweis „Stand: vor n Tagen, neu scannen?“ in Warnfarbe. Nach neuem Scan verschwindet er und die Zahlen ändern sich ohne Neuladen.
- [ ] Archivieren / Versionen: Zahlfeld plus Einheit (Tage / Monate / Jahre); Platzhalter zeigt den Config-Wert („Standard: 2 Jahre“, „Standard: 30 Tage“); leer erzeugt den Plan mit dem Config-Wert, „2“ + Jahre erzeugt denselben Plan wie `plan archive --older-than 2y`; „abc“ ergibt eine Meldung statt eines Plans.
- [ ] Profil als Ziel: Platzhalter zeigen die Werte aus dem Profil, nicht die globalen.
- [ ] Duplikate: Dropdown mit „Die älteste / neueste Datei bleibt / Die Kopie in einem bestimmten Ordner bleibt“; bei der dritten Variante „Ordner wählen …“ öffnet den Dialog, Pfad gekürzt mit Tooltip.
- [ ] Nach Regeln: Regeldatei wählen, Regelnamen erscheinen als Häkchen; nur angehakte Regeln landen im Plan (Kopf des Plans prüfen); alle abwählen gibt eine Meldung. Datei mit Syntaxfehler: roter Text mit Datei und Zeile, kein Absturz.
- [ ] Zuletzt erzeugte Pläne: Zeile mit relativer Zeit, Art, „n Aktionen, Größe“ und gekürztem Wurzelpfad; Klick öffnet den Plan; eine fremde `.json` im Planordner erscheint nur mit Dateinamen.
- [ ] Fenster schmal ziehen: Karten rücken auf weniger Spalten, nichts wird abgeschnitten.
- [ ] Fenster auf linke und rechte Bildschirmhälfte: Karten füllen die Breite, rechts ist derselbe Rand wie links.
- [ ] Fenster niedrig ziehen: die Seite scrollt, alle Optionen und „Plan erzeugen“ sind erreichbar.
- [ ] Versionen-Plan anwenden, „Weiter“ (Ergebnis schließen): die Karten sind wieder da.

## 5d. GUI-Bedienung, Stufe 4 (Review, Tastatur, Theme)

- [ ] Einstellungen → „Darstellung“: Wie Windows / Hell / Dunkel wirken sofort und bleiben nach Neustart. „Wie Windows“ folgt dem Windows-Modus; Windows zur Laufzeit umstellen (Einstellungen → Personalisierung → Farben): die GUI zieht mit. Falls nicht, ist das der Rückfall aus dem Plan (Registry `AppsUseLightTheme`), bitte melden.
- [ ] Hell und Dunkel: Fehler (rot), Warnungen (gelb/braun), „Gültig“ (grün), Status-Texte und Score-Farben sind in beiden Modi lesbar; der Gefahr-Knopf (Quarantäne leeren, Löschen) hat in beiden Modi weißen Text auf dunkelrotem Grund.
- [ ] DPI 100 %, 150 %, 200 %: Karten, Kopfleiste, Review-Tabelle, Seitenleiste ohne abgeschnittenen Text; Symbole in der Seitenleiste scharf.
- [ ] Windows 10: Seitenleisten-Symbole (⌂ ◔ ✂ ↺ ⚙) erscheinen. Fehlt eines (leeres Kästchen), notieren; ohne `seguisym.ttf` erscheint die Seitenleiste ohne Symbole.
- [ ] Review: Kopfzeile zeigt „Anwenden (N Einträge, X GB)“, bei Auswahl 0 ist der Knopf grau. Darunter steht „Nichts wird gelöscht. Dateien gehen in die Quarantäne und lassen sich n Tage lang zurückholen.“ mit den Tagen aus der Config (`quarantine_days` ändern und Plan neu öffnen). Bei Archiv-/Regel-Plänen lautet der Satz „… werden verschoben …“, bei „Leere Ordner“ „Entfernt werden nur leere Ordner …“.
- [ ] Review-Tabelle: zwischen Größe, Grund / Regel und Alter sind Trennlinien und Abstand sichtbar, in Hell und Dunkel; Scrollen durch 110.000 Zeilen ruckelt nicht stärker als vorher.
- [ ] Einstellungen → Regeln: Name, Ziel und die Felder darunter beginnen in derselben Spalte und enden bündig.
- [ ] Review: Menü „Auswahl ▾“ (Alle/Keine/Gefilterte/Ordner), „Übersprungene zeigen (n)“ mit richtiger Zahl; Größen rechtsbündig.
- [ ] Tastatur im Review: ↑/↓ wechseln die Zeile und scrollen mit, Leertaste wählt an/ab und springt weiter (beim Abwählen mit abhängigen Einträgen erscheint die Rückfrage), Strg+A / Strg+Umschalt+A, Strg+F fokussiert die Suche, Strg+Enter öffnet nur den Bestätigungsdialog „Anwenden“ (nichts wird ohne Dialog angewendet).
- [ ] Tastatur global: Strg+1 bis Strg+5 wechseln die Seite, Strg+O öffnet den Plan-Dialog, Strg+R lädt neu, `?` öffnet die Kürzel-Liste, Esc schließt sie. Esc in einem Dialog bricht ab (bestätigt nie); während eines Dialogs wirken die Kürzel nicht.
- [ ] In einem Textfeld getippt (Suche, Ordner, Zahlen): Leertaste, Buchstaben und Pfeiltasten gehen ins Feld und lösen kein Kürzel aus.
- [ ] Tooltips: Seitenleiste „Strg+n“, Anwenden-Knopf „Strg+Enter“.

## 5e. GUI-Hilfe

Spec: [`features/gui-hilfe.md`](features/gui-hilfe.md). Mit `cargo build --release` bauen, die exe-Dateien samt `README.md` und `docs\dokumentation.md` in einen Wegwerf-Ordner kopieren (README daneben, Dokumentation im Unterordner `docs`) und die GUI von dort starten.

- [x] Jede Seite (Übersicht, Analyse, Aufräumen, Verlauf, Einstellungen) hat neben der Überschrift ein „?“ mit Tooltip „Hilfe zu … (F1)“; ein Klick öffnet das Hilfefenster mit dem Kurztext der Seite.
- [x] Aufräumen: Jede der sechs Karten hat oben rechts ein „?“. Ein Klick darauf öffnet den Text dieser Karte und ändert die Auswahl der Karte nicht. Ein Klick auf den Rest der Karte wählt sie weiterhin.
- [x] `F1` öffnet die Hilfe zur aktuellen Seite, auch wenn der Fokus in einem Textfeld steht (zum Beispiel in der Suche). In der Review-Ansicht öffnet `F1` den Text „Plan prüfen“, ebenso das „?“ neben der Überschrift des Plans. Bei offenem Dialog tut `F1` nichts.
- [x] Seitenleiste: „? Hilfe“ öffnet die Hilfe zur aktuellen Seite, „? Kürzel“ und `?` öffnen den Reiter „Tastenkürzel“ (mit der Zeile `F1`); ein zweiter Druck schließt ihn wieder.
- [x] Reiter „Erklärung“ und „Tastenkürzel“ wechseln im selben Fenster; der Text ist ohne Befehlszeilen-Kenntnis verständlich (mit einer Person aus dem Nutzertest gegenlesen) und stimmt mit dem überein, was die Funktion tut.
- [x] „Ausführliche Doku öffnen“: öffnet die Datei im Standardprogramm; ohne Programm für `.md` öffnet sich Notepad. Der Kurztext nennt den Abschnitt, in dem die Erklärung steht.
- [x] `docs`-Ordner löschen und den Knopf erneut klicken: rote Meldung mit dem erwarteten Pfad, „Pfad kopieren“ legt ihn in die Zwischenablage, der Kurztext bleibt lesbar, kein Absturz.
- [x] Hilfe offen, während ein Scan läuft: Fortschritt und „Abbrechen“ in der Statuszeile funktionieren weiter, der Scan läuft ungestört.
- [x] Hilfe und ein Bestätigungsdialog („Anwenden“) gleichzeitig offen: das erste `Esc` schließt nur die Hilfe, das zweite den Dialog (ohne zu bestätigen).
- [ ] Hell und dunkel, DPI 100/150/200 %, schmales und niedriges Fenster: Text umbricht, das Fenster scrollt, „Schließen“ (Kreuz) bleibt erreichbar, nichts ist abgeschnitten. Geprüft ohne DPI-Stufen (offen: DPI 100/150/200 %).
- [x] `netstat -ano` während die Hilfe benutzt wird: keine Verbindung des Prozesses.

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

