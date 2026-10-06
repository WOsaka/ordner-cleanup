---
title: "Fix-Plan: Pläne in „Zuletzt erzeugte Pläne“ entdoppeln"
status: fixed
fixed_at: 2026-10-05
created: 2026-10-05
updated: 2026-10-05
analysis: recent-plans-duplicates-analysis.md
---

# Fix-Plan: Nach Pfad entdoppeln

## Gewählter Fix
In `recent_plans` (`src/gui/views/cleanup.rs:44`) die gesammelten Dateien nach `paths::path_key` entdoppeln, bevor sortiert und auf `limit` gekürzt wird. Das deckt jede Überlappung der Suchordner ab (Standard-`_gui` unter `plans\`, aber auch ein eigenes `[gui] plans_dir` innerhalb von `plans\`).

Verworfen: `_gui` aus dem Unterordner-Durchlauf ausnehmen. Das hängt am Standardpfad und würde ein konfiguriertes `plans_dir` unter `plans\` weiter doppelt zeigen.

## Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/gui/views/cleanup.rs` | `recent_plans` (Z. 44-72) | `let mut seen = HashSet::new();` und nach dem Sammeln `found.retain(\|p\| seen.insert(paths::path_key(&p.path)));` vor `sort_by_key`/`truncate`. `HashSet` zum `use` ergänzen. |

## Tests (vor dem Fix rot)
In den Tests von `cleanup.rs`:
- Zwei Suchordner, einer liegt unter dem anderen (`plans` und `plans\_gui`), je eine Plan-Datei → `recent_plans(&[plans\_gui, plans], 10)` liefert jede Datei genau einmal (heute: die Datei aus `_gui` doppelt).
- Das Kürzen auf `limit` zählt nach dem Entdoppeln (zwei Dateien, `limit = 2` → beide, nicht eine doppelte).
- Bestehende Tests bleiben unverändert grün.

## Risiken
Keine. Die Reihenfolge (neueste zuerst) und die Auswahl bleiben gleich, nur Dubletten entfallen. `path_key` ist unabhängig von Groß-/Kleinschreibung und Schreibweise des Pfads.

## Rollback
`git revert`.
