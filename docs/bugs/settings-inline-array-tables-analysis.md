---
title: "Bug: Settings-Editor erkennt Regel-Listen in Inline-Schreibweise nicht"
status: fixed
created: 2026-10-05
updated: 2026-10-05
---

# Bug: Settings-Editor und Inline-Arrays (`rules = [ {…} ]`)

## Observed Behavior
Steht in `rules.toml` (oder `junk_rules` in der Config) die Liste in Inline-Schreibweise `rules = [ { name = "a", … } ]`, zeigt der Editor 0 Einträge. Hinzufügen konvertiert die Liste still, vergibt aber den Namen anhand der leeren Namensliste. Duplizieren, Verschieben, Löschen und Feld-Änderungen wirken nicht oder nur teilweise.

## Expected Behavior
Inline-Listen werden wie `[[rules]]`-Blöcke angezeigt und bearbeitet; Namen bleiben eindeutig.

## Reproduction Steps
1. `rules.toml` mit `rules = [ { name = "A", ext = ["jpg"], target = "x/" } ]` (gültiges TOML; `RuleSet::parse` liest es).
2. Regeln im Settings-Editor öffnen: Liste leer.
3. Regel „A“ hinzufügen: `unique_name` sieht keine vorhandenen Namen → zweite Regel „A“; nach der Konvertierung steht der Name doppelt, die Validierung meldet „Name kommt doppelt vor“.

## Environment
- Branch: `dev` (1fab064)
- Relevant: Dateien, die von Hand oder per anderem Werkzeug in Inline-Form geschrieben wurden. Commit `40dd709` hat nur den Panic beim Schreiben behoben.

## Root Cause Hypotheses
| # | Hypothesis | Probability | Evidence |
|---|-----------|-------------|----------|
| 1 | Lese-Helfer und Schreib-Helfer kennen unterschiedliche Formen. `array()` liefert nur `ArrayOfTables` (`doc.get(key)?.as_array_of_tables()`), `array_mut()` wandelt Inline-Arrays um. Eine Inline-Liste ist `Item::Value(Array)` und fällt beim Lesen durch. | High | `src/settings/tables.rs:9-11` (nur Lesen als `ArrayOfTables`), `:16-32` (Umwandlung beim Schreiben). Betroffen: `len`, `names`, `get_field`, `get_map`, `duplicate`, `move_by` (nutzen `array`), außerdem `remove` und `set_map` (`as_array_of_tables_mut` direkt, ohne Umwandlung). |
| 2 | `add` und `duplicate` berechnen `names` vor dem Umwandeln. | High | `tables.rs:83` (`names(doc, key)` vor `array_mut` in Zeile 89) und `:96`. Folge von 1; der Fehler verschwindet, wenn die Liste vor dem Lesen umgewandelt wird. |
| 3 | Der Test deckt nur den Schreibpfad ab. | Med | `tables.rs:318` `inline_liste_wird_beim_hinzufuegen_umgewandelt_statt_zu_paniken`. Kein Test, der eine Inline-Liste anzeigt, löscht oder verschiebt. |

## Affected Code
| File | Line | Description |
|------|------|-------------|
| `src/settings/tables.rs` | 9-11 | `array()` nur für `ArrayOfTables` |
| `src/settings/tables.rs` | 43-62, 83, 96, 119-123, 134-143, 145-147, 162-173, 194-198 | Leser und Schreiber ohne Umwandlung |
| `src/settings/tables.rs` | 16-32 | `array_mut` wandelt um |

## Notes
Lösungsrichtungen für `/debug:fix-plan`:
- (a) Das Dokument beim Öffnen im Editor einmal normalisieren (Inline-Liste → `[[…]]`, `normalize(doc, key)`), danach bleiben alle Helfer unverändert. Der Ort ist die Stelle, an der der `DocumentMut` erzeugt wird (`src/gui/views/settings/…`, `doc_editor.rs`). Vorteil: eine Änderung, keine Doppelpfade. Nachteil: die Datei wird beim ersten Speichern umformatiert (Kommentare bleiben).
- (b) Lese-Helfer auf `&mut DocumentMut` umstellen und vorher umwandeln. Mehr Änderungen an Signaturen.

Empfehlung: (a). Regressionstests: Inline-Dokument → `len`, `names`, `add` (eindeutiger Name), `remove`, `move_by`, `duplicate`.
