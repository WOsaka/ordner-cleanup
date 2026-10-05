---
title: "Grafische Oberfläche (Phase 6b)"
status: approved          # draft | approved | implemented
created: 2026-10-05
updated: 2026-10-05
---

# Feature: Grafische Oberfläche (Phase 6b)

## Problem Statement
ordner-cleanup lässt sich heute nur über die CLI bedienen. Um einen Plan zu prüfen, muss man JSON- oder CSV-Dateien lesen und von Hand bearbeiten. Die Analyse gibt es nur als statischen HTML-Bericht. Regeln, Profile, Zeitpläne und Konfiguration stehen in TOML-Dateien, die man im Editor pflegt. Damit ist das Werkzeug Menschen ohne Konsolenerfahrung praktisch verschlossen, und auch für den Entwickler bleibt das Review großer Pläne mühsam.

Betroffen ist zunächst der Entwickler selbst, später Familie und Kollegen **ohne CLI-Kenntnisse**. Phase 6b gibt ordner-cleanup eine eigene Desktop-Oberfläche, über die **alles steuerbar ist, was die CLI kann**. Die Grundprinzipien bleiben dabei unverändert: Plan → Review → Apply, Quarantäne statt Löschen, geschützte Pfade, Cloud-only-Dateien nie lesen (siehe [`../roadmap.md`](../roadmap.md), Abschnitt 0 und 6).

## Goals
- [ ] Eigene `ordner-cleanup-gui.exe` (egui/eframe, ohne Konsolenfenster, portabel, ohne Admin-Rechte) neben der unveränderten CLI-exe
- [ ] **Funktionsgleichheit mit der CLI:** jeder Befehl (`scan`, `classify`, `report`, `history`, `run`, `schedule`, `profiles`, `index`, `plan *`, `apply`, `undo`, `runs`, `purge`) ist über die GUI erreichbar; ausgenommen ist nur der interne Hilfsprozess `ocr-worker`
- [ ] **Eine Logik, ein Format:** die GUI ruft dieselbe Bibliothekslogik (`app::*`, `change::*` …) auf wie die CLI und liest bzw. schreibt dieselben Dateien (Index, Plan, Undo-Journal, Lauf-Register, Config, Regel-TOML, Vorlagen). Ein in der GUI erzeugter Plan lässt sich per CLI anwenden und umgekehrt
- [ ] **Interaktive Analyse:** Größenbaum, Typen und Alter, Duplikate, Problemdateien, Strukturprobleme, Kategorien und „Zum Prüfen“ (6a), Health-Score und Verlauf; Export als HTML/JSON/CSV wie `report`
- [ ] **Plan-Review:** filtern, sortieren, suchen, Einträge einzeln oder gesammelt an- und abwählen, Details pro Eintrag (Quelle, Ziel, Grund, Größe, behaltene Kopie); übersprungene Einträge mit Grund sichtbar
- [ ] **Apply mit Bestätigungsdialog:** Anzahl, Größe, Aktionstypen, OneDrive-Warnungen und Obergrenze (`--allow-large` nur nach ausdrücklicher Bestätigung); danach eine Ergebnisansicht (ausgeführt / bereits erledigt / veraltet / übersprungen / fehlgeschlagen)
- [ ] **Verlauf:** Läufe mit Status und Quarantäne-Größe, „Rückgängig“ pro Lauf, Quarantäne leeren (purge)
- [ ] **Einstellungen:** Regeln (Formular-Editor, Trefferliste als Vorschau, Roh-TOML-Umschalter), Profile, geplante Läufe (Windows-Aufgabenplanung), Konfiguration (geschützte Pfade, Ausschlüsse, Quarantänefrist, Junk-Kategorien, Defaults der Planer …), Zielstruktur-Vorlagen, Klassifikation und lokales LLM
- [ ] **Nie einfrieren:** Scan, Classify, Plan, Apply, Undo und Purge laufen in Hintergrund-Threads, mit Fortschrittsanzeige und Abbrechen-Knopf
- [ ] **Große Datenmengen:** Listen mit 100.000+ Einträgen scrollen, filtern und sortieren ohne spürbares Ruckeln
- [ ] **Messbar:** Einen Plan mit 500 Einträgen durchsehen, korrigieren und anwenden in unter 5 Minuten, ohne eine Datei zu öffnen. Eine Person ohne CLI-Kenntnisse schafft Scan → Duplikate-Plan → Apply → Undo ohne Hilfe

