---
title: "Implementation Plan: Änderungsplan, Apply & Undo (Phase 2)"
feature_spec: docs/features/aenderungsplan-apply-undo.md
status: approved   # pending-approval | approved | implemented
created: 2026-10-03
updated: 2026-10-03
---

# Implementation Plan: Änderungsplan, Apply & Undo (Phase 2)

## Summary
Phase 2 ergänzt ein neues Modul `change/` mit fünf klar getrennten Teilen: **Schutzregeln** (`protect`), **Plan-Format und Planer** (`plan`, `dedupe`), **Journal** (`journal`), **Quarantäne und Ausführung** (`quarantine`, `apply`) sowie **Undo/Purge** (`undo`). Alle Dateisystemzugriffe laufen über ein kleines `FsOps`-Trait (`rename`, `metadata`, `exists`), damit Absturz-, Kollisions- und Sperrfälle mit einer Fake-Implementierung testbar sind, analog zum `DirSource`-Trait aus Phase 1.

Der Planer liest ausschließlich den Index (`exact_duplicate_groups`) und verändert nichts. `apply` verifiziert pro Aktion Größe, Änderungszeitpunkt und Hash (frisch neu berechnet, nur für lokale Dateien), schreibt den Journal-Eintrag mit `fsync` **vor** dem Move und verschiebt per `std::fs::rename` innerhalb desselben Volumes. Undo liest das Journal rückwärts und verschiebt zurück, ohne zu überschreiben.

Phase 1 bleibt unverändert lesend. Nötig ist nur, dass der Scan den Ordner `.ordner-cleanup` ausschließt (kleine Änderung in `scan/classify.rs`), damit Quarantäne und Journal nie im Index landen.

## Entscheidungen aus der Spec (Zusammenfassung)
- Plan als JSON, getrennte Befehle `plan dedupe` → `apply` → `undo`, dazu `runs` und `purge`
- Quarantäne pro Wurzel: `<wurzel>\.ordner-cleanup\quarantine\<run-id>\<relativer Pfad>`, 30 Tage
- Journal: JSON Lines, Write-ahead, `fsync` je Eintrag, unter `<wurzel>\.ordner-cleanup\journal\<run-id>.jsonl`
- Cloud-only-Platzhalter und Links werden nie angefasst
- Stale-Aktionen werden übersprungen, der Rest läuft; Teilerfolg über Exit-Code

## Vorschläge zu den offenen Fragen der Spec (bitte beim Freigeben bestätigen)
| Frage | Vorschlag |
|---|---|
| Ort der Config | Bestehende `config.toml` in `%APPDATA%\ordner-cleanup` um die Felder `protected_paths` (Liste) und `quarantine_days` (Default 30) erweitern. Kein neues Dateiformat. |
| Index aktuell? | Reicht die Stale-Prüfung pro Datei. Zusätzlich beim `plan` ein Hinweis, wenn die Wurzel nicht `Complete` ist oder der letzte Scan älter als 7 Tage ist. Kein Abbruch. |
| Hash für Stale-Prüfung | Derselbe xxh3-128 wie Phase 1 (`scan::hasher::full_hash`). Er schützt vor Änderungen, nicht vor Manipulation; ein Angreifer ist nicht Teil des Bedrohungsmodells. |
| Quarantäne unter OneDrive-Wurzeln | Der Move bleibt auf demselben Volume, aber `.ordner-cleanup` liegt dann im synchronisierten Bereich und erzeugt Sync-Traffic. `plan` und `apply` zeigen eine Warnung mit dem Hinweis, die Dateien stattdessen mit `purge` zu entsorgen. Ein Ausweichort ist eine spätere Option (Phase 3). |

