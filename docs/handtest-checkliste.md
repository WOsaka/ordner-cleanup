---
title: "Handtest-Checkliste vor Release 1.0"
created: 2026-10-13
---

# Handtest-Checkliste vor Release 1.0

Kurzfassung und Reihenfolge der offenen Punkte aus [`manual-tests.md`](manual-tests.md) (dort stehen die Details und die Abschnittsnummern). Pro Block abhaken, Ergebnis unter „Ergebnisse“ in `manual-tests.md` eintragen, danach den Plan auf `status: implemented` setzen (vorher `verification-before-completion`).

## 0. Vorbereitung (einmal)

- [x] `cargo build --release` (bei Compiler-Absturz `STATUS_HEAP_CORRUPTION` siehe Auffälligkeit 6: einmal `mdsched` laufen lassen)
- [x] Wegwerf-Datenordner: `$env:ORDNER_CLEANUP_HOME = "D:\ordner-test-home"`
- [x] Testordner mit Kopien echter Dateien (Duplikate, „Kopie von …“-Ordner, Müll, PDFs/Fotos)
- [x] OneDrive-Testordner mit mindestens einer Datei auf „Nur online“ (Rechtsklick → Speicherplatz freigeben)

## 1. Block A: Mit dem OneDrive-Testordner (größtes Risiko zuerst)

Abschnitt 1 und 1a in `manual-tests.md`.

- [x] Scan: Cloud-only-Datei wird nicht gelesen, kein Download startet
- [x] `plan dedupe` / `plan junk` / `plan dedupe-dirs`: Platzhalter erscheint nicht als Aktion
- [x] GUI „Öffnen“ auf Platzhalter: Meldung statt Download
- [x] Apply über der Obergrenze: CLI `--allow-large`, GUI Haken im Dialog
- [x] Phase 4: Regeln mit Platzhalter: kein EXIF-Lesen, kein Download

## 2. Block B: Reale Ordner, CLI (Plan-Integrität, Ordner-Duplikate)

Abschnitt 1a und 1b.

- [x] `plan dedupe-dirs` auf Ordner mit Kopie: Kopie wird entfernt, Original bleibt; Apply, `undo` stellt alles her
- [x] Heuristik auf Fehltreffer prüfen („Altbau“, „Backup-Strategie“), Muster notieren
- [x] Bericht „Ordner-Duplikate“ plausibel, Schwelle ändern
- [x] Plan in Notepad öffnen/speichern (auch CRLF): `apply` ohne Fehlalarm
- [x] Plan ändern: `apply` bricht ab, `$LASTEXITCODE` = 3, `runs` ohne neuen Lauf
- [x] `plan seal`: „N“ → Exit 1, „j“ → versiegelt, `apply` läuft
- [x] Alter Plan (Format 2): Hinweis, Lauf geht weiter
- [x] Laufzeit `plan dedupe-dirs` auf Index > 500.000 Dateien unter ca. 15 s

## 3. Block C: GUI-Kern

Abschnitt 4, 5, 1b (GUI) und 1a (GUI).

- [x] Karte „Doppelte Ordner“: Zahl, Review, Teilauswahl anwenden, zurückdrehen
- [x] Plan extern ändern, „Anwenden“: Dialog „Plan wurde verändert“, „Plan neu laden“
- [x] Review-Liste mit 500 echten Einträgen in unter 5 Minuten sichten
- [x] Purge ohne Eingriff (Hinweis: `0d` wird abgelehnt, Auffälligkeit 2; Frist ggf. 1d und einen Tag warten)
- [x] 100.000 Zeilen: Ruckeln beim Scrollbalken akzeptieren oder Kriterium umformulieren
- [x] Regel per Formular anlegen, Roh-TOML-Fehler, Config-Konflikt mit `.bak`
- [x] Profil/Zeitplan aus der GUI, Vorlage kopieren, Startmenü-Verknüpfung

## 4. Block D: Quarantäne-Ansicht (Abschnitt 1c)

Nach einem echten Aufräumlauf (Duplikate + Müll):

