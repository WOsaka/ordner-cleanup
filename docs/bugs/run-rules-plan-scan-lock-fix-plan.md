---
title: "Fix-Plan: run mit Regel-Plan scheitert an der eigenen Scan-Sperre"
status: fixed
fixed_at: 2026-10-05
created: 2026-10-05
updated: 2026-10-05
analysis: run-rules-plan-scan-lock-analysis.md
---

# Fix-Plan: Scan-Sperre im `run` doppelt genommen

## Ursache (Kurzfassung)
`run::execute` hält den `ScanLock` bis zum Ende (`src/app/run.rs:90`). Der Regel-Plan läuft in derselben Funktion, und `CachedLookup::content` nimmt die Sperre erneut (`src/ops/classify.rs:492`). `share_mode(0)` lässt auch im selben Prozess keine zweite Sperre zu → `Busy`.

## Gewählter Fix
Der Aufrufer sagt dem Plan-Pfad, dass er die Sperre schon hält. `CachedLookup` nimmt sie dann nicht.

Verworfen:
- **Sperre vor den Plänen freigeben:** öffnet ein Zeitfenster, in dem ein anderer Scan den Index verändert, während der Plan liest.
- **`no_classify: true` im `run`:** hält die Sperre nicht zusätzlich, aber ein Regel-Plan würde dann nie nachklassifizieren, außer das Profil hat `classify = true`. Das ändert das Verhalten des Plans, nicht nur den Fehler.

## Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/ops/plan.rs` | `PlanKindRequest::Rules` (Z. 53-62) | neues Feld `lock_held: bool` („Aufrufer hält die Scan-Sperre schon, Nachklassifizieren darf sie nicht nehmen“) |
| `src/ops/plan.rs` | `plan()` (Z. 207-220) und `plan_rules_kind` (Z. 474-529) | Feld durchreichen; `CachedLookup { lock_held, .. }` |
| `src/ops/classify.rs` | `CachedLookup` (Z. 443-454) | neues Feld `pub lock_held: bool` |
| `src/ops/classify.rs` | Z. 492 | `let _lock = if self.lock_held { None } else { Some(acquire_scan_lock().map_err(…)?) };` |
| `src/app/plan.rs` | `plan_rules_command` (Z. 129-141) | wird zu dünnem Wrapper um `plan_rules_with(args, lock_held: bool)`; der Wrapper übergibt `false` (CLI-Verhalten unverändert) |
| `src/app/run.rs` | `make_plan`, Zweig `"rules"` (Z. 250) | ruft `plan_rules_with(&args, true)` |
| Weitere Konstruktoren von `PlanKindRequest::Rules` | `src/gui/views/cleanup.rs` (~Z. 162), `src/gui/views/settings/rules.rs` (~Z. 253), Tests in `src/ops/plan.rs` | `lock_held: false` ergänzen (der Compiler findet alle) |

`classify_files` selbst nimmt keine Sperre (nur `classify()` in `ops/classify.rs:935` und `scan` in `ops/scan.rs:197`); kein weiterer Pfad ist betroffen. `PlanRulesArgs` (CLI) bekommt bewusst kein Feld, damit es kein Flag für Nutzer gibt.

## Tests
1. **Regression** (`src/ops/classify.rs`-Tests oder `src/app/run.rs`): Sperre in einem `ScanLock` halten, `CachedLookup` mit `lock_held: true` und fehlendem Cache-Eintrag aufrufen → kein `Busy`. Mit `lock_held: false` → `Busy` (belegt, dass der Test den Fehler wirklich trifft).
2. **Ende-zu-Ende**, falls der vorhandene Run-Test-Aufbau es erlaubt: Profil mit `plans = ["rules"]`, Regel mit `category`, leerer Content-Cache → Lauf endet `Ok`, Plan-Datei existiert.
3. Bestehende Tests für `plan rules` (CLI) laufen unverändert (`lock_held: false`).

Vor dem Fix den Regressionstest schreiben und rot sehen (`superpowers:test-driven-development`).

## Risiken
- Wer `lock_held: true` setzt, ohne die Sperre zu halten, umgeht den Schutz vor parallelen Scans. Deshalb nur `run.rs` setzt es; Doc-Kommentar am Feld.
- Kein Formatwechsel, keine Änderung der CLI-Oberfläche.

## Rollback
Reiner Code-Fix in einem Commit; `git revert`. Ohne den Fix endet `run` mit Regel-Plan wieder als `Partial`, sonst keine Folgen.
