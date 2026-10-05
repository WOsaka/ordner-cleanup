---
title: "Bug: Geplanter run mit Regel-Plan scheitert an der eigenen Scan-Sperre"
status: fixed
created: 2026-10-05
updated: 2026-10-05
---

# Bug: Geplanter `run` mit Regel-Plan scheitert an der eigenen Scan-Sperre

## Observed Behavior
`run --profile X` mit `plans = ["rules"]` endet als `Partial` (Exit `EXIT_PARTIAL`), sobald das Nachklassifizieren für den Regel-Plan Dateien nachholen muss. In `RunRecord.errors` steht `Plan rules: Ein anderer Scan läuft bereits (Sperre …)`. Der Regel-Plan wird nicht geschrieben.

## Expected Behavior
Der Lauf hält die Sperre schon und darf sie für das Nachklassifizieren nicht noch einmal nehmen. Der Regel-Plan entsteht, der Lauf endet `Ok`.

## Reproduction Steps
1. Profil mit `plans = ["rules"]` und einer Regel, die `category` oder `doc.*` nutzt (oder bei der Dateien im Cache unklassifiziert sind).
2. `ordner-cleanup run --profile X` (so auch die geplante Aufgabe).
3. Ergebnis: `Partial`, Fehler „Plan rules: Ein anderer Scan läuft bereits“.

Bedingung: `CachedLookup::content` findet fehlende/veraltete Einträge und `classify == true`. Ist der Cache komplett gültig, wird die Sperre nicht angefasst und der Fehler tritt nicht auf. Deshalb fällt er nur bei neuen/geänderten Dateien auf.

## Environment
- Branch: `dev` (553ecdf)
- Windows 10/11, geplanter Lauf (`run`)
- Relevant config: Profil mit `plans = ["rules"]`, Regeln mit Inhaltsbedingungen

## Root Cause Hypotheses
| # | Hypothesis | Probability | Evidence |
|---|-----------|-------------|----------|
| 1 | `execute` hält `ScanLock` bis zum Funktionsende. `make_plan` → `plan_rules_command` → `CachedLookup::content` ruft `acquire_scan_lock()` auf dieselbe Datei auf. `ScanLock` öffnet mit `share_mode(0)`, das scheitert auch im selben Prozess (`ERROR_SHARING_VIOLATION` → `Busy`). | High | `src/app/run.rs:90` hält `_lock`; `drop(res)` (Z. 181) gibt nur den Index frei, nicht die Sperre. `src/ops/classify.rs:492` nimmt sie erneut. `src/scan/lock.rs:39-60`. Der Unit-Test `zweite_sperre_scheitert_bis_die_erste_frei_ist` belegt das Verhalten. `make_plan` setzt `no_classify: false` (`run.rs:256`). |
| 2 | Der Fehler wird nicht früher bemerkt, weil kein Test `run` mit Regel-Plan und leerem Content-Cache abdeckt. | Med | Keine Treffer für diesen Pfad in den Tests (nicht vollständig geprüft). |

Die Sperre wird nur an drei Stellen genommen: `ops/scan.rs:197`, `ops/classify.rs:935` (`classify`), `ops/classify.rs:492` (`CachedLookup`). Nur die dritte läuft in einem Kontext, in dem der Aufrufer sie schon halten kann.

## Affected Code
| File | Line | Description |
|------|------|-------------|
| `src/ops/classify.rs` | 492 | `acquire_scan_lock()` in `CachedLookup::content` |
| `src/app/run.rs` | 90, 181, 185, 256 | `execute` hält die Sperre; Pläne laufen in derselben Funktion mit `no_classify: false` |
| `src/ops/plan.rs` | 474-529 | `plan_rules_kind` reicht `no_classify` an `CachedLookup.classify` |
| `src/scan/lock.rs` | 39-60 | `share_mode(0)` → keine zweite Sperre, auch nicht im selben Prozess |

## Notes
Lösungsrichtungen für `/debug:fix-plan`:
- (a) `CachedLookup` bekommt ein Feld, ob der Aufrufer die Sperre schon hält (`PlanRequest`/`PlanRulesArgs` tragen es durch), und nimmt sie dann nicht.
- (b) `run` gibt die Sperre vor den Plänen frei. Das öffnet ein Zeitfenster für einen parallelen Scan, deshalb weniger gut.
- (c) Sperre als Objekt durchreichen (Token „Sperre gehalten“).

Für Befund 1 reicht (a). Test: `execute` mit Regel-Plan und leerem Content-Cache.