## Lieferstufen
Die Spec beschreibt den vollen Umfang, umgesetzt wird in zwei Stufen:

| Stufe | Inhalt |
|---|---|
| **1** | Übersicht, Scan und Classify, Analyse, alle Plan-Arten mit Review, Apply, Verlauf (Undo, Purge), Index-Wurzeln verwalten, Bericht exportieren |
| **2** | Einstellungen: Regel-Editor, Profile, geplante Läufe, Konfiguration, Vorlagen, Klassifikation/LLM |

Nach Stufe 1 bleiben die Einstellungen weiter per Editor pflegbar. Die GUI bietet dann den Knopf „Datei im Editor öffnen“ und lädt sie nach dem Speichern neu.

## Out of Scope
- TUI
- Watch-Mode, Auto-Apply in jeder Form (geplante Läufe erzeugen weiter nur Pläne)
- macOS, Linux
- Mehrbenutzerbetrieb, Fernzugriff, lokale Web-GUI, jeder offene Port
- Ziele einzelner Plan-Einträge in der Review-Liste von Hand ändern (nur an- und abwählen; wer ein anderes Ziel will, ändert die Regel und plant neu)
- Mehrsprachige Oberfläche (nur Deutsch, wie die CLI)
- Installer (MSI/MSIX), automatische Updates
- Neue Analyse- oder Aufräumfunktionen, die die CLI nicht hat
- Dateivorschau mit Inhalt (Bilder, PDFs). Erlaubt sind „Im Explorer zeigen“ und „Mit Standardprogramm öffnen“ für lokal vorhandene Dateien

## User Journey

### Start und Übersicht
1. Der Nutzer startet `ordner-cleanup-gui.exe`, per Doppelklick oder über die Verknüpfung im Startmenü, die die GUI auf Wunsch selbst anlegt. Es öffnet sich ein Fenster ohne Konsole.
2. **Übersicht:** gescannte Wurzeln und Profile mit letztem Scan, Health-Score und Trend, letzte Läufe, Anzahl „Zum Prüfen“, ausstehende Quarantäne. Gibt es noch keinen Index, steht dort ein Hinweis „Ordner wählen und scannen“.
3. Die Navigation (Seitenleiste) bietet: Übersicht · Analyse · Aufräumen · Verlauf · Einstellungen.

### Scannen und Klassifizieren
4. Der Nutzer wählt einen Ordner (Windows-Ordnerdialog) oder ein Profil und klickt **Scannen**. Optionen wie in der CLI (zusätzliche Ausschlüsse, Default-Ausschlüsse aus, Netzlaufwerk trotzdem, Index neu aufbauen, Threads, Vorlage für Soll/Ist) stehen unter „Erweitert“.
5. Fortschrittsanzeige (Dateien, Größe, aktueller Pfad) mit **Abbrechen**. Die Oberfläche bleibt bedienbar, Analyse-Ansichten anderer Wurzeln bleiben nutzbar.
6. Optional **Klassifizieren** (6a) mit denselben Optionen wie `classify` (ohne LLM, alles neu, nur Endungen oder Glob, Text-Cache löschen); Fortschritt mit Quelle (Text/OCR/LLM).

