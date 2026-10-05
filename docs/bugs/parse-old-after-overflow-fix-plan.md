---
title: "Fix-Plan: Überlauf in parse_old_after"
status: fixed
fixed_at: 2026-10-05
created: 2026-10-05
updated: 2026-10-05
analysis: parse-old-after-overflow-analysis.md
---

# Fix-Plan: `checked_mul`

## Gewählter Fix
In `src/analysis/age.rs:65` `Ok(number * factor)` ersetzen durch `number.checked_mul(factor).ok_or_else(err)`. Zu große Eingaben ergeben dieselbe Fehlermeldung wie andere ungültige Dauern. Weil `Config::load` (`validate_cleanup`, `src/config.rs:511`) und die Einstellungsprüfung (`settings/fields.rs:548`) `parse_old_after` aufrufen, werden zu große Werte schon beim Laden abgelehnt.

Verworfen: zusätzliches Tageslimit. `checked_mul` verhindert den Fehler vollständig, ein willkürliches Limit wäre eine Verhaltensänderung.

## Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/analysis/age.rs` | 65 | `number.checked_mul(factor).ok_or_else(err)` |

## Tests (vor dem Fix rot)
In `age.rs` neben den vorhandenen `rstest`-Fällen (Z. 120, 133):
- `parse_old_after("99999999999999999y")` → `Err` (heute Panic im Debug-Build).
- Grenzfälle: größtes passendes `y`-Vielfaches `i64::MAX / 365` → `Ok`; `+1` → `Err`. Analog für `m` (Faktor 30).
- `d` mit sehr großer Zahl bleibt `Ok` (`i64::MAX` Tage, Faktor 1).
- `Config::parse("archive_older_than = \"99999999999999999y\"")` → `Err` (`src/config.rs`-Tests).

## Risiken
Keine Verhaltensänderung für gültige Werte; nur Eingaben, die vorher Panic oder Wraparound auslösten, werden zu einem Fehler. Zu erwähnen: Release-Verhalten bei solchen Eingaben war nicht definiert.

## Rollback
`git revert`.
