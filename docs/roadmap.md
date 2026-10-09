---
title: "Roadmap & Feature-Ideen"
created: 2026-10-02
updated: 2026-10-09
---

# Roadmap & Feature-Ideen

Ideensammlung und grobe Priorisierung für ordner-cleanup. Einzelne Features werden per `/feature:spec` in `docs/features/` spezifiziert; hier steht nur der Überblick.

## 0. Grundprinzipien (gelten für alle Features)

- **Plan → Review → Apply:** Jede schreibende Aktion erzeugt zuerst einen Änderungsplan (JSON/CSV/HTML). Ausführung erst nach Bestätigung. Dry-Run ist der Standard.
- **Undo-Journal:** Jede Operation wird protokolliert (Quelle, Ziel, Hash, Zeitstempel). `undo <run-id>` dreht einen kompletten Lauf zurück.
- **Nie hart löschen:** Windows-Papierkorb oder Quarantäne-Ordner mit Aufbewahrungsfrist.
- **Geschützte Pfade:** `C:\Windows`, `Program Files`, `AppData`, `.git`, `node_modules`, Projektordner mit Marker-Dateien usw. sind per Default tabu und konfigurierbar.
- **Idempotenz:** Ein zweiter Lauf mit denselben Regeln ändert nichts.

## 1. Analyse (nur lesend)

| Feature | Nutzen |
|---|---|
| Größenbaum / Speicherfresser | Zeigt, wo der Platz hingeht |
| Statistik nach Dateityp, Alter, Größe | z. B. „40 % Videos, 60 % seit 3 Jahren nicht angefasst“ |
| Exakte Duplikate (Größe → Teil-Hash → Voll-Hash) | Schnelle, sichere Platzersparnis |
| Duplikate ganzer Ordner | „Backup_alt“, „Kopie von Projekt“ |
| Ähnliche Dateien / Versionswildwuchs (`_final_v2`, `- Kopie (3)`) | Versionschaos sichtbar machen |
| Strukturprobleme: leere Ordner, tiefe Verschachtelung, Ein-Datei-Ordner, Riesenordner | Hinweise auf schlechte Struktur |
| Problemdateien: Pfade > 260 Zeichen, ungültige Zeichen, kaputte `.lnk`, 0-Byte, `~$`-Lockdateien, `.tmp` | Verursachen Sync- und Backup-Fehler |
| Namenskonventionen prüfen | Uneinheitliche Datumsformate, Umlaute, Leerzeichen |
| Health-Score pro Ordner | Eine Kennzahl, die man über Zeit verfolgen kann |
| Export: HTML-Bericht + CSV/JSON | Lesbar und weiterverarbeitbar |

## 2. Aufräumen

- Duplikate auflösen mit Strategie (älteste/neueste/kanonischer Pfad/interaktiv), optional Ersatz durch Hardlinks
- Leere Ordner rekursiv entfernen
- Müll erkennen: Temp-Dateien, `Thumbs.db`, alte Installer in Downloads, abgebrochene Downloads (`.crdownload`, `.part`)
- Archivieren statt löschen: lange unberührte Ordner nach ZIP bzw. `_Archiv/<Jahr>/…`
- Versions-Cleanup: nur die neueste Version behalten, den Rest archivieren

## 3. Neuorganisation

- **Regel-Engine (TOML):** Match auf Pfad/Endung/Regex/Alter, Aktionen move/rename/archive, Platzhalter `{year}`, `{month}`, `{ext}`, `{exif.date}`, `{parent}`
- **Umbenennen:** ISO-Datum voranstellen, Normalisierung (Leerzeichen, Umlaute, „Kopie“-Suffixe), Kollisionen durchnummerieren
- **Zielstruktur-Vorlagen:** PARA, Johnny.Decimal, eigene Firmenvorlage; Soll/Ist-Abgleich
- **Inhaltsbasierte Klassifikation:** EXIF, PDF-/Office-Metadaten, Textextraktion; optional LLM-Vorschläge (nur als Plan, Datenschutz beachten)

## 4. Laufender Betrieb

- Watch-Mode für Downloads/Desktop (nicht in Phase 5, später)
- Geplante Läufe (Windows-Aufgabenplanung), wöchentlicher Bericht
- Verlauf von Speicherverbrauch und Health-Score
- Profile (Privat / Firma / Downloads)

## 5. Windows- & OneDrive-Besonderheiten