### Analyse
7. Tabs für die gewählte Wurzel: **Größenbaum** (aufklappbar, sortiert nach Größe, Anteil als Balken), **Typen & Alter**, **Duplikate** (Gruppen mit Kopien), **Problemdateien & Struktur**, **Inhalte** (Kategorien, „Zum Prüfen“ mit Top-2-Kategorien), **Health-Score & Verlauf** (wie `history`, mit Diagramm), **Soll/Ist** (bei Vorlage).
8. Aus jeder Liste: „Im Explorer zeigen“, Pfad kopieren, und wo passend ein direkter Sprung „Plan dafür erzeugen“ (z. B. aus „Duplikate“ zu `plan dedupe`).
9. **Bericht exportieren:** Zielordner, Formate (HTML/JSON/CSV), Altersgrenze, Top-N; der HTML-Bericht lässt sich danach im Browser öffnen.

### Aufräumen: Plan erzeugen und prüfen
10. Der Nutzer wählt eine Plan-Art: Duplikate, Müll, leere Ordner, Archivieren, Versionen oder Regeln. Er stellt die Optionen der jeweiligen CLI-Variante ein (Strategie oldest/newest/path, Junk-Kategorien, Altersgrenzen, Regeldatei und Auswahl einzelner Regeln, ohne Nachklassifizieren). Die Defaults kommen aus Config bzw. Profil.
11. Der Plan wird im Hintergrund erzeugt und, wie in der CLI, als Plan-Datei gespeichert, standardmäßig im Plan-Ordner der App statt im aktuellen Ordner (siehe Technical Constraints). Danach öffnet sich die **Review-Liste**.
12. **Review-Liste:**
    - Spalten: Auswahl, Aktion, Quelle, Ziel, Größe, Grund bzw. Regel, Alter
    - Filter nach Aktion, Ordner, Regel, Größe und Text
    - Sortierung per Spaltenkopf
    - Zusammenfassung oben (ausgewählt: N Einträge, X GB)
    - Gesammelt an- und abwählen: alle, keine, aktuell gefilterte, ganzer Ordner
    - Detailbereich: zum Eintrag gehörende Infos, bei Duplikaten die behaltene Kopie, bei Regeln die Regel und die Platzhalterwerte
    - Übersprungene Einträge (z. B. `low-confidence`, `protected`) in einem eigenen Filter, nicht auswählbar
13. Bereits erzeugte Plan-Dateien, auch per CLI oder durch geplante Läufe erzeugte, lassen sich über **Plan öffnen** laden und genauso prüfen.

### Anwenden
14. **Anwenden** öffnet einen Bestätigungsdialog. Er zeigt Anzahl und Größe der gewählten Einträge nach Aktionstyp, das Ziel (Quarantäne bzw. Archivordner), Warnungen (OneDrive-Ordner, Obergrenze überschritten, Cloud-only-Dateien werden übersprungen) und, falls nötig, ein ausdrücklich anzuhakendes „Obergrenze für diesen Plan aufheben“.
15. Wurden Einträge abgewählt, schreibt die GUI vor dem Apply eine **neue Plan-Datei** mit nur den gewählten Einträgen (`<original>-auswahl-<Zeitstempel>.json`). Das Original bleibt unverändert. Angewendet wird die neue Datei, wie `apply` in der CLI.
16. Fortschritt mit **Abbrechen**. Ein Abbruch beendet die laufende Einzeloperation sauber, das Journal bleibt konsistent, und der Teil-Lauf lässt sich rückgängig machen.
17. **Ergebnisansicht:** ausgeführt / bereits erledigt / veraltet / übersprungen (mit Grund) / fehlgeschlagen (mit Fehlertext), Lauf-ID, Knopf „Rückgängig“.

### Verlauf
18. **Läufe:** Liste wie `runs` (Wurzel, Datum, Plan-Art, Status, Anzahl, Quarantäne-Größe), filterbar nach Wurzel; Details zeigen die Einträge des Journals.
19. **Rückgängig:** Bestätigungsdialog → Undo im Hintergrund → Ergebnis (zurückgedreht / Konflikte mit Grund).
20. **Quarantäne leeren:** Mindestalter (Default `quarantine_days`), Wurzel, Vorschau der betroffenen Läufe und Größe → Bestätigung → endgültiges Löschen.
21. **Index-Wurzeln verwalten:** Liste wie `index`, Wurzel aus dem Index entfernen (mit Bestätigung).

