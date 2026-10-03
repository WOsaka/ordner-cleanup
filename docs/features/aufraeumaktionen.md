---
title: "Aufräumaktionen (Phase 3)"
status: draft          # draft | approved | implemented
created: 2026-10-03
updated: 2026-10-03
---

# Feature: Aufräumaktionen (Phase 3)

## Problem Statement
Phase 2 hat das Sicherheitsfundament geschaffen (Plan → Apply → Undo, Quarantäne, geschützte Pfade, Write-ahead-Journal), kennt aber nur eine Aktion: exakte Duplikate in die Quarantäne verschieben. Die typischen Alltagsprobleme in gewachsenen Ordnern bleiben ungelöst: Müll (Thumbs.db, Temp- und Lockdateien, abgebrochene Downloads, alte Installer), leere Ordner, seit Jahren unberührte Ordner, die aktive Bereiche verstopfen, und Versionswildwuchs (`_final_v2`, `- Kopie (3)`).

Betroffen ist zunächst der Entwickler selbst, später Kollegen. Phase 3 liefert die ersten echten Aufräumaktionen auf Basis des Phase-2-Fundaments (siehe [`../roadmap.md`](../roadmap.md)).

## Goals
- [ ] `plan junk` erkennt Müll nach Kategorien und plant ihn in die Quarantäne
- [ ] `plan empty-dirs` entfernt rekursiv leere Ordner (neue Aktion `remove-dir`, per `undo` wiederherstellbar)
- [ ] `plan archive` verschiebt lange unberührte Ordner nach `_Archiv\<Jahr>\…` (neue Aktion `move`)
- [ ] `plan versions` behält pro Versionsgruppe die neueste Datei und verschiebt ältere Versionen nach `_Archiv\Versionen\…`
- [ ] Alle Aktionen laufen über `apply`/`undo`/`runs` aus Phase 2 und erfüllen dieselben Garantien (Dry-Run, Stale-Prüfung, Schutzregeln, Cloud-Platzhalter, Idempotenz, Write-ahead)
- [ ] Große Verschiebungen unter OneDrive verlangen eine ausdrückliche Freigabe
- [ ] `undo` räumt die danach leeren Ordner unter `quarantine\<run-id>` auf (offener Punkt aus Phase 2)

## Out of Scope
- ZIP-Archive (Archivieren ist nur Verschieben)
- Ersetzen von Duplikaten durch Hardlinks
- Interaktives Review (TUI) oder GUI; Review erfolgt weiterhin über die Plan-Datei
- Kombinierter Plan (`plan cleanup` o. Ä.); jede Aktion erzeugt ihren eigenen Plan
- Regel-Engine (YAML), Umbenennen (Phase 4)
- Inhaltsbasierte Versionserkennung (nur Namensheuristik aus Phase 1)
- Strengere Behandlung der AppData-Wurzel
- Windows-Papierkorb als Ziel, automatisches Leeren der Quarantäne
- Netzlaufwerke (SMB), SharePoint-spezifische Behandlung, Linux/macOS

## User Journey
1. Der Nutzer hat einen Ordner mit `scan` indiziert.
2. Er wählt eine Aufräumaktion und erzeugt einen Plan (Dry-Run, nichts wird verändert):
   - `ordner-cleanup plan junk <pfad> [--category system,temp,downloads,installer] [--out <plan.json>]`
   - `ordner-cleanup plan empty-dirs <pfad> [--out <plan.json>]`
   - `ordner-cleanup plan archive <pfad> [--older-than 2y] [--out <plan.json>]`
   - `ordner-cleanup plan versions <pfad> [--min-age 30d] [--out <plan.json>]`
3. Das System gibt eine Zusammenfassung aus (Anzahl Aktionen, betroffene Bytes bzw. Ordner, übersprungene Einträge mit Gründen) und nennt den Pfad der Plan-Datei. Unter einer OneDrive-Wurzel erscheint die Warnung aus Phase 2.
4. Der Nutzer prüft die Plan-Datei.
5. Er startet `ordner-cleanup apply <plan.json> [--yes] [--allow-large]`. Überschreitet ein Plan unter einer OneDrive-Wurzel die konfigurierte Obergrenze (Anzahl Dateien oder Bytes, die verschoben werden), verweigert `apply` ohne `--allow-large` mit klarer Meldung.
6. Das System führt die Aktionen wie in Phase 2 aus: Stale-Prüfung, Write-ahead-Journal, Schutzregeln erneut prüfen, dann Aktion.
7. Am Ende erscheinen Zusammenfassung und `run-id`. `undo <run-id>` dreht den Lauf zurück, `runs` listet ihn.

Empfohlene Reihenfolge für einen Komplettputz: `junk` → `apply` → `scan` → `empty-dirs` → `apply`. Ordner, in denen nur noch Müll liegt, gelten für `empty-dirs` nicht als leer.

