---
title: "Bug: apply ignoriert Profil-protected_paths"
status: fixed
created: 2026-10-05
updated: 2026-10-05
---

# Bug: `apply` ignoriert Profil-`protected_paths`

## Observed Behavior
`protected_paths` aus einem Profil schützen beim Planen (`plan --profile X`), aber nicht beim Anwenden. `apply` baut seinen `Protector` nur aus der globalen `config.toml`.

## Expected Behavior
`apply` ist die letzte Schutzschicht und lehnt Aktionen unter allen Pfaden ab, die für den Plan gelten, also auch Profil-Pfade.

## Reproduction Steps
1. Profil mit `protected_paths = ['D:\Daten\Keep']`, kein solcher Eintrag in der globalen Config.
2. `plan … --profile X`: Aktionen unter `Keep` fehlen (Protector aus Profil-Config).
3. Plan von Hand ergänzen, veralten lassen oder als Teilplan verändern, sodass er eine Aktion unter `D:\Daten\Keep` enthält.
4. `ordner-cleanup apply plan.json`: Aktion wird ausgeführt.

Ein frisch erzeugter, unveränderter Plan ist nicht betroffen, weil die Pfade schon beim Planen herausfallen. Relevant wird es bei von Hand editierten Plänen, Plänen aus einem früheren Stand oder nachträglich erweiterten Profil-Pfaden (Plan älter als die Schutzänderung).

## Environment
- Branch: `dev` (553ecdf)
- Relevant config: `[profiles.<name>] protected_paths`

## Root Cause Hypotheses
| # | Hypothesis | Probability | Evidence |
|---|-----------|-------------|----------|
| 1 | `apply_execute` kennt kein Profil. Es lädt `load_config()` (nur globale Datei) und baut daraus den `Protector`. Das Profil wird nur beim Planen über `TargetSpec.resolve()` → `Config::with_profile` eingemischt (`protected_paths` wird dort ergänzt). | High | `src/ops/apply.rs:72, 80`. `src/ops/mod.rs:263-268` (`load_config`). `src/config.rs:459-470`. `src/ops/plan.rs:122-139` (Planer: Config aus `resolve()`). |
| 2 | Der Plan speichert kein Profil: `Plan` hat nur `version, created, kind, root, keep_strategy, params, actions, skipped`. `apply` kann das Profil also nicht aus dem Plan ableiten, und die CLI (`ApplyArgs`) bietet kein `--profile`. | High | `src/change/plan.rs:54-67`. `src/app/mod.rs:155-198`, `src/gui/views/apply.rs:211` (beide rufen `apply_execute` ohne Profil). |

Auch `check_with` (Obergrenzen/`limits::exceeds`) nutzt nur die globale Config. Das ist hier nicht der gemeldete Fehler, aber dieselbe Ursache; beim Fix mitprüfen.

## Affected Code
| File | Line | Description |
|------|------|-------------|
| `src/ops/apply.rs` | 72-80 | globale Config → `Protector` |
| `src/ops/apply.rs` | 41-43, 45-62 | `apply_check` ebenfalls nur global |
| `src/change/plan.rs` | 54-67 | `Plan` ohne Profilbezug |
| `src/app/mod.rs` | 155-198 | `apply_command` ohne Profil |
| `src/gui/views/apply.rs` | 211 | GUI-Aufruf ohne Profil |
| `src/config.rs` | 459-470 | Profil ergänzt `protected_paths` |

## Notes
Lösungsrichtungen für `/debug:fix-plan` (Entscheidung nötig, da sich das Planformat oder die CLI ändert):
- (a) Plan merkt sich das Profil in `params` (z. B. `profile=X`); `apply_execute` löst es auf. Plan-Format bleibt kompatibel (`params` ist offen). Bei fehlendem Profil: Fehler statt stiller Fallback.
- (b) Plan speichert die wirksamen `protected_paths` als Schnappschuss. Unabhängig von späteren Config-Änderungen, aber Format-Erweiterung.
- (c) `apply --profile X` und GUI-Auswahl. Nutzer kann es vergessen, schützt also nur halb.

Empfehlung zur Diskussion: (a) oder (b); (c) allein löst die Aufgabe „letzte Schutzschicht“ nicht.
