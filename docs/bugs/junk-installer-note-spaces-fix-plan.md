---
title: "Fix-Plan: Leerzeichenfolge im Installer-Hinweis"
status: fixed
fixed_at: 2026-10-05
created: 2026-10-05
updated: 2026-10-05
analysis: junk-installer-note-spaces-analysis.md
---

# Fix-Plan: Ein Leerzeichen statt vierzehn

## Gewählter Fix
In `src/change/junk.rs:168` die Leerzeichenfolge zwischen „gefunden“ und „(Known Folder“ durch ein einzelnes Leerzeichen ersetzen. Sonst nichts ändern.

## Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/change/junk.rs` | 168 | `… gefunden (Known Folder oder downloads_dirs); es werden keine Installer geplant.` |

## Tests (vor dem Fix rot)
- Im Test-Modul von `junk.rs` einen Test, der `plan_junk` mit Kategorie `installer` und ohne Downloads-Ordner unter der Wurzel aufruft und prüft: der Hinweis enthält „gefunden (Known Folder“ und kein Doppel-Leerzeichen (`!note.contains("  ")`). Vorhandenen Test mit diesem Fall zuerst suchen (`grep "installer" src/change/junk.rs`), um ihn zu erweitern statt zu duplizieren.
- Es gibt nach der Analyse keine weitere Leerzeichenfolge in Strings unter `src/`.

## Risiken
Keine. Reiner Text. Tests, die den Text mit der alten Leerzeichenfolge vergleichen, müssten angepasst werden (keine gefunden: `grep "kein Downloads-Ordner"` trifft nur den Quelltext und einen Doc-Kommentar).

## Rollback
`git revert`.
