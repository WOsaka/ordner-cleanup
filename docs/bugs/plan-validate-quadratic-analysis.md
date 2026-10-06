---
title: "Bug: Plan::validate ist quadratisch"
status: fixed
created: 2026-10-05
updated: 2026-10-05
---

# Bug: `Plan::validate` ist O(n²)

## Observed Behavior
Für jede Aktion mit `target` durchläuft `validate` alle Aktionen (`self.actions.iter().any(...)`, `src/change/plan.rs:290-295`) und berechnet dabei für jede Ordner-Aktion `paths::path_key`. Bei 100.000 `move`-Aktionen (Regel-/Archiv-Plan) sind das etwa 10^10 Vergleiche je Aufruf. `validate` läuft in `from_json`/`Plan::load`, `apply_plan` und `save_subset`.

## Expected Behavior
Laden, Anwenden und Teilplan-Speichern bleiben bei großen Plänen schnell (linear bzw. n·Tiefe).

## Reproduction Steps
1. Plan mit z. B. 50.000 `move`-Aktionen mit `target` erzeugen (Regel-Plan über eine große Wurzel).
2. `ordner-cleanup apply plan.json` oder in der GUI öffnen.
3. Messung: Zeit in `Plan::validate` wächst quadratisch (Test unten).

## Environment
- Branch: `dev` (1fab064), Windows
- `tests/perf_plans.rs` vorhanden (Leistungstests für Pläne), prüft `validate` bisher nur bei der Planung mit wenigen Ordner-Aktionen (nicht geprüft)

## Root Cause Hypotheses
| # | Hypothesis | Probability | Evidence |
|---|-----------|-------------|----------|
| 1 | Die Prüfung „Ziel liegt in einem Ordner, der selbst verschoben wird“ sucht je Aktion in allen Aktionen, statt die verschobenen Ordner einmal vorzuberechnen. Dazu wird `path_key` in der inneren Schleife jedes Mal neu gebildet. | High | `src/change/plan.rs:281-301`. Die Schleife ist nur bei `is_dir`-Aktionen fachlich nötig, wird aber trotzdem für alle `target`-Aktionen vollständig durchlaufen (`o.is_dir && …` filtert erst im Rumpf). |
| 2 | Weitere O(n)-Pfade sind unkritisch: `ids`, `sources`, `targets` sind `HashSet`s. | Low | `plan.rs:233-271`. |

## Affected Code
| File | Line | Description |
|------|------|-------------|
| `src/change/plan.rs` | 281-301 | innere Schleife je Aktion |

## Notes
Lösungsrichtung für `/debug:fix-plan`: Einmal vorab `HashMap<String, u32>` der Schlüssel aller `is_dir`-Aktionen (Pfad-Schlüssel → id) bilden. Je Aktion mit Ziel die Vorfahren von `target_key` ablaufen (je Ebene ein Lookup, Ergebnis nur zählen, wenn `id != a.id`). Kosten O(n·Tiefe). Ohne `is_dir`-Aktionen entfällt die Prüfung ganz. Vorher klären, ob `paths::is_under` echte Vorfahren oder auch Gleichheit zählt (Verhalten bei Gleichheit muss gleich bleiben).

Tests: bestehende `validate`-Tests unverändert grün (Ziel in verschobenem Ordner, Gleichheit, eigene id); neuer Test mit 100.000 Aktionen und Zeitlimit großzügig (z. B. < 2 s), als Leistungstest in `tests/perf_plans.rs`, falls dort Platz ist.