### Einstellungen (Stufe 2)
22. **Regeln:**
    - Liste der Regeln der gewählten Regeldatei
    - Jede Regel als Formular: Name, Match-Bedingungen (Pfad/Glob, Endung, Regex, Alter, Größe, EXIF, Inhaltsbedingungen aus 6a) und Aktion (move/rename/archive mit Ziel- bzw. Namensmuster und Platzhaltern), mit Platzhalter-Auswahl
    - Validierung beim Tippen mit derselben Prüfung wie die CLI
    - **Vorschau „Treffer“** auf einer gescannten Wurzel: welche Dateien zutreffen und wie das Ergebnis aussähe; verändert nichts und ist dasselbe wie ein `plan rules`, der nicht gespeichert wird
    - Reihenfolge per Ziehen oder per Auf/Ab-Knöpfe; neu, duplizieren, löschen
    - Umschalter **Roh-TOML** für Fortgeschrittene, mit derselben Validierung
    - Speichern schreibt die TOML-Datei, vorher wird eine Sicherung `rules.toml.bak` angelegt
23. **Profile:** anlegen, bearbeiten, löschen (Wurzel, Ausschlüsse, Plan-Defaults, `classify`, Benachrichtigung …).
24. **Geplante Läufe:** pro Profil täglich oder wöchentlich mit Uhrzeit, Benachrichtigung an/aus; Liste mit nächstem und letztem Lauf (wie `schedule add/list/remove`).
25. **Konfiguration:** geschützte Pfade, Default-Ausschlüsse, Quarantänefrist, Junk-Kategorien, Defaults für archive/versions, OneDrive-Obergrenzen, Speicherort der Pläne.
26. **Vorlagen:** eingebaute (PARA, Johnny.Decimal) ansehen, eigene Vorlagendateien anlegen und bearbeiten.
27. **Klassifikation & LLM:** eigene Kategorien, Mindest-Konfidenz, OCR-Sprachen, LLM an/aus, Modell und Endpunkt (nur Loopback), Knopf „Verbindung testen“.