## Files to Create
| File | Purpose |
|------|---------|
| `src/change/mod.rs` | Modulwurzel, gemeinsame Typen (`RunId`, `Action`, `SkipReason`), Re-Exports |
| `src/change/protect.rs` | `Protector`: feste und konfigurierbare geschützte Pfade, Projekt-Marker, Normalisierung (8.3, `\\?\`, Case) |
| `src/change/plan.rs` | Plan-Datenmodell (`Plan`, `PlannedAction`, `Skipped`), Serialisierung, Versionsprüfung, Validierung |
| `src/change/dedupe.rs` | Planer für Duplikate: Strategie `oldest`/`newest`/`path:<prefix>`, Auswahl der zu behaltenden Datei, Sicherheitsregeln |
| `src/change/fsops.rs` | `FsOps`-Trait + `RealFs` (rename, metadata, create_dir_all, fsync) |
| `src/change/journal.rs` | JSON-Lines-Schreiber (Write-ahead, `fsync`) und Leser (tolerant gegenüber abgeschnittener letzter Zeile) |
| `src/change/quarantine.rs` | Ziel-Pfadbildung (relativer Pfad bleibt erhalten, Längenprüfung, Kollisionsvermeidung), Quarantäne-Wurzel je Volume |
| `src/change/apply.rs` | Ausführung: Vorprüfung, Stale-Check, Schutzprüfung, Journal, Move, Ergebnis-Zusammenfassung |
| `src/change/undo.rs` | Rückgängigmachen anhand des Journals, `runs`-Auflistung, `purge` |
| `src/change/tests/` | Tabellen- und Fake-FS-Tests (siehe Teststrategie) |
| `tests/cli_apply_undo.rs` | Integrationstests über die echte CLI mit Temp-Ordnern |
| `tests/protected_paths.rs` | Schutzregeln gegen manipulierte Pläne |

## Files to Modify
| File | Change Description |
|------|--------------------|
| `src/lib.rs` | `pub mod change;` |
| `src/cli.rs` | Neue Befehle `Plan`, `Apply`, `Undo`, `Runs`, `Purge` mit Args-Structs; `about`-Text anpassen (nicht mehr „rein lesend“) |
| `src/app.rs` | Neue `*_command`-Funktionen, Bestätigungsdialog (j/N), Exit-Code-Mapping, Verdrahtung in `run` |
| `src/config.rs` | Felder `protected_paths: Vec<String>` und `quarantine_days: u32` (Default 30), Tests für Defaults und Validierung |
| `src/scan/classify.rs` | `.ordner-cleanup` als Ausschlussname in `EXCLUDED_NAMES`, Test |
| `src/paths.rs` | Helfer `relative_to(root, path)` und Prüfung `is_under(path_key, prefix_key)` (zentral, statt der lokalen `under` in `classify.rs`) |
| `src/platform/windows.rs` | `volume_serial(path)` für die Volume-Prüfung (Quelle und Quarantäne auf demselben Volume), `is_reparse_or_cloud(path)` über die Attribute |
| `src/index/dups.rs` | Erweiterung der `DupFile`-Abfrage um `size`, `volume_serial`, `file_index` und `hash` pro Datei, damit der Planer Hardlinks und Hashes ohne zweite Abfrage hat |
| `Cargo.toml` | Keine neuen Pflicht-Crates. Optional: `ulid` entfällt, die `run-id` wird als `YYYYMMDD-HHMMSS-<4 hex>` erzeugt. Beschreibung anpassen. |
| `README.md` | Abschnitt zu `plan`/`apply`/`undo`/`runs`/`purge` |
| `docs/roadmap.md` | Status Phase 2 aktualisieren |

## Data Model Changes
Der SQLite-Index bleibt unverändert (Schema-Version 1). Neu sind nur Dateien.

### Plan-Datei (JSON, `version: 1`)
```text
Plan { version, created, kind: "dedupe", root, keep_strategy,
       actions: [PlannedAction], skipped: [Skipped] }
PlannedAction { id, type: "quarantine", path, size, mtime_ticks, hash (hex, xxh3-128),
                keep (Pfad der behaltenen Datei), keep_hash, reason }
Skipped { path, reason: protected | cloud-placeholder | hardlink | link | group-incomplete | too-long | ... }
```
- `deny_unknown_fields`, `version` wird beim Laden geprüft; unbekannte Version → klarer Fehler.
- Pfade werden als Anzeige-Pfade (ohne `\\?\`) gespeichert, Zeiten als Ticks (wie im Index) plus lesbarer RFC-3339-Text.

### Journal (JSON Lines, eine Zeile je Ereignis)
```text
{"t":"run_start","run":"…","plan":"…","root":"…","started":"…"}
{"t":"intent","run":"…","action":1,"from":"…","to":"…","size":…,"hash":"…"}   // vor dem Move
{"t":"done","run":"…","action":1}                                              // nach dem Move
{"t":"skip","run":"…","action":2,"reason":"stale"|"protected"|…}
{"t":"fail","run":"…","action":3,"error":"…"}
{"t":"run_end","run":"…","status":"complete"|"partial"}
{"t":"undo_start"|"undo_done"|"undo_conflict"|"undo_end", …}
```
- Ein `intent` ohne `done` bedeutet: Der Move kann passiert sein oder nicht. Undo prüft den tatsächlichen Zustand (Quelle fehlt und Ziel existiert → zurück; sonst nichts tun).
- Das Lesen toleriert eine abgeschnittene letzte Zeile (Absturz beim Schreiben).

### Config (`config.toml`, zusätzliche Felder)
```toml
protected_paths = ["D:\\Wichtig"]   # ergänzt die Defaults, schaltet Systempfade nie ab
quarantine_days = 30
```

## API / Interface Changes
### CLI
| Befehl | Zweck | Exit-Codes |
|---|---|---|
| `plan dedupe <pfad> [--keep oldest\|newest\|path:<prefix>] [--out <plan.json>]` | Plan aus dem Index erzeugen, nichts verändern | 0 OK, 1 Fehler |
| `apply <plan.json> [--yes]` | Plan nach Bestätigung ausführen | 0 alles ausgeführt, 2 Teilerfolg (stale/Fehler/übersprungen), 1 Fehler/Abbruch |
| `undo <run-id> [--yes]` | Lauf zurückdrehen | 0 OK, 2 mit Kollisionen/fehlenden Dateien, 1 Fehler |
| `runs [<pfad>]` | Läufe auflisten (Zeit, Aktionen, Status, Größe, Ablauf) | 0 |
| `purge [--older-than 30d] [--yes]` | Abgelaufene Quarantäne-Läufe endgültig löschen | 0 |

Die Exit-Codes folgen Phase 1 (0 OK, 1 Abbruch/Fehler, 2 mit Teilfehlern).

### Interne Schnittstellen (Auswahl)
```rust
pub trait FsOps { fn rename(&self, from: &Path, to: &Path) -> io::Result<()>; fn metadata(&self, p: &Path) -> io::Result<FileMeta>; /* … */ }
pub fn plan_dedupe(index: &Index, root: &Path, strategy: KeepStrategy, protector: &Protector) -> Result<Plan>;
pub fn apply_plan(plan: &Plan, env: &ApplyEnv) -> Result<ApplyOutcome>;
pub fn undo_run(root: &Path, run: &RunId, env: &UndoEnv) -> Result<UndoOutcome>;
```
`ApplyEnv` bündelt `FsOps`, `Protector`, `Journal`-Fabrik, Abbruch-Flag (Strg+C) und Fortschritt, damit Tests keine globalen Zustände brauchen.

## Implementation Sequence
Jeder Schritt wird testgetrieben umgesetzt (Skill `test-driven-development`), `cargo test`, `cargo clippy -- -D warnings` und `cargo fmt` müssen nach jedem Schritt grün sein.

1. **Schutzregeln** — `Protector` mit festen Systempfaden (aus `SystemRoot`, `ProgramFiles`, `ProgramFiles(x86)`, `ProgramData`), konfigurierbaren Defaults (`AppData`, `.git`, `node_modules`, Marker-Dateien `Cargo.toml`, `package.json`, `*.sln`, `*.csproj`) und Nutzerliste. Normalisierung über `paths::path_key`, zusätzlich `canonicalize` für 8.3-Namen. Feste Pfade lassen sich per Config nicht entfernen.
   - Files: `src/change/protect.rs`, `src/paths.rs`, `src/config.rs`
   - Depends on: nichts
2. **Plan-Datenmodell** — Strukturen, Serde, Versionsprüfung, Validierung (doppelte IDs, `keep` fehlt, Aktion zeigt auf `keep`).
   - Files: `src/change/plan.rs`, `src/change/mod.rs`, `src/lib.rs`
   - Depends on: —
3. **Scan schließt `.ordner-cleanup` aus** — Ausschlussname plus Regressionstest, damit Quarantäne und Journal nie indiziert werden.
   - Files: `src/scan/classify.rs`
   - Depends on: —
4. **Index-Abfrage erweitern** — Duplikatgruppen liefern pro Datei Größe, Hash, Volume-Serial und File-Index; bestehende Report-Tests dürfen sich nicht ändern.
   - Files: `src/index/dups.rs`
   - Depends on: —
5. **Planer `dedupe`** — Auswahl der zu behaltenden Datei (`oldest` mit Tie-Break über kürzesten Pfad, dann alphabetisch; `newest`; `path:<prefix>`), Sicherheitsregeln aus der Spec: mindestens eine Kopie bleibt, geschützte Pfade und `keep` werden nie Kandidat, Platzhalter werden nie Kandidat, Hardlinks desselben Inhalts zählen nicht, Gruppe ohne zulässigen `keep` wird ausgelassen. Ausgabe nach Pfad sortiert (deterministisch).
   - Files: `src/change/dedupe.rs`
   - Depends on: Schritte 1, 2, 4
6. **FsOps, Journal, Quarantäne-Pfade** — `RealFs` und `FakeFs`; Journal-Schreiber mit `fsync` je Eintrag und tolerantem Leser; Zielpfadbildung (relativer Pfad bleibt erhalten, `\\?\`, Längenprüfung, nie überschreiben), Volume-Prüfung.
   - Files: `src/change/fsops.rs`, `src/change/journal.rs`, `src/change/quarantine.rs`, `src/platform/windows.rs`
   - Depends on: Schritt 2
7. **Apply** — Ablauf: Plan laden und validieren → Schutzregeln erneut prüfen → pro Aktion Stale-Check (Größe, mtime, Hash neu berechnen), `keep` existiert und stimmt → Journal `intent` → Move → `done`. Bereits erledigte Aktionen (Quelle weg, Ziel vorhanden und Hash gleich) zählen als „bereits erledigt“ (Idempotenz). Strg+C beendet sauber nach der laufenden Aktion. Zusammenfassung und Exit-Code.
   - Files: `src/change/apply.rs`
   - Depends on: Schritte 1, 5, 6
8. **Undo, Runs, Purge** — Undo liest das Journal, prüft den echten Zustand je Eintrag, verschiebt zurück ohne Überschreiben, meldet Kollisionen und fehlende Quarantäne-Dateien, ist selbst journaliert und idempotent. `runs` listet Läufe aus den Journalen. `purge` löscht nur abgelaufene, bestätigte Läufe (einziger Ort mit hartem Löschen).
   - Files: `src/change/undo.rs`
   - Depends on: Schritt 7
9. **CLI-Verdrahtung** — Befehle, Argumente, Bestätigungsdialog (j/N, `--yes`, in nicht interaktiven Sitzungen ohne `--yes` Abbruch mit Hinweis), Fortschrittsanzeige wie beim Scan, OneDrive-Warnung, Exit-Codes.
   - Files: `src/cli.rs`, `src/app.rs`
   - Depends on: Schritte 5, 7, 8
10. **Integrationstests** — Ende-zu-Ende über die echte CLI (siehe Teststrategie), inklusive Read-only-Nachweis für `plan` und manipulierte Pläne.
    - Files: `tests/cli_apply_undo.rs`, `tests/protected_paths.rs`
    - Depends on: Schritt 9
11. **Doku und manueller Test** — README, Roadmap, Spec auf `implemented` erst nach `verification-before-completion`. Manueller Lauf auf einem echten OneDrive-Testordner (nur mit `--yes`-freiem Dialog, danach `undo`).
    - Files: `README.md`, `docs/roadmap.md`, `docs/features/aenderungsplan-apply-undo.md`
    - Depends on: Schritt 10

Checkpoints für den Nutzer: nach Schritt 5 (Plan sichtbar, noch nichts schreibend), nach Schritt 8 (Kern fertig) und nach Schritt 11.

## Test Strategy
- **Unit (Tabellen-Tests, `rstest`)**
  - `protect`: Systempfade, Marker, 8.3-Kurznamen, `\\?\`-Präfix, Groß-/Kleinschreibung, Config kann Systempfade nicht abschalten
  - `plan`: Roundtrip, unbekannte Version, unbekannte Felder, doppelte IDs, Aktion zeigt auf `keep`
  - `dedupe`: Strategien, Tie-Break, Gruppe wird nie komplett entfernt, Platzhalter und Hardlinks, geschützter `keep`
  - `journal`: Reihenfolge der Einträge, abgeschnittene letzte Zeile wird toleriert
  - `quarantine`: Zielpfad, zu lange Pfade, Kollision erzeugt eindeutigen Namen und überschreibt nie
- **Fake-FS (Absturz und Fehler)**: Abbruch zwischen `intent` und Move, zwischen Move und `done`; gesperrte Datei; fehlende Rechte; Ziel existiert schon. Danach Undo-Zustand prüfen.
- **Integration (`assert_cmd`, `tempfile`, `ORDNER_CLEANUP_HOME`)**: scan → plan → apply → undo mit byteidentischer Wiederherstellung (Pfad, Inhalt, mtime); zweiter `apply` ändert nichts; Datei zwischen plan und apply geändert → `stale`, Exit 2; manipulierter Plan mit Pfad unter `C:\Windows`; `apply` ohne `--yes` und Eingabe „n“; `purge` löscht nur abgelaufene Läufe; `plan` verändert keinen Hash im Ordnerbaum (Read-only-Nachweis wie in `tests/readonly.rs`).
- **Windows-Besonderheiten**: Pfad > 260 Zeichen, Umlaute, Leerzeichen, Hardlinks (`mklink /H` über `std::fs::hard_link`), Junction als Quelle. Cloud-only-Platzhalter werden über die Fake-Quelle simuliert; ein echter Test erfolgt manuell auf OneDrive.
- **E2E (manuell)**: echter OneDrive-Testordner: Plan prüfen, Apply mit Bestätigung, Undo, `runs`, `purge`. Ergebnis wird in der Spec dokumentiert.

## Risks & Open Questions
- **Datenverlust durch falsche `keep`-Wahl**: Mitigation: mindestens eine Kopie bleibt immer, `keep` wird beim Apply neu gehasht und muss existieren, Quarantäne statt Löschen, Undo getestet.
- **Absturz zwischen Journal und Move**: Mitigation: Write-ahead mit `fsync`; Undo prüft den realen Dateizustand statt nur dem Journal zu vertrauen.
- **Manipulierte oder veraltete Pläne**: Mitigation: Schutzregeln und Stale-Check beim Apply unabhängig vom Plan; strenge Versionsprüfung und `deny_unknown_fields`.
- **Pfadnormalisierung (8.3, Junctions, Case)** kann Schutzregeln umgehen: Mitigation: einheitlich über `paths::path_key` plus `canonicalize`; dedizierte Regressionstests, wie schon bei `index remove`.
- **OneDrive-Sync-Traffic durch Quarantäne im synchronisierten Bereich**: Mitigation: Warnung; Ausweichort ist bewusst nicht Teil von Phase 2.
- **Hash-Neuberechnung beim Apply** kostet Zeit bei großen Dateien: akzeptiert, Sicherheit vor Geschwindigkeit; Fortschrittsanzeige.
- **Zeitstempel-Genauigkeit**: Vergleich in Ticks (100 ns) wie im Index; Undo stellt Dateien durch `rename` mit unveränderten Zeitstempeln wieder her, daher kein Zeitstempel-Restore nötig.
- [ ] Bestätigung der vier Vorschläge in der Tabelle „Vorschläge zu den offenen Fragen“
- [ ] Soll `plan` ohne `--out` die Datei im aktuellen Verzeichnis (`plan-<timestamp>.json`) oder unter `%LOCALAPPDATA%\ordner-cleanup\plans\` ablegen? Vorschlag: aktuelles Verzeichnis, analog zum Bericht.
- [ ] Soll `apply` in nicht interaktiven Sitzungen ohne `--yes` hart abbrechen? Vorschlag: ja.
