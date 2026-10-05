---
title: "Fix-Plan: Plan::validate linear machen"
status: fixed
fixed_at: 2026-10-05
created: 2026-10-05
updated: 2026-10-05
analysis: plan-validate-quadratic-analysis.md
---

# Fix-Plan: Verschobene Ordner einmal vorab erfassen

## Gewählter Fix
In `Plan::validate` (`src/change/plan.rs`, zweite Schleife, Z. 281-301) die Ordner-Aktionen einmal vor der Schleife in eine `HashMap<String, u32>` (Pfad-Schlüssel → Aktions-ID) sammeln. Je Aktion mit `target` den Ziel-Schlüssel und seine Vorfahren ablaufen und je Ebene nachschlagen. Kosten O(n · Pfadtiefe) statt O(n · Ordner-Aktionen). Gibt es keine Ordner-Aktion, entfällt die Prüfung.

Das Verhalten bleibt identisch zu `paths::is_under(target_key, dir)` (`src/paths.rs:57`): Gleichheit zählt als „darin“, Vorfahren-Beziehung nur an Pfadgrenzen (`\`). Eine Aktion zählt nicht gegen sich selbst (`o.id != a.id`).

Verworfen: nur `is_dir`-Aktionen vorfiltern und weiter linear suchen. Das bleibt quadratisch, wenn viele Ordner verschoben werden (z. B. Archiv-Pläne).

## Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/change/plan.rs` | vor Z. 272 | `let dir_moves: HashMap<String, u32>` aus allen `is_dir`-Aktionen (Schlüssel = `paths::path_key(Path::new(&o.path))`); `HashMap` zum `use` ergänzen |
| `src/change/plan.rs` | 290-295 | `nested` = „irgendein Schlüssel aus `ancestors_inclusive(target_key)` steht in `dir_moves` mit ID ≠ `a.id`“. Kleine private Hilfsfunktion `ancestors_inclusive(key: &str) -> impl Iterator<Item = &str>`, die `key`, dann jeweils bis vor das letzte `\` kürzt (Wurzel-Laufwerk `c:` mit eingeschlossen, wie `is_under` es für Präfixe `c:\` tut) |

Zu beachten: `is_under` entfernt am Präfix ein abschließendes `\` (`trim_end_matches`). Schlüssel für Ordner können mit `\` enden (Laufwerks-Wurzel); die Hilfsfunktion muss beide Schreibweisen abdecken. Im Test mit Wurzel-Ordner und Laufwerks-Wurzel prüfen.

## Tests
1. **Äquivalenz:** Eigenschaftstest oder Tabellentest, der für viele Paare (Ziel, Ordner) die neue Logik mit `paths::is_under` vergleicht, inklusive Gleichheit, Geschwisterpfad mit gleichem Präfix (`a\b` gegen `a\bc`), Wurzel `c:\` und Groß-/Kleinschreibung (Schlüssel sind schon kleingeschrieben).
2. Bestehende `validate`-Tests (Ziel in verschobenem Ordner, eigene ID, Gleichheit) bleiben unverändert grün.
3. **Leistung:** Plan mit 100.000 `move`-Aktionen mit Ziel und 1.000 Ordner-Aktionen; `validate` unter einer großzügigen Grenze (z. B. 2 s im Debug-Build) als Test in `tests/perf_plans.rs` (dort Aufbau prüfen, ggf. als `#[ignore]` wie die übrigen Leistungstests). Vorher messen: derselbe Test mit der alten Fassung zeigt das quadratische Verhalten.

## Risiken
Die Äquivalenz bei Randfällen der Pfadschlüssel (abschließendes `\`, Laufwerks-Wurzel). Der Äquivalenztest ist deshalb Pflicht und kommt vor dem Umbau.

## Rollback
`git revert`.