### Aktion `junk`
- Eingebaute Kategorien (per `--category` wählbar, Default: alle):
  - `system`: `Thumbs.db`, `ehthumbs.db`, `.DS_Store`
  - `temp`: `*.tmp`, `~$*` (Office-Lockdateien)
  - `downloads`: `*.crdownload`, `*.part`, `*.partial`
  - `installer`: `*.exe`, `*.msi` direkt im Downloads-Ordner des Nutzers, älter als N Tage (Default 90, Config). Der Downloads-Ordner wird über die Known Folder API (`SHGetKnownFolderPath(FOLDERID_Downloads)`) bestimmt, damit auch umgeleitete Ordner erkannt werden; die Config kann ihn überschreiben und weitere Ordner ergänzen.
- Die Config kann eigene Kategorien bzw. Muster ergänzen (Glob auf den Dateinamen, optional Mindestalter, optional auf Ordner beschränkt).
- Aktionstyp `quarantine` mit `reason: junk:<kategorie>`.

### Aktion `empty-dirs`
- Ein Ordner ist leer, wenn er weder Dateien (auch keine Cloud-Platzhalter, versteckten oder System-Dateien) noch nicht-leere Unterordner enthält. Rekursiv: Ordner, die nur leere Ordner enthalten, werden mit entfernt. Die Reihenfolge im Plan ist von unten nach oben.
- Die Scan-Wurzel selbst wird nie entfernt.
- Neuer Aktionstyp `remove-dir`: Beim Apply wird der Ordner nur entfernt, wenn er in diesem Moment wirklich leer ist. Das Journal speichert Attribute und Zeitstempel. `undo` legt Ordner von oben nach unten wieder an und setzt Attribute und Zeitstempel zurück.

### Aktion `archive`
- Ein Ordner gilt als alt, wenn die jüngste mtime aller Dateien darin (rekursiv) älter als `--older-than` ist (Default 2 Jahre, Config). Es zählt nur die mtime; die Zugriffszeit (atime) wird nicht berücksichtigt, weil sie unter Windows oft deaktiviert ist oder von Virenscanner und Indexer verfälscht wird.
- Nur der oberste passende Ordner wird verschoben, nie zusätzlich seine Unterordner. Einzeldateien werden nicht archiviert.
- Ziel: `<wurzel>\_Archiv\<Jahr der jüngsten mtime>\<relativer Pfad>`.
- Neuer Aktionstyp `move` (Quelle, Ziel, für Ordner: Anzahl Dateien und Summe der Bytes als Stale-Merkmal). Moves bleiben auf demselben Volume.
- Ordner unterhalb von `_Archiv` sind keine Kandidaten (Idempotenz).

### Aktion `versions`
- Gruppenbildung über die Namensnormalisierung aus Phase 1 (`analysis/similar.rs`), zusätzlich eingeschränkt auf denselben Ordner und dieselbe Endung. Eine Gruppe braucht mindestens zwei Dateien.
- Die Datei mit der jüngsten mtime bleibt (`keep`), alle anderen bekommen eine `move`-Aktion nach `<wurzel>\_Archiv\Versionen\<relativer Pfad>`, sofern ihre mtime mindestens `--min-age` alt ist (Default 30 Tage, Config). Jüngere ältere Versionen bleiben am Ort und erscheinen als übersprungen (`reason: too-recent`), damit laufende Arbeit nicht gestört wird.
- Exakte Duplikate innerhalb einer Gruppe bleiben trotzdem Versionen. Für Duplikate ist `plan dedupe` zuständig.
- Dateien unterhalb von `_Archiv` sind keine Kandidaten.

### Plan-Format (Erweiterung)
Neue Pläne tragen `version: 2`. `apply` und `undo` lesen Pläne der Versionen 1 und 2; ein Build, der nur Version 1 kennt, lehnt v2-Pläne mit klarer Meldung ab. Neue `kind`-Werte: `junk`, `empty-dirs`, `archive`, `versions`. Neue Aktionstypen:

```json
{ "id": 1, "type": "remove-dir", "path": "C:\\...\\leer", "mtime": "…", "reason": "empty-dir" }
{ "id": 2, "type": "move", "path": "C:\\...\\Projekt2019", "target": "C:\\...\\_Archiv\\2019\\Projekt2019",
  "is_dir": true, "files": 412, "size": 81234567, "mtime": "…", "reason": "archive:older-than-2y" }
{ "id": 3, "type": "move", "path": "C:\\...\\Bericht_v1.docx", "target": "C:\\...\\_Archiv\\Versionen\\…\\Bericht_v1.docx",
  "size": 12345, "mtime": "…", "hash": "blake3:…", "keep": "C:\\...\\Bericht_final.docx", "reason": "older-version" }
```

