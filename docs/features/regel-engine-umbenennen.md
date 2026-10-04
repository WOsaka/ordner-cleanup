---
title: "Regel-Engine & Umbenennen (Phase 4)"
status: draft          # draft | approved | implemented
created: 2026-10-04
updated: 2026-10-04
---

# Feature: Regel-Engine & Umbenennen (Phase 4)

## Problem Statement
Phase 1 bis 3 analysieren und räumen auf, ordnen aber nichts neu. Dateien in Downloads, Desktop oder Scan-Ordnern landen weiterhin von Hand (oder gar nicht) am richtigen Ort und tragen uneinheitliche Namen (`Rechnung final (2).pdf`, `IMG_1234.jpg`, `Scan 03.10.2026.pdf`). Wer Ordnung halten will, sortiert jedes Mal neu und benennt einzeln um.

Betroffen ist zunächst der Entwickler selbst, später Kollegen. Phase 4 liefert eine wiederholbare Regeldatei, die Dateien einsortiert und Namen vereinheitlicht, auf Basis des Plan → Apply → Undo-Fundaments aus Phase 2 und 3 (siehe [`../roadmap.md`](../roadmap.md)).

## Goals
- [ ] Regeldatei (TOML, `[[rules]]`) mit Bedingungen, Ziel-Vorlage mit Platzhaltern und optionaler Namens-Normalisierung
- [ ] `plan rules` erzeugt aus Index und Regeln einen Plan mit `move`-Aktionen (Umbenennen = `move` im selben Ordner)
- [ ] Platzhalter `{exif.date:<format>}` liest das Aufnahmedatum aus Bildern, mit Fallback
- [ ] Idempotenz: Ein zweiter `plan rules` nach `apply` und `scan` mit denselben Regeln erzeugt einen leeren Plan
- [ ] Ein echter Alltagsordner (z. B. Downloads) lässt sich mit 5 bis 10 Regeln vollständig einsortieren
- [ ] Jeder Lauf ist per `undo` vollständig umkehrbar; alle Garantien aus Phase 2/3 gelten (Dry-Run, Stale-Prüfung, Schutzregeln, Cloud-Platzhalter, Write-ahead, OneDrive-Obergrenze)

## Out of Scope
- Zielstruktur-Vorlagen (PARA, Johnny.Decimal) und Soll/Ist-Abgleich (Phase 5)
- Inhaltsbasierte Klassifikation (PDF-/Office-Metadaten, Textextraktion), KI-Vorschläge (Phase 6)
- Watch-Mode, geplante Läufe (Phase 5)
- Regeln, die löschen oder in die Quarantäne verschieben (bleibt bei `plan junk`)
- Regeln auf Ordnern (nur Dateien)
- Ziele außerhalb der Scan-Wurzel, Moves über Volumes
- Aufnahmedatum aus Videos (MP4/MOV); dort greift der Fallback
- Verschachtelte Bedingungen (`any`/`all`), mehrere Aktionen pro Regel, Regel-Ketten
- YAML als Regelformat (Entscheidung siehe Open Questions)

## User Journey
1. Der Nutzer schreibt eine Regeldatei. Default: `<ORDNER_CLEANUP_HOME>\rules.toml`; alternativ `--rules <datei>`.
2. Er indiziert den Ordner mit `scan <pfad>`.
3. Er erzeugt einen Plan (Dry-Run, nichts wird verändert):
   `ordner-cleanup plan rules <pfad> [--rules <datei>] [--rule <name>…] [--out <plan.json>]`
4. Das System prüft zuerst die Regeldatei vollständig (Syntax, unbekannte Schlüssel, unbekannte Platzhalter, ungültige Regex/Globs/Altersangaben, unzulässige Ziele). Bei Fehlern bricht es mit Datei, Zeile und Regelname ab, bevor ein Plan entsteht.
5. Das System wendet die Regeln auf alle Dateien im Index an und gibt eine Zusammenfassung aus: Aktionen je Regel, betroffene Bytes, übersprungene Einträge nach Grund, Dateien ohne passende Regel (Anzahl), eine Vorschau mit einigen Beispielen „alt → neu“ je Regel, Pfad der Plan-Datei. Unter einer OneDrive-Wurzel erscheinen die Warnung aus Phase 2 und ggf. der Hinweis auf `--allow-large`.
6. Der Nutzer prüft die Plan-Datei.
7. `ordner-cleanup apply <plan.json> [--yes] [--allow-large]` führt die Moves aus wie in Phase 3 (Stale-Prüfung, Write-ahead-Journal, Schutzregeln erneut prüfen, Zielordner bei Bedarf anlegen).
8. `undo <run-id>` dreht den Lauf zurück (Dateien an den alten Ort und alten Namen, neu angelegte, danach leere Zielordner werden entfernt); `runs` zeigt die Aktionen je Regel.

