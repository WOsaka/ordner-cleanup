---
title: "Fix-Plan: Lokal vorhandene OneDrive-Dateien werden gelesen"
status: fixed
fixed_at: 2026-10-05
created: 2026-10-05
updated: 2026-10-05
analysis: onedrive-hydrated-files-not-local-analysis.md
---

# Fix-Plan: Reparse-Bedingung aus `open_local` entfernen

## Gewählter Fix
In `src/scan/exif.rs:86-91` die Bedingung `attrs.0 & FileAttrs::REPARSE_POINT != 0` streichen. Übrig bleiben `attrs.is_cloud_only()` (Recall-/Offline-Flags) und `meta.file_type().is_symlink()` (Symlink und Junction). Das ist dieselbe Definition wie im Scanner (`is_link`, `cloud_only`) und in `apply`.

Verworfen: nach Reparse-Tag unterscheiden (nur Cloud-Tags erlauben). Mehr Code für Win32-Aufrufe ohne Nutzen, solange Links und Platzhalter schon über `is_symlink`/`is_cloud_only` erkannt werden.

## Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/scan/exif.rs` | 81-92 | Bedingung auf `attrs.is_cloud_only() \|\| meta.file_type().is_symlink()` kürzen; Doc-Kommentar der Funktion und Fehlertext (Z. 29) bleiben stimmig („Cloud-Platzhalter oder Link“) |

Die Aufrufer (`change/rules.rs`, `content/extract/*`, `platform/ocr.rs`) brauchen keine Änderung.

## Tests
1. **Regression** (`src/scan/exif.rs`-Tests): Ein Reparse-Point, der kein Symlink/Junction ist, lässt sich in einer Test-Umgebung nicht ohne Weiteres erzeugen (`REPARSE_POINT` ist per `SetFileAttributesW` nicht setzbar). Deshalb die Entscheidung in eine kleine reine Funktion auslagern: `fn blocks_open(attrs: FileAttrs, is_symlink: bool) -> bool`. Test: `0x20 | REPARSE_POINT` ohne Recall-Flags und kein Symlink → `false` (heute `true`); mit `RECALL_ON_DATA_ACCESS` → `true`; mit `is_symlink` → `true`. Vor dem Fix schreiben und rot sehen.
2. Echter Symlink bzw. Junction bleibt `NotLocal` (falls Symlink-Anlegen ohne Rechte scheitert, Test überspringen; Junction per `mklink /J` oder vorhandener Testhelfer `std_source_erkennt_junctions_als_link`).
3. Bestehender Test `platzhalter_wird_nicht_geoeffnet` bleibt grün.
4. Manuell am Entwicklungsrechner: `plan rules` auf einen OneDrive-Ordner mit lokal vorhandenen Fotos; EXIF-Datum wird gelesen (33 von 35 Dateien im Beispielordner waren betroffen).

## Risiken
- Andere Reparse-Tags (z. B. Dateideduplizierung) werden jetzt geöffnet. Lesen ist dort normal möglich; Platzhalter, die einen Download auslösen würden, tragen die Recall-Flags und bleiben blockiert.
- Rückfallrisiko: ein Platzhalter ohne Recall-Flag würde beim Öffnen einen Download auslösen. Das widerspricht der Annahme in `src/platform/mod.rs:29`, die der Scanner schon heute teilt.

## Rollback
Ein Commit, `git revert`.