### Config (Erweiterung)
- `junk`: zusätzliche Kategorien bzw. Muster, Mindestalter für Installer
- `junk.downloads_dirs`: Downloads-Ordner überschreiben bzw. ergänzen (Default: Known Folder)
- `archive.older_than`: Default-Schwelle (2 Jahre)
- `versions.min_age`: Mindestalter älterer Versionen (30 Tage)
- `onedrive.max_move_files`, `onedrive.max_move_bytes`: Obergrenze, ab der `--allow-large` nötig ist (Default 1.000 Dateien oder 5 GB, was zuerst erreicht wird)

## Acceptance Criteria
- [ ] Given ein indizierter Ordner, when ein beliebiges `plan junk|empty-dirs|archive|versions` läuft, then entsteht eine Plan-Datei und im Ordner ändert sich kein Byte
- [ ] Given Dateien aller Müll-Kategorien, when `plan junk --category temp` läuft, then stehen nur die Temp-Dateien als `quarantine` im Plan; ohne `--category` stehen alle Kategorien drin
- [ ] Given ein in der Config ergänztes Müll-Muster, when `plan junk` läuft, then werden passende Dateien erkannt
- [ ] Given ein Installer in Downloads jünger als die Schwelle, when `plan junk` läuft, then ist er nicht im Plan
- [ ] Given verschachtelte leere Ordner, when `plan empty-dirs` und `apply` laufen, then sind alle leeren Ordner entfernt und die Scan-Wurzel existiert noch
- [ ] Given ein Ordner, der nur `Thumbs.db` enthält, when `plan empty-dirs` läuft, then ist er nicht im Plan
- [ ] Given ein `empty-dirs`-Lauf, when `undo` läuft, then existieren alle Ordner wieder mit ursprünglichen Attributen und Zeitstempeln
- [ ] Given ein Ordner, dessen jüngste Datei älter als 2 Jahre ist, when `plan archive` und `apply` laufen, then liegt er unter `_Archiv\<Jahr>\<relativer Pfad>` mit unverändertem Inhalt; `undo` stellt ihn byteidentisch am Ursprungsort wieder her
- [ ] Given ein alter Ordner mit einem alten Unterordner, when `plan archive` läuft, then gibt es genau eine Aktion für den obersten Ordner
- [ ] Given eine Versionsgruppe (`Bericht_v1.docx`, `Bericht_v2.docx`, `Bericht final.docx`), deren ältere Versionen mindestens 30 Tage alt sind, when `plan versions` und `apply` laufen, then bleibt die jüngste Datei am Ort und die anderen liegen unter `_Archiv\Versionen\…`; `undo` stellt den Ausgangszustand her
- [ ] Given eine ältere Version jünger als `--min-age`, when `plan versions` läuft, then ist sie nicht im Plan und erscheint als übersprungen (`too-recent`)
- [ ] Given ein Plan mit `version: 1` aus Phase 2, when `apply` oder `undo` läuft, then funktioniert er unverändert
- [ ] Given gleich normalisierte Namen in verschiedenen Ordnern oder mit verschiedener Endung, when `plan versions` läuft, then bilden sie keine Gruppe
- [ ] Given ein bereits ausgeführter Archiv- oder Versionslauf, when `scan` und dasselbe `plan` erneut laufen, then ist der Plan leer (Idempotenz, `_Archiv` wird ausgelassen)
- [ ] Given ein Plan unter einer OneDrive-Wurzel über der Obergrenze, when `apply` ohne `--allow-large` läuft, then wird nichts verändert und der Grund gemeldet; mit `--allow-large` läuft er
- [ ] Given ein Lauf mit Quarantäne-Aktionen, when `undo` läuft, then bleiben unter `quarantine\<run-id>` keine leeren Ordner zurück
- [ ] Given geschützte Pfade oder Cloud-Platzhalter, when eine der neuen Aktionen plant oder angewendet wird, then gelten dieselben Regeln wie in Phase 2 (nie anfassen, als übersprungen melden, Apply prüft erneut)
- [ ] Integrationstests decken Plan → Apply → Undo für jede Aktion ab; `cargo clippy -- -D warnings` und `cargo test` sind grün
- [ ] Manueller Test im OneDrive-Testordner (wie Phase 2) und ein echter Lauf auf einem Alltagsordner (z. B. Downloads), jeweils mit `undo`, sind dokumentiert

