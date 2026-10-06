---
title: "Fix-Plan: Quarantäne-Frist im Bestätigungsdialog"
status: fixed
fixed_at: 2026-10-05
created: 2026-10-05
updated: 2026-10-05
analysis: apply-dialog-quarantine-days-analysis.md
---

# Fix-Plan: `quarantine_days` im `ApplyCheck` mitgeben

## Gewählter Fix
`ApplyCheck` bekommt ein Feld `quarantine_days: u32`, das `check_with` aus `config.quarantine_days` füllt. `confirm_text` und `target_text` in `src/gui/views/apply.rs` formatieren daraus den Dialogtext. Profile können `quarantine_days` nicht überschreiben (`Config::with_profile`), die globale Config genügt.

Verworfen: Config im GUI-Code ein zweites Mal laden. Der Check trägt die Daten für den Dialog schon (Obergrenze, Warnungen).

## Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/ops/apply.rs` | `ApplyCheck` (Z. 21-30) | Feld `pub quarantine_days: u32` |
| `src/ops/apply.rs` | `check_with` (Z. 54-61) | `quarantine_days: config.quarantine_days` |
| `src/gui/views/apply.rs` | `target_text` (Z. 60-66) | zusätzlicher Parameter `quarantine_days: u32`; Text `„… .ordner-cleanup in der Wurzel; {n} Tag(e), per „Rückgängig“ holbar“` (Einzahl bei 1: „1 Tag“) |
| `src/gui/views/apply.rs` | `confirm_text` (Z. 80) | übergibt `check.quarantine_days` |
| `src/gui/views/apply.rs` | Test-Helfer `check()` (Z. 285-300) | neues Feld setzen (30) |

`ApplyCheck` wird sonst nirgends konstruiert (Suche: `ApplyCheck {` nur in `ops/apply.rs` und dem Test-Helfer).

## Tests (vor dem Fix rot)
1. `src/gui/views/apply.rs`: `confirm_text` mit `quarantine_days = 7` enthält „7 Tage“ und nicht „30 Tage“; mit `1` „1 Tag“. Vorhandene Tests, die den alten Text mit „30 Tage“ prüfen, anpassen (Suche: `grep "30 Tage" src tests` fand nur das Literal selbst).
2. `src/ops/apply.rs`: `check_with` mit `Config { quarantine_days: 7, .. }` → `check.quarantine_days == 7`.
3. Für Pläne der Arten Archive, Versions, Rules, EmptyDirs bleibt der Text ohne Frist unverändert.

## Risiken
Gering: ein zusätzliches Feld im internen Check-Ergebnis, keine Planformat-Änderung.

## Rollback
`git revert`.
