---
title: "Bug: --rule / only bricht text_regex wegen Rule.index"
status: fixed
created: 2026-10-05
updated: 2026-10-05
---

# Bug: `--rule` / `only` bricht `text_regex`

## Observed Behavior
Bei `plan rules --rule B` (oder `only` in der GUI) matcht eine `text_regex`-Regel B nie, wenn vor ihr in der Datei weitere Regeln stehen. Je nach Reihenfolge liest sie stattdessen das Textergebnis einer anderen Regel.

## Expected Behavior
Mit gefilterter Regelmenge liefert B dieselben Treffer wie mit der vollen Datei.

## Reproduction Steps
1. `rules.toml` mit `[A (ohne text_regex), B (mit text_regex)]`.
2. `ordner-cleanup plan rules --rule B`, Wurzel mit einer Datei, deren Text B trifft.
3. Ergebnis: Datei bleibt ungematcht. Mit `plan rules` (ohne `--rule`) wird sie verschoben.

## Environment
- Branch: `dev` (553ecdf)
- Relevant config: Regeldatei mit `text_regex`

## Root Cause Hypotheses
| # | Hypothesis | Probability | Evidence |
|---|-----------|-------------|----------|
| 1 | Zwei Indexräume. `Rule.index` ist die Position in der Originaldatei (`rules/file.rs:119-136, 297`) und bleibt nach `select` erhalten. `plan_rules` legt `hits` aber mit `rules.rules.len()` (Länge der gefilterten Liste) an und schreibt `hits[*r]` mit `r` = Position in der gefilterten Liste. `evaluate` liest `hits.get(self.index)` mit dem Originalindex. | High | `src/rules/mod.rs:150-154` (kein Neu-Indizieren). `src/change/rules.rs:355-360` (Schreiben nach Listenposition). `src/rules/mod.rs:173` und `src/rules/condition.rs:296` (Lesen nach `rule_index`). Fehlender Index → `None` → `NoMatch`. |
| 2 | Testlücke: `select_filtert_und_meldet_unbekannte_regeln` prüft nur Länge und Name, nicht `index` oder die Auswertung danach. | High | `src/rules/mod.rs:407-416`. Der Test bei `mod.rs:525` erwartet `index` nur für ungefilterte Mengen. |

Fälle:
- Gefilterte Liste kleiner als Originalindex → `get` liefert `None` → Regel matcht nie (stiller Verlust).
- Originalindex < Länge der gefilterten Liste, aber ≠ Position → liest das Ergebnis einer anderen Regel. Für Regeln ohne `text_regex` ist das Ergebnis `false`, bei zwei `text_regex`-Regeln kann ein fremder Treffer Dateien ins falsche Ziel schicken.

Hinweis: `matches_before_text` setzt ebenfalls `rule_index`, benutzt `hits` aber nicht. Die Dwell-/Text-Pfade sind nur über `evaluate` betroffen.

## Affected Code
| File | Line | Description |
|------|------|-------------|
| `src/rules/mod.rs` | 150-159 | `select` filtert, ohne `index` neu zu vergeben |
| `src/rules/mod.rs` | 169-177 | `evaluate` übergibt `self.index` als `rule_index` |
| `src/rules/condition.rs` | 296 | `text_hits.get(ctx.rule_index)` |
| `src/change/rules.rs` | 355-363 | `hits` nach Listenposition gefüllt |
| `src/ops/plan.rs` | 506-508 | ruft `select` auf |

## Notes
Lösungsrichtungen für `/debug:fix-plan`:
- (a) `select` vergibt `index` nach dem Filtern neu (`rules[i].index = i`). Kleinste Änderung; prüfen, ob `index` sonst irgendwo als Position in der Originaldatei gebraucht wird (z. B. Fehlermeldungen/GUI). Bisher nur die oben genannten Fundstellen gefunden (nicht über alle Dateien geprüft).
- (b) `hits` nach `rule.index` statt Listenposition indizieren.

Regressionstest: `RuleSet` mit zwei Regeln, `select(&["B"])`, `plan_rules` mit Text, der B trifft.
