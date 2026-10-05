---
title: "Bug: „Zuletzt erzeugte Pläne“ zeigt Pläne aus dem GUI-Ordner doppelt"
status: analysis
created: 2026-10-05
updated: 2026-10-05
---

# Bug: „Zuletzt erzeugte Pläne“ (Aufräumen) listet GUI-Pläne doppelt

Gefunden im manuellen Test GUI Teil A (`docs/manual-tests.md`, Auffälligkeit 1).

## Observed Behavior
In der Ansicht „Aufräumen“ erscheint jeder Plan, der im GUI-Planordner liegt (Standard `%LOCALAPPDATA%\ordner-cleanup\plans\_gui`), zweimal in „Zuletzt erzeugte Pläne“. Die Liste ist auf 12 Einträge begrenzt, die Dubletten verdrängen dadurch andere Pläne.

Nicht betroffen: Pläne, die direkt in `plans\` liegen, und Pläne in `plans\<profil>` (werden nur einmal gefunden). Ein eigener `[gui] plans_dir` außerhalb von `plans\` erzeugt keine Dubletten.

## Expected Behavior
Jede Plan-Datei erscheint höchstens einmal.

## Reproduction Steps
1. In der GUI einen Plan erzeugen (landet in `plans\_gui`).
2. „Aufräumen“ öffnen, „Zuletzt erzeugte Pläne“ ansehen: derselbe Plan steht zweimal mit gleichem Pfad.

## Environment
- Branch: `dev` (4c1527f), Windows 11, Release-Build
- Relevant config: `[gui] plans_dir` nicht gesetzt (Standard)

## Root Cause Hypotheses
| # | Hypothesis | Probability | Evidence |
|---|-----------|-------------|----------|
| 1 | Die Suchordner überlappen und die Ergebnisse werden nicht nach Pfad entdoppelt. `plan_dirs()` liefert `[data\plans\_gui, data\plans]`. `recent_plans` liest jeden Ordner und zusätzlich eine Ebene Unterordner. `data\plans\_gui` wird so einmal als eigener Eintrag gelesen und einmal als Unterordner von `data\plans`. | High | `src/gui/views/cleanup.rs:74-84` (zwei Wurzeln), `:44-72` (Ordner plus Unterordner, kein Abgleich). Standardpfad: `src/paths.rs:157-160` (`plans\_gui`). Die vorhandenen Tests (`cleanup.rs:495-506`) übergeben nur einen Ordner. |

## Affected Code
| File | Line | Description |
|------|------|-------------|
| `src/gui/views/cleanup.rs` | 44-72 | `recent_plans` sammelt ohne Entdopplung |
| `src/gui/views/cleanup.rs` | 74-84 | `plan_dirs` mit überlappenden Wurzeln |

## Notes
Lösungsrichtung für `/debug:fix-plan`: In `recent_plans` nach dem Sammeln nach Pfad entdoppeln (`paths::path_key`), bevor sortiert und auf `limit` gekürzt wird. Das deckt auch andere Überlappungen ab (z. B. ein `plans_dir`, das in `plans\` liegt). Alternative, nur `_gui` aus dem zweiten Durchlauf ausnehmen, deckt weniger ab. Regressionstest: zwei Wurzeln, von denen eine unter der anderen liegt, mit je einer Plan-Datei → jede genau einmal.