- [x] Liste stimmt mit „Verlauf“ überein (Name, Ort, Größe, Lauf, Ablauf, Status)
- [x] Suche, Wurzel-Auswahl, Sortierung
- [x] Zurückholen: normal, bei belegtem Ursprungspfad („(2)“), Ordner, veränderte Datei (Warnung)
- [x] Datei in Quarantäne im Explorer gelöscht: „fehlt“
- [x] Rechtsklick: „Im Explorer zeigen“ (Quarantäne-Ort), „Pfad kopieren“
- [x] „Inhalt ansehen“ im Verlauf, Filter-Etikett; Knopf deaktiviert bei leerem Lauf
- [x] Teilweise zurückgeholt, dann „Rückgängig“ des Laufs
- [x] Cloud-only in Quarantäne: „nur online“, kein Download
- [x] `F1`, `?`, Zurückholen während Scan/Apply
- [x] Viele Einträge: flüssig, Ladezeit notieren

## 5. Block E: GUI-Optik (alles in einem Durchgang)

Abschnitte 5a bis 5e, 4, 1c. Eine Runde mit Hell/Dunkel × DPI 100/150/200 %, dabei alle Seiten durchklicken.

- [x] Frischer Datenordner: „Noch nichts gescannt“, Kopfleiste, Dropdown, max. 8 zuletzt gewählte
- [x] Ordner umbenennen/Laufwerk trennen: Warnsymbol, Oberfläche hängt nicht
- [x] Kacheln nach Scan/Klassifizieren, Fehler-Kachel, abgebrochener Scan
- [x] Übersicht: Läufe, Quarantäne-Satz, Profile; Uhr +1 Tag: keine negativen Texte
- [x] Aufräumen: sechs Karten, Zahlen, Veraltet-Hinweis (> 7 Tage), Zahlfelder, Profil-Platzhalter, Regeln-Häkchen
- [x] Karten bei schmalem, niedrigem, halbem Fenster
- [x] Review: Kopfzeile, Hinweissatz je Plan-Art, Trennlinien, Tastatur (Pfeile, Leertaste, Strg+A/F/Enter)
- [x] Globale Kürzel Strg+1 bis Strg+6, in Textfeldern kein Kürzel
- [x] Einstellungen → Darstellung: Wie Windows/Hell/Dunkel, Umschalten zur Laufzeit
- [x] Hilfe (5e): Optik in Hell/Dunkel und DPI 100/150/200 %
- [ ] Seitenleisten-Symbole auf Windows 10 (falls vorhanden)
- [ ] VM und Remotedesktop: GUI startet (wgpu oder glow) oder MessageBox

## 6. Block F: Geplante Läufe (Abschnitt 2)

- [ ] `schedule add <profil> --daily 09:00`, `schedule list`, Aufgabenplanung zeigt sie
- [ ] `schtasks /run`: Lauf in `runs`, Bericht und Pläne unter `plans\<profil>`
- [ ] Toast bei mehr neuen Dateien „zum Prüfen“
- [ ] Parallel zu GUI-Scan: „Ein anderer Scan läuft“, Lauf als übersprungen
- [ ] `schedule remove`

## 7. Block G: Klassifikation und LLM (Abschnitt 3)

- [ ] `classify` auf Downloads/Scans: Kategorien plausibel, „Zum Prüfen“ nicht absurd groß
- [ ] OCR auf einem Scan, Abbruch sauber
- [ ] Lokales LLM aus/an: aus = keine Netzverbindung, an = nur lokales Ziel (`netstat`)
- [ ] `plan rules` nutzt Kategorien, unsichere als `skipped: low-confidence`

## 8. Block H: Nutzertest und Lasttest

- [ ] Person ohne CLI-Kenntnisse: Duplikate finden, prüfen, in Quarantäne, zurückholen; Stolpersteine notieren
- [ ] Lasttest ca. 1 Mio. Dateien (Index-Größe, GUI-Speicher, inkrementeller Scan)

## 9. Abschluss

- [ ] Fehler als Bug-Analyse unter `docs/bugs/` anlegen und per `/debug:…` beheben
- [ ] Pläne auf `status: implemented` (`gui.md` Spec und Plan), Roadmap-Status aktualisieren
- [ ] `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` grün
- [ ] PR `dev → main`, nach Merge Tag `v1.0.0`