## Acceptance Criteria
- [ ] Given ein frischer PC ohne Admin-Rechte, when der Nutzer `ordner-cleanup-gui.exe` startet, then öffnet sich das Fenster ohne Konsolenfenster und ohne Installation
- [ ] Given ein Ordner mit 200.000 Dateien, when der Nutzer scannt, then bleibt die Oberfläche bedienbar (Fenster verschieben, Tabs wechseln), der Fortschritt aktualisiert sich mindestens einmal pro Sekunde, und „Abbrechen“ beendet den Scan innerhalb weniger Sekunden
- [ ] Given ein Plan mit 100.000 Einträgen, when der Nutzer scrollt, filtert oder sortiert, then reagiert die Liste ohne spürbares Ruckeln (Ziel: Filter/Sortierung < 300 ms, Scrollen flüssig)
- [ ] Given ein Plan mit 500 Einträgen, when der Nutzer 20 davon abwählt und anwendet, then entsteht eine neue Plan-Datei mit 480 Einträgen, das Original bleibt bytegleich, und nur die 480 Einträge werden ausgeführt
- [ ] Given ein in der GUI erzeugter (oder gefilterter) Plan, when er per CLI `apply` angewendet wird, then verhält er sich identisch; Given ein per CLI erzeugter Plan, when er in der GUI geöffnet wird, then lässt er sich prüfen und anwenden
- [ ] Given ein in der GUI ausgeführter Lauf, when er per CLI `undo` zurückgedreht wird (und umgekehrt), then funktioniert das wie bei reinen CLI-Läufen
- [ ] Given ein Plan, der die OneDrive-Obergrenze überschreitet, when der Nutzer anwendet, then ist „Anwenden“ erst nach Anhaken von „Obergrenze für diesen Plan aufheben“ möglich
- [ ] Given Dateien wurden seit dem Plan geändert, when der Nutzer anwendet, then werden sie als „veraltet“ übersprungen und in der Ergebnisansicht so gezeigt
- [ ] Given ein laufender Scan (GUI oder geplanter Lauf), when ein zweiter Scan derselben Installation startet, then erhält der Nutzer die Meldung „Ein anderer Scan läuft“, und nichts geht kaputt
- [ ] Given ein laufendes Apply, when der Nutzer das Fenster schließt, then fragt die GUI nach; bei „Abbrechen und schließen“ endet die aktuelle Einzeloperation sauber, das Journal ist konsistent, und der Teil-Lauf lässt sich rückgängig machen
- [ ] Given eine ungültige Regel im Formular oder im Roh-TOML, when der Nutzer speichert, then wird nichts geschrieben, und die Fehlerstelle (Regel, Feld bzw. Zeile) ist markiert
- [ ] Given eine gespeicherte Regeländerung, when der Nutzer danach per CLI `plan rules` aufruft, then nutzt die CLI die geänderte Regeldatei
- [ ] Given ein Profil mit Zeitplan, das in der GUI angelegt wurde, when der Nutzer `schedule list` aufruft, then erscheint die Aufgabe dort genauso wie eine per CLI angelegte
- [ ] Given die GUI läuft, when man die offenen Netzwerkverbindungen prüft, then gibt es keinen lauschenden Port und keine Verbindung außer, falls eingeschaltet, zum LLM auf Loopback
- [ ] Given die CI, when `cargo tree` geprüft wird, then enthält die GUI-Abhängigkeit keinen gesperrten HTTP- oder TLS-Stack
- [ ] Given eine Person ohne CLI-Kenntnisse, when sie die Aufgabe „Duplikate in Ordner X finden, prüfen, in die Quarantäne verschieben und wieder zurückholen“ bekommt, then schafft sie das ohne Hilfe

