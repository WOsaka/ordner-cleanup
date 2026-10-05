---
title: "Bug: Lokal vorhandene OneDrive-Dateien gelten als nicht lokal"
status: fixed
created: 2026-10-05
updated: 2026-10-05
---

# Bug: Lokal vorhandene OneDrive-Dateien gelten in `open_local` als „nicht lokal“

## Observed Behavior
`open_local` (`src/scan/exif.rs:81`) lehnt jede Datei mit gesetztem `FILE_ATTRIBUTE_REPARSE_POINT` mit `ExifError::NotLocal` ab. Dateien in OneDrive tragen dieses Attribut auch dann, wenn ihr Inhalt lokal liegt. Folgen je Aufrufer:
- `plan rules` (`src/change/rules.rs:792`): EXIF-Datum fehlt, Datei landet als `CloudPlaceholder` bei den übersprungenen.
- PDF-, Office-, Bild-, Video-Extraktoren und OCR (`src/content/extract/*.rs`, `src/platform/ocr.rs:160`): Status `Unreadable("cloud-placeholder")`, die Datei bleibt unklassifiziert und wird bei jedem Lauf erneut versucht.

## Expected Behavior
Nur echte Links (Symlink/Junction) und nicht lokale Platzhalter (`RECALL_*`, `OFFLINE`) werden nicht geöffnet. Lokal vorhandene OneDrive-Dateien werden wie normale Dateien gelesen.

## Reproduction Steps
1. Ordner unter `%OneDrive%` mit Fotos/PDFs, die lokal vorhanden sind („Immer auf diesem Gerät behalten“ oder bereits geöffnet).
2. `scan` auf den Ordner, dann `classify` bzw. `plan rules` mit EXIF-Datum oder Kategorie in der Regel.
3. Ergebnis: Dateien erscheinen als Cloud-Platzhalter bzw. `cloud-placeholder`, obwohl sie lokal liegen.

Beleg am Entwicklungsrechner (PowerShell, `%OneDrive%` = `C:\Users\Oskar\OneDrive`, 35 Dateien gelesen): 33 haben die Attribute `Archive, ReparsePoint` (lokal, kein Recall-Flag), 2 haben Recall-Flags (nicht lokal), 1 nur `Archive`. Das heißt: 33 von 35 würden von `open_local` abgelehnt, obwohl `is_cloud_only()` für sie `false` ist.

## Environment
- Branch: `dev` (1fab064), Windows 11, OneDrive mit Dateien bei Bedarf
- Betrifft die Hauptzielumgebung (Dokumente/Fotos in OneDrive)

## Root Cause Hypotheses
| # | Hypothesis | Probability | Evidence |
|---|-----------|-------------|----------|
| 1 | Die Bedingung `attrs.0 & FileAttrs::REPARSE_POINT != 0` in `open_local` ist zu breit. Sie fängt jeden Reparse-Point, nicht nur Links. OneDrive-Platzhalter behalten den Reparse-Point (Cloud-Filter-Tag), auch wenn sie geladen sind. | High | `src/scan/exif.rs:86-89`. Der Scanner ist enger: `is_link` = `file_type.is_symlink()` (`src/scan/source.rs:76`), `cloud_only` = Recall-/Offline-Flags (`src/platform/mod.rs:29-31`); `apply` und `duplicates` nutzen `is_cloud_only()` ohne Reparse-Bit (`src/change/apply.rs:154`, `src/scan/duplicates.rs:103`). `src/platform/mod.rs:53` hält ausdrücklich fest, dass REPARSE_POINT allein nicht cloud-only ist. |
| 2 | Das Reparse-Bit wurde als Schutz gegen Links ergänzt, obwohl `meta.file_type().is_symlink()` (Symlink und Junction) davor schon greift. Es ist damit redundant für Links und schadet nur bei anderen Reparse-Tags (OneDrive, Dateideduplizierung u. a.). | Med | Zeilen 86-88 prüfen Symlink und Reparse-Bit nacheinander. Test `platzhalter_wird_nicht_geoeffnet` (`exif.rs:255`) deckt nur das `OFFLINE`-Flag ab, nie einen Reparse-Point. |

## Affected Code
| File | Line | Description |
|------|------|-------------|
| `src/scan/exif.rs` | 86-91 | zu breite Reparse-Bedingung |
| `src/change/rules.rs` | 792 | `NotLocal` → `ExifResult::Cloud` → Skip `CloudPlaceholder` |
| `src/content/extract/mod.rs` | 86 | `NotLocal` → `Unreadable("cloud-placeholder")` (wird wiederholt) |
| `src/platform/ocr.rs` | 160-161 | `NotLocal` → `Unavailable("cloud-placeholder")` |
| `src/scan/exif.rs` | 29 | Meldung „Cloud-Platzhalter oder Link“ |

## Notes
Lösungsrichtung für `/debug:fix-plan`: Reparse-Bedingung aus `open_local` entfernen (Links deckt `is_symlink()` ab, Platzhalter `is_cloud_only()`). Zu klären: Wollen wir andere Reparse-Tags weiter blockieren (z. B. über das Reparse-Tag)? Vorschlag: nein, Konsistenz mit Scanner und `apply`. Test: Datei mit gesetztem REPARSE_POINT-Attribut ohne Recall-Flags ist lesbar (Attribut per `SetFileAttributesW` ist für REPARSE_POINT nicht setzbar, deshalb ggf. echten Symlink und Junction als Gegenprobe nutzen, und den Fall über eine Hilfsfunktion `is_unreadable(attrs, file_type)` testen).

Auswirkung ist auf Windows-Rechnern ohne OneDrive nicht sichtbar.
