---
title: "Fix-Plan: apply ignoriert Profil-protected_paths"
status: fixed
fixed_at: 2026-10-05
created: 2026-10-05
updated: 2026-10-05
analysis: apply-profile-protected-paths-analysis.md
---

# Fix-Plan: `protected_paths` als Schnappschuss im Plan

Entscheidung des Users: Schnappschuss im Plan (nicht Profilname in `params`).

## Ursache (Kurzfassung)
`apply_execute` baut den `Protector` aus `load_config()` (`src/ops/apply.rs:72, 80`). Das Profil wird nur beim Planen eingemischt (`Config::with_profile`, `src/config.rs:470`). Der `Plan` (`src/change/plan.rs:54-67`) trägt davon nichts.

## Gewählter Fix
Beim Erzeugen speichert jeder Plan die zum Planzeitpunkt wirksamen `protected_paths` (globale + Profil). `apply` schützt mit der Vereinigung aus aktueller globaler Config und Schnappschuss.

Eigenschaften:
- **Nur Vereinigung:** Ein Plan kann Schutz hinzufügen, nie die globale Config schwächen. Ein manipulierter Plan, der den Schnappschuss entfernt, fällt auf den globalen Schutz zurück (wie heute).
- **Alte Pläne** ohne Feld bleiben lesbar und verhalten sich wie bisher.
- Kein Profil-Lookup beim Anwenden, also auch kein Fehler, wenn das Profil später umbenannt oder gelöscht wird.

## Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/change/plan.rs` | `struct Plan` (Z. 54-67) | neues Feld `#[serde(default, skip_serializing_if = "Vec::is_empty")] pub protected_paths: Vec<String>`. `PLAN_VERSION` bleibt 2 (optionales Feld). `subset` kopiert es über `..self.clone()` automatisch. |
| `src/change/plan.rs` | `validate` | keine Prüfung nötig (Pfade sind nur Schutzlisten); Länge nicht begrenzen |
| `src/ops/plan.rs` | `finish` (Z. 149-201) | vor dem Speichern `plan.protected_paths = config.protected_paths.clone()` (`plan` dafür `mut`). `finish` ist der gemeinsame Punkt aller Planer und bekommt die profil-gemischte Config. |
| `src/ops/plan.rs` | Konstruktion des Regel-Plans (~Z. 717) und weitere `Plan { … }`-Literale in `src/change/{archive,junk,empty_dirs,versions,dedupe,rules}.rs`, `src/gui/review.rs`, `src/app/mod.rs:487` und Testhilfen (`src/change/test_support.rs`, `src/change/plan.rs`/`limits.rs`/`mod.rs`-Tests) | `protected_paths: Vec::new()` ergänzen (der Compiler findet alle ~30 Stellen). Die Planer setzen ihn leer, `finish` füllt ihn. |
| `src/ops/apply.rs` | `apply_execute` (Z. 72-80) | `let mut config = load_config()?;` → vor dem Protector: Einträge aus `plan.protected_paths` anhängen, die noch nicht in `config.protected_paths` stehen. Dafür kleine Funktion `fn effective_config(config: Config, plan: &Plan) -> Config` in `apply.rs` (oder eine `Config`-Methode `with_extra_protected`). |
| `src/ops/apply.rs` | `check_with` (Z. 45-62) | **Nicht Teil dieses Fixes** (nutzt die globale Config für die OneDrive-Obergrenze). Beim Umsetzen prüfen, ob dort ebenfalls Profilwerte nötig sind; falls ja, getrennten Befund anlegen. |
| `docs/` | Plan-Format-Doku, falls vorhanden (`grep -r "params" docs/`) | Feld `protected_paths` ergänzen |

## Tests
1. **Regression** (`src/ops/apply.rs`-Tests): Plan mit Aktion unter `<root>\Keep` und `protected_paths = ["<root>\Keep"]` im Plan, globale Config leer → Aktion wird übersprungen/abgelehnt (gleiche Reaktion wie bei globalem Schutz); ohne Feld im Plan → wird ausgeführt (zeigt, dass der Test den Fehler trifft).
2. **Planen** (`src/ops/plan.rs`-Tests): Profil mit `protected_paths`, `finish` schreibt sie in den Plan; Plan-Datei per `Plan::load` zurücklesen → Feld vorhanden.
3. **Format:** alte Plan-JSON ohne Feld lädt (`from_json`), `to_json` für leeren Wert schreibt das Feld nicht; `subset` behält den Schnappschuss.
4. **Union-Eigenschaft:** Globale `protected_paths` bleiben auch bei leerem Schnappschuss wirksam.

Ablauf: Regressionstest zuerst, rot sehen, dann umsetzen (`superpowers:test-driven-development`).

## Risiken
- **Alte Binärdatei, neuer Plan:** `deny_unknown_fields` lehnt Pläne mit dem neuen Feld in einer älteren Version ab. Das Feld erscheint nur bei nicht-leerem Schutz; die Hinweise im Planformat entsprechend notieren. Praktisch relevant nur, wenn Plan und Programm von verschiedenen Versionen stammen.
- **Pfade im Plan** sind Rohstrings wie in der Config; `Protector::new` normalisiert und löst Links selbst auf. Das Verhalten „Regeln, die die Wurzel enthalten, werden ausgesetzt“ gilt damit auch für Schnappschuss-Einträge.
- Pläne vor dem Fix (ohne Feld) bleiben ohne Profilschutz beim Anwenden. Das lässt sich nicht nachträglich beheben, nur durch neu erzeugte Pläne. Im Changelog/Commit erwähnen.
- Viele Literale zu ändern, deshalb mechanisch und in einem Commit, getrennt vom Verhalten, wenn möglich (erst Feld und Literale, dann Logik).

## Rollback
`git revert` der Commits. Bereits geschriebene Pläne mit dem Feld lassen sich danach nur noch laden, wenn `deny_unknown_fields` das Feld kennt, daher entweder das Feld im Struct belassen und nur die Logik zurücknehmen, oder betroffene Pläne neu erzeugen.
