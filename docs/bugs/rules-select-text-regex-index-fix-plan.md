---
title: "Fix-Plan: --rule / only bricht text_regex"
status: fixed
fixed_at: 2026-10-05
created: 2026-10-05
updated: 2026-10-05
analysis: rules-select-text-regex-index-analysis.md
---

# Fix-Plan: `Rule.index` nach `select` neu vergeben

## Ursache (Kurzfassung)
`RuleSet::select` (`src/rules/mod.rs:150-154`) filtert, lässt aber `Rule.index` auf der Position in der Originaldatei. `plan_rules` indiziert `hits` nach Position in der gefilterten Liste (`src/change/rules.rs:355-360`), `evaluate` liest nach `self.index` (`src/rules/mod.rs:173`, `src/rules/condition.rs:296`).

## Gewählter Fix
`select` vergibt `index` nach dem Filtern fortlaufend neu (`rule.index = position`). Damit gilt überall „Index = Position in der Liste“, wie bei einer frisch geparsten Datei. Kleinste Änderung, keine Anpassung am Planer nötig.

Verworfen: `hits` nach `rule.index` indizieren. Dann müsste `hits` die Länge `max(index)+1` haben, und jede Stelle, die über `rules.rules` iteriert, müsste zwischen Position und Index unterscheiden.

## Vorab prüfen
`Rule.index` auf weitere Verwendungen untersuchen (`grep "\.index"` in `src/rules`, `src/change/rules.rs`, `src/gui`, `src/report`). Bisher gefunden: `src/rules/mod.rs:173, 184` (Auswertung) und der Test `src/rules/mod.rs:525`. Falls irgendwo der Originalindex gebraucht wird (z. B. Anzeige „Regel 3“), ein getrenntes Feld einführen statt `index` zu überschreiben.

## Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/rules/mod.rs` | `select` (Z. 150-159) | nach `filter(...).collect()` über die Regeln iterieren und `rule.index = i` setzen (`let mut rules: Vec<Rule> = …; for (i, r) in rules.iter_mut().enumerate() { r.index = i; }`) |
| `src/rules/mod.rs` | Doc-Kommentar zu `Rule.index` (Z. 86) | festhalten: „Position in der aktuellen Regelmenge; `select` vergibt neu“ |

## Tests
1. **Regression** (`src/rules/mod.rs` Tests): `select(&["B"])` auf `[A, B]` → `only.rules[0].index == 0`.
2. **Ende-zu-Ende** (`src/change/rules_tests.rs`): zwei Regeln, B mit `text_regex`, Textquelle liefert B-Treffer. `plan_rules` mit voller Menge und mit `select(&["B"])` liefert dieselbe Zuordnung für die Datei. Vor dem Fix rot (Datei unzugeordnet).
3. Zusatzfall: zwei `text_regex`-Regeln, `select` auf die zweite → Treffer der ersten beeinflusst das Ergebnis nicht.
4. Bestehender Test `select_filtert_und_meldet_unbekannte_regeln` und der Index-Test bei `mod.rs:525` bleiben grün.

## Risiken
- Nur Auswirkungen, wo der Originalindex genutzt wird (siehe „Vorab prüfen“).
- Planergebnis ändert sich nur für `--rule`/`only`, und zwar in Richtung „wie ohne Filter“.

## Rollback
Ein Commit, `git revert`. Ohne Fix bleibt `--rule` mit `text_regex` wirkungslos oder falsch.
