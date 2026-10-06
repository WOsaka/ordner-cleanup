---
title: "Bug: parse_old_after läuft bei großen Zahlen über"
status: fixed
created: 2026-10-05
updated: 2026-10-05
---

# Bug: Überlauf in `parse_old_after`

## Observed Behavior
`parse_old_after("99999999999999999y")` rechnet `number * factor` mit `i64` ohne Prüfung (`src/analysis/age.rs:65`). Im Debug-Build ist das ein Panic. Im Release-Build wird mit Wraparound gerechnet (kein `overflow-checks` in `Cargo.toml`), das Ergebnis kann negativ werden. Dann gilt `is_old(…, threshold_days)` für jede Datei als wahr, `plan archive`/`versions`/`junk` schlagen alles vor.

## Expected Behavior
Zu große Werte ergeben `Err("Ungültige Dauer …")`, wie andere ungültige Eingaben.

## Reproduction Steps
1. `config.toml`: `archive_older_than = "99999999999999999y"` (oder CLI `--older-than 99999999999999999y`, `--old-after`, `--min-age`).
2. Debug-Build: Panic beim Laden der Config (`validate_cleanup`, `src/config.rs:511`). Release: Config wird akzeptiert, `plan archive` schlägt alle Ordner vor.
3. Unit-Test: `parse_old_after("99999999999999999y")`.

Größenordnung: Überlauf ab `number > i64::MAX / 365 ≈ 2,5e16` (Jahre), bei `m` ab `≈ 3,1e17`, bei `d` nie.

## Environment
- Branch: `dev` (1fab064), Release-Profil ohne `overflow-checks`
- Eingaben: Config, Profil, CLI

## Root Cause Hypotheses
| # | Hypothesis | Probability | Evidence |
|---|-----------|-------------|----------|
| 1 | Ungeprüfte Multiplikation. Die Eingabe ist nur auf `> 0` geprüft, nicht nach oben begrenzt. | High | `src/analysis/age.rs:61-65`. Aufrufer: `config.rs:432, 511, 545, 560-568`, `ops/plan.rs:315, 347`, `ops/report.rs:37`, `health/collect.rs:55`, `ops/runs.rs:103`, `rules/file.rs:147`, `settings/fields.rs:548`, `change/junk.rs:104`. |
| 2 | Die Tests prüfen nur kleine und ungültige Werte. | Med | `age.rs:120, 133`. |

Hinweis `ops/runs.rs:103`: `u32::try_from(parse_old_after(…)?)` fängt zu große Ergebnisse dort bereits als Fehler, aber erst nach dem Überlauf.

## Affected Code
| File | Line | Description |
|------|------|-------------|
| `src/analysis/age.rs` | 61-65 | `number * factor` |
| `src/config.rs` | 511, 545 | Validierung beim Laden verlässt sich auf `parse_old_after` |

## Notes
Fix: `number.checked_mul(factor).ok_or_else(err)`. Sinnvoll zusätzlich ein Tageslimit? Nicht nötig: `checked_mul` reicht, und ein später `age_days`-Vergleich mit großen Werten ist unproblematisch. Tests: Grenzwerte (`i64::MAX/365` passt, `+1` Fehler), `d`, `m`, `y`.
