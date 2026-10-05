---
title: "Bug: Bestätigungsdialog nennt feste 30 Tage Quarantäne"
status: fixed
created: 2026-10-05
updated: 2026-10-05
---

# Bug: Dialog nennt immer „30 Tage“ statt `quarantine_days`

## Observed Behavior
Der Bestätigungsdialog beim Anwenden eines Plans (Quarantäne-Pläne: junk, dedupe, …) sagt „die Quarantäne (.ordner-cleanup in der Wurzel; 30 Tage, per „Rückgängig“ holbar)“. Mit `quarantine_days = 7` oder `90` stimmt das nicht; `runs` und `purge` verwenden den konfigurierten Wert (`ops/runs.rs:27, 57, 105`).

## Expected Behavior
Der Dialog nennt den konfigurierten Wert.

## Reproduction Steps
1. `config.toml`: `quarantine_days = 7`.
2. GUI: Plan (z. B. `junk`) erzeugen, „Anwenden“.
3. Dialogtext enthält „30 Tage“.

## Environment
- Branch: `dev` (1fab064), GUI

## Root Cause Hypotheses
| # | Hypothesis | Probability | Evidence |
|---|-----------|-------------|----------|
| 1 | Der Text ist im Code als Literal fest: `target_text` hat keinen Zugriff auf die Config. | High | `src/gui/views/apply.rs:60-66`. `confirm_text` (`:70`) erhält `ApplyCheck`, Index-Hinweis und Teilmenge, aber keine Quarantäne-Frist. Aufrufe: `apply.rs:164, 305, 329`. |
| 2 | `ApplyCheck` trägt die Frist nicht, obwohl `apply_check`/`check_with` die Config schon laden. | High | `src/ops/apply.rs:21-30` (Struct), `:41-62` (`check_with(plan, &config, …)` kennt `config.quarantine_days`). |

Ob die Profil-Config die Frist überschreiben kann: Nein (`quarantine_days` steht nicht in der Liste der Profil-Felder, `config.rs:482-491`). Die globale Config genügt also.

## Affected Code
| File | Line | Description |
|------|------|-------------|
| `src/gui/views/apply.rs` | 60-66 | festes „30 Tage“ |
| `src/gui/views/apply.rs` | 70-90, 164, 305, 329 | `confirm_text` ohne Frist |
| `src/ops/apply.rs` | 21-30, 45-62 | `ApplyCheck` ohne Frist |

## Notes
Fix: `ApplyCheck` bekommt `quarantine_days: u32` (aus `config.quarantine_days` in `check_with`); `target_text`/`confirm_text` formatieren den Wert („{n} Tag(e)“, Singular beachten). Tests: Text enthält die konfigurierte Zahl; vorhandene Tests in `src/gui/views/apply.rs`, die „30 Tage“ erwarten, anpassen.

Nachgeprüft und **kein** Fehler: `check_with` nutzt für die OneDrive-Obergrenze die globale Config. Die Werte `onedrive_max_move_files/_bytes` kann ein Profil nicht überschreiben (nicht in `with_profile`, `src/config.rs:459-502`). Das Thema aus der Analyse zu `apply-profile-protected-paths` bleibt damit ohne Handlungsbedarf.