## Edge Cases & Error States
| Szenario | Erwartetes Verhalten |
|----------|----------------------|
| Fenster schließen während Scan, Classify oder Plan | Rückfrage; bei Bestätigung sauberer Abbruch (bereits Gescanntes und Klassifiziertes bleibt, wie bei Ctrl+C in der CLI) |
| Fenster schließen während Apply, Undo oder Purge | Rückfrage mit Hinweis; bei Bestätigung endet die aktuelle Einzeloperation, das Journal ist konsistent, und der Lauf steht mit Status „abgebrochen“ im Verlauf |
| Geplanter CLI-Lauf und GUI greifen gleichzeitig zu | Bestehende Scan-Sperre respektieren; die GUI meldet „Ein anderer Scan läuft“ und bietet „Später erneut versuchen“. Lesende Ansichten bleiben nutzbar |
| Zwei GUI-Fenster gleichzeitig | Erlaubt; schreibende Aktionen sind über dieselben Sperren wie die CLI geschützt |
| Plan veraltet (Dateien seit Scan geändert, verschoben, gelöscht) | Erkennung pro Eintrag wie in der CLI (`Stale`, `KeepMissing`, `AlreadyDone`); die Ergebnisansicht zeigt die Gründe. Vor dem Apply ein Hinweis, wenn der Index älter als der Plan bzw. als X Tage ist |
| Abwahl bricht Abhängigkeiten (z. B. Unterordner von `empty-dirs` abgewählt, Elternordner nicht) | Apply überspringt den Elternordner, weil er nicht leer ist, und meldet das als „übersprungen“ mit Grund; die Review-Liste weist beim Abwählen auf betroffene abhängige Einträge hin |
| Cloud-only-Platzhalter im Plan | Werden wie in der CLI nie gelesen oder gehasht und als „Cloud-only“ übersprungen; „Mit Standardprogramm öffnen“ ist für sie deaktiviert (würde einen Download auslösen) |
| OneDrive-Obergrenze überschritten | Warnung im Bestätigungsdialog, Anwenden nur mit ausdrücklichem Haken |
| Geschützter Pfad gewählt (z. B. `C:\Windows`) | Scannen erlaubt, sofern die CLI es erlaubt; Pläne enthalten dort keine Aktionen, die Review-Liste zeigt sie als „geschützt“ übersprungen |
| Netzlaufwerk/UNC gewählt | Gleiche Warnung wie die CLI, Scan erst nach Bestätigung |
| Lange Pfade (> 260 Zeichen), Umlaute, ungültige Zeichen | Werden korrekt angezeigt, kopiert und an den Explorer übergeben |
| LLM eingeschaltet, aber nicht erreichbar | Hinweis in Classify-Ergebnis und Einstellungen; Classify läuft regelbasiert weiter |
| OCR nicht verfügbar (Sprachpaket fehlt) | Hinweis mit Anleitung, welches Windows-Sprachpaket fehlt; Classify läuft ohne OCR weiter |
| Ungültige Regel-, Config- oder Profildatei beim Start (von Hand kaputt editiert) | GUI startet trotzdem; betroffene Bereiche zeigen den Fehler mit Datei und Zeile sowie „Im Editor öffnen“, statt abzustürzen |
| Regeldatei wurde extern geändert, während der Editor offen ist | Beim Speichern Konflikthinweis: „Datei wurde extern geändert: neu laden / überschreiben / abbrechen“ |
| Kein Schreibrecht im Plan-Ordner oder Zielordner | Verständliche Fehlermeldung mit Pfad, kein Absturz |
| Plan-Datei aus einer anderen Version (altes oder unbekanntes Format) | Lesbares Format wird geladen; unbekanntes Format → Meldung mit Versionsnummer, nichts wird ausgeführt |
| Aufgabenplanung nicht erreichbar (Richtlinie, Dienst aus) | Fehlermeldung beim Anlegen; Profil wird trotzdem gespeichert |
| Sehr hohe DPI / Skalierung 150–200 % | Oberfläche skaliert sauber, nichts ist abgeschnitten |
| Absturz in einem Hintergrund-Thread (Panic) | Fehlerdialog mit Meldung; die GUI bleibt offen, eine laufende Aktion gilt als abgebrochen, das Journal ist konsistent |

## Technical Constraints
- **Framework:** egui/eframe (Backend `wgpu` oder `glow`, beim Plan zu entscheiden). Tabellen über `egui_extras::TableBuilder` oder ein vergleichbares virtualisiertes Widget; Diagramme (Verlauf, Größenanteile) mit `egui_plot` oder selbst gezeichnet.
- **Binary:** neues `src/bin/ordner-cleanup-gui.rs` mit `#![windows_subsystem = "windows"]`, analog zu `ordner-cleanup-bg.rs`. Die CLI-exe und die bg-exe bleiben unverändert. Die CI baut und lädt alle drei Binaries hoch.
- **Architektur:**
  - Die GUI ist eine dünne Schicht über der Bibliothek. Fachlogik, die heute in CLI-Ausgabefunktionen steckt, wird bei Bedarf in die Bibliothek gezogen und über strukturierte Rückgabewerte angeboten (Fortschritt, Ergebnisse, Fehler), nicht über das Parsen von Konsolentext.
  - Der Abbruch läuft über ein gemeinsames Abbruch-Token, das die CLI an Ctrl+C und die GUI an den Abbrechen-Knopf bindet.
  - Bestätigungen (`--yes`) laufen über eine Schnittstelle, die die CLI auf der Konsole und die GUI per Dialog beantwortet.