## Edge Cases & Error States
| Scenario | Expected Behavior |
|----------|-------------------|
| Leerer Ordner ist beim Apply nicht mehr leer | Aktion als `stale` überspringen, Rest ausführen |
| Ordner enthält nur Cloud-Platzhalter, versteckte oder System-Dateien | Gilt als nicht leer |
| Leerer Ordner ist geschützt (z. B. unter `.git`) oder ein Projektordner | Nicht im Plan |
| Scan-Wurzel ist leer | Wird nie entfernt |
| `undo` von `remove-dir`: Pfad existiert inzwischen als Datei | Kollision melden, nichts überschreiben |
| `undo` von `remove-dir`: Ordner existiert schon | Als „bereits vorhanden“ melden, Attribute nicht überschreiben |
| Archiv-Kandidat enthält einen geschützten Unterordner, Junction/Symlink oder Cloud-Platzhalter | Ordner nicht als Ganzes verschieben, als übersprungen mit Grund melden; sein Elternordner ist dann auch kein Kandidat |
| Archiv-Ordner hat sich seit `plan` verändert (Dateianzahl, Summe, jüngste mtime) | `stale`, überspringen |
| Ziel in `_Archiv` existiert schon | Nie überschreiben oder zusammenführen; Aktion überspringen und melden |
| Ziel unter `_Archiv` wäre länger als 260 Zeichen | `\\?\`-Pfade; bei Problemen überspringen und melden |
| Ein Ordner im Archiv-Kandidaten ist gesperrt (offene Datei) | Move schlägt fehl, Journal vermerkt Fehler, Lauf geht weiter; kein Teilverschieben |
| Versionsgruppe: neueste Datei ist ein Cloud-Platzhalter | Ganze Gruppe überspringen (mtime nicht vertrauenswürdig ohne lokalen Inhalt) |
| Versionsgruppe: gleiche mtime bei mehreren Dateien | Deterministische Auswahl (z. B. lexikografisch letzter Name), im Plan sichtbar |
| `keep`-Datei fehlt beim Apply | Alle Aktionen dieser Gruppe überspringen |
| Versionsheuristik gruppiert falsch | Kein Datenverlust (nur Verschieben ins Archiv, Undo möglich); der Nutzer prüft den Plan |
| Müll-Datei ist gesperrt (z. B. `~$`-Lockdatei eines offenen Dokuments) | Aktion fehlgeschlagen, Lauf geht weiter |
| Downloads-Ordner nicht ermittelbar oder nicht unter der Wurzel | Kategorie `installer` liefert nichts, Hinweis in der Zusammenfassung |
| OneDrive-Plan über der Obergrenze | `apply` verweigert ohne `--allow-large` vor der ersten Aktion |
| `_Archiv` liegt auf anderem Volume | Kann nicht passieren (immer unter der Wurzel); Moves über Volumes werden verweigert |

## Technical Constraints
- Performance: Planerstellung für 100.000 Dateien in unter 10 Sekunden (nur Index-Zugriff, wie Phase 2). Archiv-Moves ganzer Ordner sind ein einzelnes Rename auf demselben Volume.
- Security: Keine harten Löschungen außer `remove-dir` auf nachweislich leeren Ordnern (beim Apply erneut geprüft) und `purge`. Alle Schutzregeln aus Phase 2 gelten unverändert, auch für Ziele unter `_Archiv` und für alle Pfade innerhalb eines verschobenen Ordners.
- Compliance: Alles bleibt lokal, keine Telemetrie. Journal enthält nur Pfade, Metadaten und Hashes.
- Zuverlässigkeit: Write-ahead-Journal für alle neuen Aktionstypen; Undo funktioniert nach Absturz auch für `remove-dir` und `move`.
- Rückwärtskompatibilität: Pläne und Journale aus Phase 2 bleiben lesbar und undo-fähig.
- Umsetzung inkrementell: `junk` → `empty-dirs` → `archive` → `versions`, jede Aktion einzeln lauffähig und getestet.

## Dependencies
- Phase 2: Plan-Format, `apply`, `undo`, `runs`, `purge`, Journal, Quarantäne, Schutzregeln ([`aenderungsplan-apply-undo.md`](aenderungsplan-apply-undo.md))
- Phase 1: Index, Namensheuristik für Versionen (`analysis/similar.rs`), Erkennung von Problemdateien und leeren Ordnern ([`scan-analyse-bericht.md`](scan-analyse-bericht.md))
- Config-Datei aus Phase 2 (Erweiterung um `junk`, `archive`, `onedrive`)

## Open Questions
- [x] Plan-`version`: auf 2 erhöhen, Phase-2-Pläne (v1) bleiben lesbar
- [x] OneDrive-Obergrenze: 1.000 Dateien oder 5 GB (was zuerst erreicht wird), per Config änderbar
- [x] Downloads-Ordner: Known Folder API (`FOLDERID_Downloads`), per Config überschreib- und erweiterbar
- [x] Archiv-Alter: nur jüngste mtime, keine atime
- [x] Versionen: Mindestalter 30 Tage für ältere Versionen (`--min-age`, Config), keine Mindestanzahl