- **Files On-Demand:** Cloud-only-Platzhalter (`FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` / `OFFLINE`) nie lesen oder hashen, damit kein Download ausgelöst wird
- Erkennung von Sync-Konfliktdateien (`Datei-PCNAME.docx`)
- Massenverschiebungen in OneDrive/SharePoint sind teuer (Sync-Traffic, Konflikte)
- Lange Pfade (`\\?\`), gesperrte Dateien, Junctions/Symlinks (Zyklen), versteckte/System-Dateien, Case-Insensitivity
- Berechtigungen vor schreibenden Läufen prüfen

## 6. Bedienung & Technik

- CLI zuerst (`scan`, `report`, später `plan`, `apply`, `undo`)
- Interaktives Review im Terminal (TUI)
- Optional später eine lokale Web-GUI
- Index-Cache (SQLite) für inkrementelle Re-Scans

## 7. Ausbau nach Phase 6 (Ideen, Stand 2026-10-09)

Alle Phasen 1 bis 6d sind im Code umgesetzt. Die folgenden Ideen stammen aus den „Out of Scope“-Listen der Specs, aus Lücken im Code und aus dem Ziel, das Tool an Familie und Kollegen ohne CLI-Kenntnisse weiterzugeben.

### 7.0 Konsolidieren vor neuen Features
- Offene Handtests abarbeiten (GUI 5a bis 5e, geplante Läufe mit Toast, LLM-Spikes, VM/RDP), vor allem den **echten Cloud-only-Platzhalter**
- ~~Rundreise-Property-Tests: zufälliger Baum → Plan → Apply → Undo ergibt den Ausgangsbaum bitgleich~~ erledigt: `tests/roundtrip_random.rs` (alle sechs Planer in zufälliger Reihenfolge, Idempotenz je Planer, Undo rückwärts; 8 Seeds in `cargo test`, weitere per `--ignored`)
- Lasttest mit ca. 1 Mio. Dateien (Index-Größe, Speicher der GUI-Tabellen, inkrementeller Scan)
- ~~Plan-Integrität: Hash der Plan-Datei, vor dem Apply geprüft~~ umgesetzt (siehe unten)
- Release 1.0 taggen

### 7.1 Lücken schließen
| Idee | Nutzen | Aufwand |
|---|---|---|
| ~~Duplikate ganzer Ordner (`plan dedupe-dirs`, Merkle-Hash aus Datei-Hashes)~~ (umgesetzt, siehe unten) | Größter Platzgewinn („Backup_alt“) | M |
| Kombinierter Aufräumplan (`plan cleanup`) mit Konfliktauflösung zwischen Aktionen | Ein Klick statt fünf Karten | M |
| Archivieren als ZIP (Hash-Prüfung im ZIP, Undo = Entpacken) | Echte Platzersparnis | M |
| Hardlinks statt Quarantäne für Duplikate (nur außerhalb OneDrive, opt-in) | Platz sparen, Datei bleibt erreichbar | S–M |
| Inhalts-Hash als zweiter Cache-Schlüssel für Klassifikation/OCR | Verschobene Dateien nicht neu analysieren | S |
| „Regel aus Auswahl erzeugen“ in der Review-Liste | Weniger Hin und Her zwischen Review und Regeln | M |

### 7.2 Neue Analysen
- Entwickler-/Projekt-Caches (`target/`, `node_modules`, `.venv`, `bin/obj`) in lange unberührten Projekten; braucht eine eng begrenzte Ausnahme von den geschützten Pfaden (nur bei Projekt-Markern)
- Ähnliche Fotos (pHash/dHash), Serienbilder, unscharfe Aufnahmen; Auswahl im Review
- Verwaiste Installer in Downloads (Abgleich mit installierten Programmen, nur lesend)
- Datenschutz-Check: sensible Dokumente (Ausweis, Kontoauszug, Vertrag) offen auf Desktop/Downloads oder in geteilten OneDrive-Ordnern
- Audio-Metadaten (ID3/FLAC) für Musik-Regeln
- Speicherfresser außerhalb der Nutzerordner nur als Hinweis (Hibernation, Update-Cache, Wiederherstellungspunkte)

### 7.3 Neuorganisation
- Regeln aus Beispielen lernen: manuelle Verschiebungen zwischen zwei Scans (per Hash erkannt) als Regel vorschlagen
- Regel-Testbank in der GUI: Live-Vorschau der Treffer und Ziele
- Vorlage anwenden: fehlende Ordner anlegen + Migrationsplan
- Moves über Laufwerke (Kopieren → Hash prüfen → Quarantäne), Laufwerke über Volume-Seriennummer wiedererkennen
- Regeln auf Ordnern

### 7.4 Laufender Betrieb
- Eingangsordner-Modus: Hintergrundprozess beobachtet nur Downloads/Desktop, erzeugt Pläne, Toast mit „Prüfen“
- Explorer-Kontextmenü (HKCU, ohne Admin) und Tray-Icon mit Score
- Quarantäne-Ansicht: Inhalt, Größe, Ablauf, einzelne Dateien zurückholen
- Monatsbilanz („12,4 GB frei geräumt, Score 61 → 78“)
- Vorher/Nachher-Bilanz pro Lauf: direkt nach `apply` zeigen, wie viel Platz frei wurde und wie sich der Health-Score verändert hat (kleinere Vorstufe der Monatsbilanz)
- Plan-Vorschau als Baumvergleich: im Review die Ordnerstruktur vor und nach dem Plan nebeneinander, vor allem für Regel-Engine und Neuorganisation

### 7.5 Weitergabe
- Erststart-Assistent (Ordner → Profil → Scan → Ergebnis)
- Code-Signierung bzw. Installer (SmartScreen)
- Firmen-Vorgaben: zentrale Regeln/Vorlagen/Schutzpfade von einer Freigabe, schreibgeschützt über der Nutzer-Config
- Diagnosepaket (Logs, Config mit geschwärzten Pfaden)
- Englische Oberfläche (Texte liegen zentral in `texts.rs`)

### 7.6 Technik
- Schneller Scan über das USN-Journal (meist Admin nötig, Walker als Rückfall)
- Netzlaufwerke/SMB: Bandbreitenbremse, Modus „nur Metadaten“

### 7.7 Offene Grundsatzentscheidungen
- **Auto-Apply für einzelne, freigegebene Regeln?** Würde Grundprinzip „Plan → Review → Apply“ aufweichen
- **Netzwerkzugriff für Update-Prüfung (opt-in, GitHub Releases)?** Ausnahme von „keine Daten verlassen den PC“

### 7.8 Vorschlag Reihenfolge
1. Konsolidieren und 1.0 (7.0)
2. Duplikate ganzer Ordner
3. Entwickler-Caches
4. Kombinierter Aufräumplan + Erststart-Assistent
5. Regeln aus Beispielen lernen

Kleine Sofortgewinne zwischendurch: Explorer-Kontextmenü, Inhalts-Hash als Cache-Schlüssel, Quarantäne-Ansicht.

## Priorisierung

| Phase | Inhalt | Begründung |
|---|---|---|
| **1 – MVP** | Scan + Index, Analyse-Bericht (Größe, Typen, Alter, leere Ordner, Problemdateien), exakte Duplikate | Nur lesend, sofort nützlich, kein Risiko |
| **2** | Änderungsplan-Format, Apply, Undo-Journal, Papierkorb/Quarantäne, geschützte Pfade | Sicherheitsfundament |
| **3** | Aufräumaktionen (Duplikate, leere Ordner, Müll, Archivieren) | Erste schreibende Features |
| **4** | Regel-Engine (TOML) + Umbenennen | Eigentliche Neuorganisation |
| **5** | Watch-Mode, geplante Läufe, Verlauf, Vorlagen | Ordnung dauerhaft halten |
| **6** | Inhalts-/KI-Klassifikation, GUI | Ausbaustufe |

## Feature-Status

| Feature | Spec | Status |
|---|---|---|
| Phase 1: Scan & Analyse-Bericht | [`features/scan-analyse-bericht.md`](features/scan-analyse-bericht.md), Plan: [`implementation-plans/scan-analyse-bericht.md`](implementation-plans/scan-analyse-bericht.md) | Umgesetzt (Schritte 0 bis 12), manuell auf echtem OneDrive-Ordner geprüft |
| Phase 2: Änderungsplan, Apply & Undo | [`features/aenderungsplan-apply-undo.md`](features/aenderungsplan-apply-undo.md) | Umgesetzt (`plan dedupe`, `apply`, `undo`, `runs`, `purge`), manuell auf OneDrive-Testordner geprüft; offen: Test mit echtem Cloud-only-Platzhalter und echtem Terminal |
| Phase 3: Aufräumaktionen (junk, empty-dirs, archive, versions) | [`features/aufraeumaktionen.md`](features/aufraeumaktionen.md), Plan: [`implementation-plans/aufraeumaktionen.md`](implementation-plans/aufraeumaktionen.md) | Umgesetzt (`plan junk`, `plan empty-dirs`, `plan archive`, `plan versions`, Plan-Format v2, OneDrive-Obergrenze `--allow-large`, Undo-Aufräumen), manuell im OneDrive-Testordner und auf einem Alltagsordner geprüft; offen: Test mit echtem Cloud-only-Platzhalter |
| Phase 4: Regel-Engine & Umbenennen | [`features/regel-engine-umbenennen.md`](features/regel-engine-umbenennen.md), Plan: [`implementation-plans/regel-engine-umbenennen.md`](implementation-plans/regel-engine-umbenennen.md) | Umgesetzt (manueller Test offen); Regelformat TOML statt YAML (Begründung in der Spec) |
| Phase 5: Laufender Betrieb (Verlauf, Health-Score, geplante Läufe, Profile, Vorlagen) | [`features/laufender-betrieb.md`](features/laufender-betrieb.md), Plan: [`implementation-plans/laufender-betrieb.md`](implementation-plans/laufender-betrieb.md) | Umgesetzt (manueller Test mit echten geplanten Läufen und Toast offen); Watch-Mode und Auto-Apply ausgeklammert |
| Phase 6a: Inhaltsbasierte Klassifikation (PDF/Office/EXIF/Video/OCR, Kategorien, optional lokales LLM) | [`features/inhalts-klassifikation.md`](features/inhalts-klassifikation.md), Plan: [`implementation-plans/inhalts-klassifikation.md`](implementation-plans/inhalts-klassifikation.md) | Umgesetzt (Schritte 1 bis 13; manueller Test auf echtem Downloads-/Scans-Ordner und Spikes mit echten PDFs und lokalem LLM offen); GUI (Phase 6b) bekommt eine eigene Spec |
| Phase 6b: GUI (egui, eigene `ordner-cleanup-gui.exe`, Funktionsgleichheit mit der CLI) | [`features/gui.md`](features/gui.md), Plan: [`implementation-plans/gui.md`](implementation-plans/gui.md) | Stufe 1 und 2 im Code umgesetzt (ops-Schicht, GUI, Einstellungen); manueller Test (Teil A und B), Nutzertest und Messung in VM/RDP offen |
| Phase 6c: GUI-Bedienung (Kennzahl-Kacheln, Aufräumen als Karten, Theme hell/dunkel, Tastaturkürzel, Skala) | [`features/gui-bedienung.md`](features/gui-bedienung.md), Plan: [`implementation-plans/gui-bedienung.md`](implementation-plans/gui-bedienung.md) | Umgesetzt (Stufe 1 bis 4, Schritte 1 bis 20); Handtests in `manual-tests.md` 5a bis 5d (Optik, DPI, Windows-Modus) offen |
| Phase 6d: GUI-Hilfe (Kurztexte zu Seiten, Karten und Review, F1, Link zur Doku) | [`features/gui-hilfe.md`](features/gui-hilfe.md), Plan: [`implementation-plans/gui-hilfe.md`](implementation-plans/gui-hilfe.md) | Im Code umgesetzt (Schritte 1 bis 9); Handtests in `manual-tests.md` 5e offen |
| 7.1: Duplikate ganzer Ordner (`plan dedupe-dirs`) | [`features/ordner-duplikate.md`](features/ordner-duplikate.md), Plan: [`implementation-plans/ordner-duplikate.md`](implementation-plans/ordner-duplikate.md) | Umgesetzt (`plan dedupe-dirs`, Bericht, GUI-Karte, Rundreise-Test); offen: Handtest auf echten Ordnern (`manual-tests.md` 1a) |
| 7.0: Plan-Integrität (Prüfsumme der Plan-Datei, `plan seal`, Exit-Code 3) | [`features/plan-integritaet.md`](features/plan-integritaet.md), Plan: [`implementation-plans/plan-integritaet.md`](implementation-plans/plan-integritaet.md) | Umgesetzt (Plan-Format v3, Prüfung in CLI und GUI, `plan seal`); offen: Handtest mit Notepad und GUI (`manual-tests.md` 1b) |
| 7.4: Quarantäne-Ansicht (Inhalt, Größe, Ablauf, einzelne Einträge zurückholen; nur GUI) | [`features/quarantaene-ansicht.md`](features/quarantaene-ansicht.md) | Spec freigegeben (approved) |
