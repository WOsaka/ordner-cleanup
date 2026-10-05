---
title: "Fix-Plan: Inline-Listen im Settings-Editor"
status: fixed
fixed_at: 2026-10-05
created: 2026-10-05
updated: 2026-10-05
analysis: settings-inline-array-tables-analysis.md
---

# Fix-Plan: Inline-Listen beim Parsen normalisieren

## Gewählter Fix (Variante a)
Beim Erzeugen des `DocumentMut` für die Editoren werden Listen in Inline-Schreibweise einmal in `[[…]]`-Blöcke umgewandelt. Danach sehen Lese- und Schreib-Helfer in `src/settings/tables.rs` nur noch `ArrayOfTables`.

Zentraler Ort: `parse_document` in `src/settings/mod.rs:122`. Dort nach dem Parsen für die Schlüssel `rules` und `junk_rules` `tables::normalize(&mut doc, key)` aufrufen. Verworfen: Lese-Helfer auf `&mut DocumentMut` umbauen (Variante b), das ändert viele Signaturen.

## Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/settings/tables.rs` | neu | `pub fn normalize(doc: &mut DocumentMut, key: &str)`: existiert der Schlüssel und ist er ein Inline-Array, `array_mut(doc, key)` aufrufen (wandelt um, lässt andere Formen unangetastet); legt nichts an (`existing_array_mut` statt `array_mut`) |
| `src/settings/mod.rs` | `parse_document` (Z. 122) | nach dem Parsen `normalize` für `rules` und `junk_rules` |

Vorab prüfen: Ob `parse_document` auch für reine Validierung benutzt wird (`config_text`/`validate.rs`). Dort schadet die Umwandlung nicht, solange der Text nicht daraus zurückgeschrieben wird. Falls doch, `normalize` nur am Editor-Einstieg (`doc_editor.rs`) aufrufen.

## Tests (alle zuerst rot)
1. `tables.rs`: Dokument mit `rules = [ { name = "A", ext = ["jpg"] } ]` → nach `normalize`: `len == 1`, `names == ["A"]`.
2. Danach `add(…, "A", …)` → Name „A 2“; `duplicate`, `move_by` (zwei Einträge), `remove` wirken.
3. Dokument ohne Schlüssel bleibt leer (kein `rules` angelegt); Dokument mit `rules = 5` (falsche Form) bleibt unverändert.
4. `parse_document` mit Inline-Liste liefert ein Dokument, dessen `to_string()` `[[rules]]` enthält.
5. Bestehender Test `inline_liste_wird_beim_hinzufuegen_umgewandelt_statt_zu_paniken` bleibt grün.

## Risiken
- Die Datei wird beim ersten Speichern aus dem Editor in die Block-Schreibweise umformatiert (Kommentare im Dokument bleiben). Inhaltlich identisch.
- Rein lesende Pfade, die Inline-Listen auswerten (`RuleSet::parse`), bleiben unberührt.

## Rollback
`git revert`; die Helfer verhalten sich dann wieder wie bisher.

## Abweichung bei der Umsetzung
Normalisiert wird nicht in `parse_document`, sondern in `EditableFile::load_or_empty` und im Doc-Editor beim Wechsel von Roh- zu Formularansicht (`EditableFile::normalize_lists`, Schlüssel `rules` und `junk_rules`). `set_text` und das Speichern aus der Roh-Ansicht lassen den Text des Nutzers unverändert, sonst wäre die Roh-Ansicht nach dem Speichern als „geändert“ erschienen (`is_dirty` vergleicht den Roh-Text mit dem gespeicherten). Zusätzlich räumt `array_mut` die Schlüssel-Dekoration auf, damit die Umwandlung `[[rules]]` statt `[[rules ]]` schreibt.