### Regeldatei
Flache Regeln, Reihenfolge = Priorität. Pro Datei gewinnt die **erste** Regel, deren Bedingungen **alle** zutreffen; weitere Regeln werden für diese Datei nicht geprüft.

```toml
[[rules]]
name   = "fotos"
ext    = ["jpg", "jpeg", "heic"]
target = "Fotos/{exif.date:%Y}/{exif.date:%Y-%m-%d}_{name}.{ext}"
exif_fallback = "mtime"     # mtime (Default) | skip
spaces = "_"

[[rules]]
name       = "rechnungen"
glob       = "Downloads/**"
name_regex = '(?i)rechnung[ _-]*(\d+)'
min_age    = "7d"
target     = "Finanzen/Rechnungen/{year}/"
iso_date_prefix   = true
strip_copy_suffix = true
```

**Bedingungen** (alle optional, UND-verknüpft; mindestens eine muss gesetzt sein):
- `glob`: Glob auf den Pfad relativ zur Scan-Wurzel (`/` und `\` gleichwertig, Groß-/Kleinschreibung egal)
- `ext`: Liste von Endungen ohne Punkt, Groß-/Kleinschreibung egal
- `name_regex`: Regex auf den Dateinamen inkl. Endung; Gruppen als `{1}`, `{2}` … in `target` nutzbar
- `min_age` / `max_age`: Alter nach mtime (`7d`, `6m`, `2y` wie in der Config)
- `min_size` / `max_size`: Größe (`10MB`, `1GB` wie in der Config)

**Ziel** `target`: Vorlage relativ zur Scan-Wurzel. Endet sie auf `/` (bzw. `\`), bleibt der Dateiname erhalten (reines Einsortieren). Sonst ist das letzte Segment der neue Dateiname. Ein Ziel ohne `/` ist ein Umbenennen im selben Ordner, wenn es mit `./` beginnt (z. B. `./{year}-{month}-{day}_{name}.{ext}`).

**Platzhalter:**
| Platzhalter | Wert |
|---|---|
| `{name}` | Dateiname ohne Endung |
| `{ext}` | Endung ohne Punkt (Original-Schreibweise, mit `lowercase_ext` klein) |
| `{parent}` | Name des aktuellen Elternordners |
| `{year}`, `{month}`, `{day}` | Aus der mtime, zweistellig bei Monat/Tag |
| `{exif.date:<format>}` | EXIF-Aufnahmedatum (`DateTimeOriginal`, sonst `DateTimeDigitized`), `chrono`-Format; ohne Format `%Y-%m-%d` |
| `{1}`, `{2}` … | Gruppen aus `name_regex` |

Hat eine Datei kein (lesbares) EXIF-Datum, greift `exif_fallback`: `mtime` (Default) setzt das Änderungsdatum ein, `skip` überspringt die Datei (`skipped: no-exif-date`).

**Normalisierung** (optional pro Regel, wird nach dem Einsetzen der Platzhalter auf den Dateinamen angewandt, nicht auf Ordnerteile):
- `iso_date_prefix = true`: stellt `YYYY-MM-DD_` voran (EXIF-Datum, falls die Regel `{exif.date}` nutzt, sonst mtime); entfällt, wenn der Name schon mit einem ISO-Datum beginnt
- `spaces = "_" | "-"`: Leerzeichen (auch mehrfache) ersetzen
- `umlauts = true`: ä→ae, ö→oe, ü→ue, ß→ss (auch Großbuchstaben)
- `strip_copy_suffix = true`: entfernt `- Kopie`, `- Kopie (n)`, ` (n)`, `- Copy`, `_final`, `_final_vN` am Namensende (gleiche Heuristik wie Phase 1/3)
- `lowercase_ext = true`: Endung kleinschreiben

### Plan-Format (Erweiterung)
Plan-Format bleibt `version: 2`. Die Aktion ist das bestehende `move` aus Phase 3, ergänzt um das optionale Feld `rule` (fehlt in älteren Plänen, Default leer):
```json
{ "id": 7, "type": "move", "path": "C:\\...\\Downloads\\Rechnung 4711 final.pdf",
  "target": "C:\\...\\Finanzen\\Rechnungen\\2026\\2026-09-30_Rechnung_4711.pdf",
  "size": 81234, "mtime": "…", "rule": "rechnungen", "reason": "rule:rechnungen" }
```

### Index (Erweiterung)
EXIF-Datum wird im Index zwischengespeichert (je Datei: Aufnahmedatum oder „kein EXIF“), gültig solange Größe und mtime unverändert sind. Ein erneuter `plan rules` liest dann keine Bilder erneut.

### Config (Erweiterung)
- `rules_file`: Pfad der Default-Regeldatei (Default `<ORDNER_CLEANUP_HOME>\rules.toml`)
- Obergrenze `onedrive_max_move_files` / `onedrive_max_move_bytes` aus Phase 3 gilt auch für `plan rules`

## Acceptance Criteria
- [ ] Given ein indizierter Ordner und eine gültige Regeldatei, when `plan rules` läuft, then entsteht eine Plan-Datei und im Ordner ändert sich kein Byte
- [ ] Given eine Regeldatei mit Syntaxfehler, unbekanntem Schlüssel, unbekanntem Platzhalter oder ungültiger Regex, when `plan rules` läuft, then bricht es mit Datei, Zeile und Regelname ab und schreibt keinen Plan
- [ ] Given ein Ziel mit absolutem Pfad, Laufwerksbuchstaben, UNC-Pfad oder `..`, when die Regeldatei geprüft wird, then ist das ein Fehler vor der Planung
- [ ] Given zwei Regeln, die beide auf eine Datei passen, when `plan rules` läuft, then gilt nur die erste
- [ ] Given `--rule fotos`, when `plan rules` läuft, then enthält der Plan nur Aktionen der Regel `fotos`
- [ ] Given eine Regel mit `glob`, `ext`, `name_regex`, Alter und Größe, when `plan rules` läuft, then werden nur Dateien geplant, die alle Bedingungen erfüllen
- [ ] Given `target` mit `{year}/{month}`, `{name}`, `{ext}`, `{parent}` und Regex-Gruppen, when `plan rules` läuft, then stehen die korrekt eingesetzten Ziele im Plan
- [ ] Given ein JPEG mit `DateTimeOriginal`, when eine Regel `{exif.date:%Y-%m-%d}` nutzt, then wird das Aufnahmedatum eingesetzt, nicht die mtime
- [ ] Given ein Bild ohne EXIF bzw. mit kaputtem EXIF, when `exif_fallback = "mtime"`, then wird die mtime eingesetzt; when `exif_fallback = "skip"`, then erscheint es als `skipped: no-exif-date`; in keinem Fall bricht der Lauf ab
- [ ] Given ein zweiter `plan rules` ohne Dateiänderungen, when er läuft, then werden keine Bilder erneut gelesen (EXIF-Cache)
- [ ] Given die Normalisierungsoptionen, when `plan rules` läuft, then entsprechen die Zielnamen den Regeln (z. B. `Rechnung März - Kopie (2).PDF` → `2026-09-30_Rechnung_Maerz.pdf`)
- [ ] Given am Ziel liegt bereits eine Datei mit identischem Inhalt, when `plan rules` läuft, then wird die Quelle als `skipped: duplicate-at-target` geführt
- [ ] Given am Ziel liegt eine Datei mit anderem Inhalt oder zwei Quellen ergeben dasselbe Ziel, when `plan rules` läuft, then werden die Ziele mit ` (2)`, ` (3)` … durchnummeriert, in stabiler Reihenfolge (zweimal geplant = identischer Plan)
- [ ] Given `plan rules` → `apply` → `scan` → `plan rules` mit denselben Regeln, when der zweite Plan entsteht, then enthält er keine Aktionen (Idempotenz)
- [ ] Given ein Ziel, das sich nur in Groß-/Kleinschreibung unterscheidet (`foto.JPG` → `foto.jpg`), when `apply` läuft, then ist die Datei umbenannt; `undo` stellt die alte Schreibweise wieder her
- [ ] Given ein Plan aus `plan rules`, when `apply` und danach `undo` laufen, then sind alle Dateien mit Name, Größe, mtime, Attributen und Inhalt am alten Ort und neu angelegte Zielordner sind wieder entfernt
- [ ] Given eine Quelle oder ein Ziel in einem geschützten Pfad, when `plan rules` läuft, then ist die Datei übersprungen mit Grund; `apply` prüft erneut
- [ ] Given eine Cloud-only-Datei, when `plan rules` läuft, then wird sie weder gelesen (auch kein EXIF) noch verschoben und erscheint als `skipped: cloud-placeholder`
- [ ] Given ein Plan unter einer OneDrive-Wurzel über der Obergrenze, when `apply` ohne `--allow-large` läuft, then verweigert es vor der ersten Aktion
- [ ] Given ein Phase-2/3-Plan ohne Feld `rule`, when `apply` oder `undo` läuft, then funktioniert er unverändert
- [ ] Manueller Test im OneDrive-Testordner und ein echter Lauf auf einem Alltagsordner (z. B. Downloads), jeweils mit `undo`, sind dokumentiert

## Edge Cases & Error States
| Scenario | Expected Behavior |
|----------|-------------------|
| Regeldatei fehlt (Default-Pfad, kein `--rules`) | Fehler mit Hinweis auf den erwarteten Pfad und ein Minimalbeispiel |
| Regeldatei ohne Regeln oder Regel ohne Bedingung | Fehler beim Prüfen |
| Doppelter Regelname | Fehler beim Prüfen (Namen sind Schlüssel für `--rule` und Zusammenfassung) |
| `--rule` mit unbekanntem Namen | Fehler mit Liste der vorhandenen Regeln |
| Regex-Gruppe `{3}` bei nur zwei Gruppen | Fehler beim Prüfen |
| `{exif.date}` mit ungültigem Format | Fehler beim Prüfen |
| Datei liegt bereits am Ziel (gleicher Pfad, gleiche Schreibweise) | Keine Aktion (Idempotenz) |
| Ziel = Quelle bis auf Groß-/Kleinschreibung | Echtes Umbenennen in zwei Schritten über einen temporären Namen; Journal vermerkt beide Schritte, Undo/Absturz-Recovery decken den Zwischenstand ab |
| Ziel existiert, gleicher Inhalt | `skipped: duplicate-at-target` (Aufräumen später per `plan dedupe`) |
| Ziel existiert, anderer Inhalt | Durchnummerieren ` (2)`, ` (3)` …, erstes freies Ziel |
| Mehrere Quellen ergeben dasselbe Ziel | Durchnummerieren in stabiler Reihenfolge (Quellpfad, case-insensitiv sortiert) |
| Ziel ist zwischen `plan` und `apply` entstanden | `apply` verschiebt nicht, Aktion `stale` (kein Überschreiben) |
| Quelle seit dem Scan verändert oder verschwunden | `stale`, Lauf geht weiter |
| Ziel enthält ungültige Zeichen (`<>:"|?*`, Steuerzeichen), reservierte Namen (`CON`, `NUL` …) oder endet auf Punkt/Leerzeichen | `skipped: invalid-target` mit Grund |
| Ziel länger als 260 Zeichen | `skipped: path-too-long` (Moves auf lange Pfade erzeugen Sync- und Explorer-Probleme) |
| Platzhalter ergibt leeren Ordner- oder Dateinamen (z. B. leere Regex-Gruppe) | `skipped: invalid-target` |
| Ziel liegt in geschütztem Pfad oder unter `_Archiv` / Quarantäne | `skipped: protected` |
| Regel verschiebt Dateien in einen Ordner, auf den dieselbe Regel wieder passt | Kein Problem: Datei liegt nach `apply` am Ziel, nächster Lauf erzeugt keine Aktion |
| Regel A verschiebt nach X, Regel B würde Dateien in X weiterverschieben | Nicht in einem Lauf (Plan basiert auf dem Index-Stand); beim nächsten Lauf greift B. Zusammenfassung weist auf Ziele hin, die von einer anderen Regel erfasst würden |
| Cloud-only-Platzhalter | `skipped: cloud-placeholder`, kein EXIF-Lesen |
| Gesperrte Datei beim `apply` | Aktion fehlgeschlagen, Journal vermerkt Fehler, Lauf geht weiter |
| EXIF-Daten kaputt, abgeschnitten oder extrem groß | Gilt als „kein EXIF“, Fallback; nur der Metadatenbereich wird gelesen (Obergrenze) |
| Link/Junction als Quelle oder auf dem Zielweg | Übersprungen wie in Phase 2/3 |

## Technical Constraints
- Performance: `plan rules` ohne `{exif.date}` auf 100.000 Dateien in unter 10 Sekunden (nur Index-Zugriff, wie Phase 3). EXIF wird nur gelesen, wenn alle übrigen Bedingungen einer Regel zutreffen und ihr Ziel bzw. `iso_date_prefix` es braucht; nur der Header, parallel (`rayon`), mit Cache im Index. Messung wie in Phase 3 dokumentieren.
- Security: Ziele sind immer relativ zur Scan-Wurzel und werden nach dem Einsetzen kanonisch geprüft (kein Ausbruch über Platzhalterwerte). Keine löschenden Aktionen. Schutzregeln gelten für Quelle und Ziel. `regex` hat lineare Laufzeit (kein ReDoS). EXIF-Parsing in reinem Rust, Fehler und Panics im Parser führen nie zum Abbruch.
- Compliance: Alles bleibt lokal, keine Telemetrie. Journal und Plan enthalten nur Pfade, Metadaten und Hashes.
- Zuverlässigkeit: Write-ahead-Journal für alle Moves inkl. Zwischenschritt beim Umbenennen nur in der Schreibweise; Undo funktioniert nach Absturz.
- OneDrive: Jedes Umbenennen erzeugt Sync-Traffic; Obergrenze und `--allow-large` aus Phase 3 gelten unverändert.
- Rückwärtskompatibilität: Pläne und Journale aus Phase 2/3 bleiben lesbar und undo-fähig; Index-Schema-Erweiterung mit Migration.
- Format: Regeldatei in TOML (bestehende `toml`-Crate), keine neue Parser-Abhängigkeit.

## Dependencies
- Phase 3: Aktion `move` inkl. Apply/Undo, Anlegen und Aufräumen von Zielordnern, OneDrive-Obergrenze, Plan-Format v2 ([`aufraeumaktionen.md`](aufraeumaktionen.md))
- Phase 2: `apply`, `undo`, `runs`, Journal, Schutzregeln, Cloud-Platzhalter-Erkennung ([`aenderungsplan-apply-undo.md`](aenderungsplan-apply-undo.md))
- Phase 1: Index (SQLite), Namensheuristik für Kopie-/Versions-Suffixe ([`scan-analyse-bericht.md`](scan-analyse-bericht.md))
- Neue Crate: `kamadak-exif` (EXIF aus JPEG, TIFF, HEIF/HEIC, PNG, WebP; MIT/BSD-2)
- Vorhanden: `toml`, `globset`, `regex`, `chrono`, `rayon`

## Open Questions
- [x] Regelformat: TOML statt YAML. Gründe: Platzhalter `{…}` am Wertanfang würden in YAML als Mapping gelesen, keine stillen Typumwandlungen (`no` → `false`), `serde_yaml` ist archiviert, Config und Regeln teilen Format und Parser. YAML erst neu prüfen, wenn spätere Phasen tiefe Verschachtelung brauchen.
- [x] EXIF-Cache: ja, im Index, gültig solange Größe und mtime gleich sind
- [x] Plan-Format: v2 mit optionalem Feld `rule`, kein v3
- [x] Videos: nicht in Phase 4 (`kamadak-exif`, Fallback für Videos)
- [x] Priorität: nächstes Feature, kein fester Termin
