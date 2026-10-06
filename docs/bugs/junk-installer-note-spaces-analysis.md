---
title: "Bug: Lange Leerzeichenfolge im Installer-Hinweis"
status: fixed
created: 2026-10-05
updated: 2026-10-05
---

# Bug: Leerzeichenfolge im Hinweis „kein Downloads-Ordner“

## Observed Behavior
`plan junk` mit Kategorie `installer` und ohne Downloads-Ordner unter der Wurzel gibt in CLI, GUI und Plan-Zusammenfassung diesen Hinweis aus: `… unter <root> gefunden              (Known Folder oder downloads_dirs); …` mit etwa 14 Leerzeichen zwischen „gefunden“ und der Klammer.

## Expected Behavior
Ein einzelnes Leerzeichen: `… gefunden (Known Folder oder downloads_dirs); …`.

## Reproduction Steps
1. `plan junk --category installer` für eine gescannte Wurzel ohne Downloads-Ordner.
2. Hinweis in der Ausgabe lesen.

## Environment
- Branch: `dev` (1fab064)

## Root Cause Hypotheses
| # | Hypothesis | Probability | Evidence |
|---|-----------|-------------|----------|
| 1 | Eine Zeilenfortsetzung in einem String-Literal (`\` am Zeilenende) wurde durch ein langes Leerzeichenstück ersetzt, vermutlich beim Umbruch oder Formatieren. | High | `src/change/junk.rs:168`. Eine Suche nach Folgen von acht oder mehr Leerzeichen in String-Literalen unter `src/` findet nur diese Stelle (die übrigen Treffer in `report/terminal.rs` sind gewollte Ausrichtung). |

## Affected Code
| File | Line | Description |
|------|------|-------------|
| `src/change/junk.rs` | 168 | `format!`-Literal mit Leerzeichenfolge |

## Notes
Fix: Leerzeichenfolge durch ein Leerzeichen ersetzen (oder Zeilenfortsetzung mit `\`). Test: Der Hinweistext enthält kein Doppel-Leerzeichen (`!note.contains("  ")`); es gibt bereits Tests für `plan_junk` mit `notes` in `junk.rs` (prüfen, ob einer den Text vergleicht und angepasst werden muss).