- **Threads:** Lange Aufgaben laufen in Worker-Threads. Kommunikation über Kanäle (z. B. `crossbeam-channel`, schon vorhanden); der UI-Thread blockiert nie auf I/O.
- **Plan-Speicherort:** Die GUI legt Pläne standardmäßig in einem festen App-Ordner ab, z. B. `%LOCALAPPDATA%\ordner-cleanup\plans\`; konfigurierbar. Die CLI bleibt beim bisherigen Default (aktueller Ordner).
- **Gefilterter Plan:** Ein abgewählter Teil wird als neue Plan-Datei im selben Format (`PLAN_VERSION`) geschrieben. Es gibt keinen Formatwechsel und kein `skip`-Feld im Plan.
- **Performance:**
  - virtuelle Listen (nur sichtbare Zeilen zeichnen)
  - Filter und Sortierung auf vorbereiteten Indizes statt auf Strings pro Frame
  - Größenbaum lazy pro aufgeklappter Ebene aus dem Index laden
- **Security:**
  - kein lauschender Port, kein HTTP-Server, keine Telemetrie
  - Netzwerkzugriff nur für das lokale LLM über Loopback (bestehende Prüfung in `content/llm.rs`)
  - die CI-Sperre für `reqwest|hyper|curl|isahc|attohttpc|rustls|native-tls|openssl` gilt weiter und muss mit egui/eframe und allen neuen Abhängigkeiten bestehen
- **Rechte:** läuft als normaler Benutzer; keine Registry-Schreibzugriffe außer denen, die die CLI schon macht (Aufgabenplanung, Benachrichtigungen); portable exe, Startmenü-Verknüpfung nur auf Wunsch und nur im Benutzerprofil.
- **Sprache:** Oberfläche nur Deutsch; Texte zentral gesammelt, damit eine spätere Übersetzung möglich bleibt (ohne i18n-Framework).
- **Tests:**
  - Logik der GUI-Zustände (Filter, Auswahl, Plan-Teilmenge, Dialog-Abläufe) als Unit-Tests ohne Fenster
  - Rundreise-Tests „GUI-Teilplan → CLI-apply → CLI-undo“
  - Darstellung wird manuell geprüft
- **Compliance:** Keine Daten verlassen den PC. Es gelten dieselben Datenschutzregeln wie in 6a (OCR-Text nur verschlüsselt im Cache).

## Dependencies
- **Bibliothek:** `app::*`, `change::*` (Plan, Apply, Undo, Quarantäne, Journal, Register, Limits), `scan::*` (inkl. `ScanLock`), `report`, `history`, `health`, `rules`, `template`, `content` (6a), `schedule`, `config`, `paths`
- **Phasen 1–6a** müssen umgesetzt sein (sind es); die GUI setzt auf deren Formate und Verhalten auf
- **Neue Crates:** `eframe`/`egui`, `egui_extras`, eventuell `egui_plot`; nativer Ordner- und Dateidialog (z. B. `rfd` mit Windows-Backend, sonst `IFileDialog` direkt über das vorhandene `windows`-Crate). Alle müssen die CI-Sperre bestehen
- **Windows:** Aufgabenplanung (wie Phase 5), Explorer-Integration („Im Explorer zeigen“ über `SHOpenFolderAndSelectItems`), `ShellExecute` für „Mit Standardprogramm öffnen“
- **Optional:** lokales LLM (Ollama) und Windows-OCR wie in 6a

## Open Questions
- [ ] Welche Teile der heutigen CLI-Ausgabe (`app::*`) geben Ergebnisse nur als Konsolentext aus und müssen für die GUI in strukturierte Rückgabewerte umgebaut werden? (Bestandsaufnahme im Implementierungsplan)
- [ ] egui-Backend `wgpu` oder `glow`: Binary-Größe, Start auf PCs ohne aktuelle Grafiktreiber bzw. in VMs/RDP
- [ ] Ordnerdialog über `rfd` oder direkt über `IFileDialog` (eine Abhängigkeit weniger)?
- [ ] Soll die GUI eine Startmenü-Verknüpfung selbst anlegen, oder reicht ein Hinweis?
- [ ] Wie werden Abhängigkeiten zwischen Plan-Einträgen (empty-dirs, archive) in der Review-Liste am klarsten dargestellt: Baumansicht statt flacher Liste?
