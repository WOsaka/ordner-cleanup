---
title: "Roadmap & Feature-Ideen"
created: 2026-10-02
updated: 2026-10-05
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
| Phase 6b: GUI (egui, eigene `ordner-cleanup-gui.exe`, Funktionsgleichheit mit der CLI) | [`features/gui.md`](features/gui.md), Plan: [`implementation-plans/gui.md`](implementation-plans/gui.md) | Spec freigegeben, Plan wartet auf Freigabe |
