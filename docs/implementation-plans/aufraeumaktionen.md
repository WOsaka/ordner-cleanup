---
title: "Implementation Plan: Aufräumaktionen (Phase 3)"
feature_spec: docs/features/aufraeumaktionen.md
status: approved   # pending-approval | approved | implemented
created: 2026-10-03
updated: 2026-10-03
---

# Implementation Plan: Aufräumaktionen (Phase 3)

## Summary
Phase 3 erweitert das Phase-2-Fundament (`src/change/`) um drei Dinge: ein **Plan-Format v2** mit den neuen Aktionstypen `remove-dir` und `move` neben `quarantine`, **vier neue Planer** (`junk`, `empty_dirs`, `archive`, `versions`), die wie `dedupe` ausschließlich den Index lesen, und eine **Erweiterung von Apply/Undo/Journal**, sodass jeder Aktionstyp write-ahead journaliert, stale-geprüft und zurückdrehbar ist. Für die Planer kommt eine gemeinsame Index-Abfrage hinzu (`dirs_under`, `files_under`) samt einem kleinen In-Memory-Baum, aus dem sich „leer“, „jüngste mtime im Teilbaum“ und „Inhalt vollständig bekannt“ ableiten lassen.

`FsOps` bekommt die fehlenden Operationen (`remove_dir` nur für leere Ordner, `create_dir`, `read_dir` nur mit Metadaten, `set_dir_meta`), damit alle neuen Pfade weiterhin mit `FaultyFs` testbar sind. `apply` prüft vor dem ersten Schritt die OneDrive-Obergrenze. `undo` lernt die neuen Journal-Einträge, räumt leere Ordner in Quarantäne und `_Archiv` auf und behandelt `purge` pro Aktion statt pro Lauf.

Die Umsetzung erfolgt inkrementell: erst das gemeinsame Fundament (Plan v2, Journal, FsOps, Index-Listing, Config, CLI-Gerüst), dann `junk` → `empty-dirs` → `archive` → `versions`. Jede Aktion ist nach ihrem Schritt Ende-zu-Ende lauffähig.

## Entscheidungen aus der Spec und dem Planungsinterview
| Thema | Entscheidung |
|---|---|
| Plan-Format | `version: 2` für neue Pläne; `Plan::from_json` akzeptiert 1 und 2 |
| Config | Flache Schlüssel wie bisher (siehe „Config“), eigene Müllmuster als `[[junk_rules]]` |
| Downloads-Ordner | `SHGetKnownFolderPath(FOLDERID_Downloads)`, überschreib-/erweiterbar per `downloads_dirs` |
| Archiv-Alter | Nur jüngste mtime im Teilbaum |
| Versionen | Mindestalter 30 Tage für ältere Versionen |
| OneDrive-Obergrenze | 1.000 Dateien oder 5 GB, gilt für **alle** Pläne (auch `dedupe`, `junk`); `remove-dir` zählt nicht |
| Stale bei `junk`/`versions`/`archive` | Größe + mtime (bei Ordnern: Dateianzahl, Summe, jüngste mtime), **kein Hash**. `dedupe` behält die Hash-Prüfung. |
| Ordner mit unvollständigem Index | Nie Kandidat für `empty-dirs`/`archive` (`incomplete-index`) |
| Undo nach `purge` | Nur Quarantäne-Aktionen gelten als „nicht mehr wiederherstellbar“; `move` nach `_Archiv` und `remove-dir` bleiben undo-fähig |
| Aufräumen nach Undo | Leere Elternordner unter `quarantine\<run-id>` und unter `_Archiv` werden entfernt |
| Code-Struktur | `src/app.rs` → `src/app/mod.rs` + `src/app/plan.rs` |

## Files to Create
| File | Purpose |
|------|---------|
| `src/change/tree.rs` | In-Memory-Baum aus `dirs_under`/`files_under`: Kinder je Ordner, `is_empty` (rekursiv), `newest_mtime`, `file_count`/`bytes`, `complete` (Index kennt alle Einträge, kein Summary-/Link-/Cloud-Inhalt) |
| `src/change/junk.rs` | Planer `plan_junk`: eingebaute Kategorien, `[[junk_rules]]`, Installer-Regel mit Downloads-Ordnern und Mindestalter |
| `src/change/empty_dirs.rs` | Planer `plan_empty_dirs`: rekursiv leere Ordner, bottom-up sortiert, Wurzel nie |
| `src/change/archive.rs` | Planer `plan_archive`: oberste alte Ordner, Zielbildung `_Archiv\<Jahr>\<rel>`; außerdem gemeinsame Helfer `archive_root`, `archive_target`, `is_in_archive` |
| `src/change/versions.rs` | Planer `plan_versions`: Gruppen über `analysis::similar::group_similar` je Ordner, `keep` = jüngste mtime, Mindestalter |
| `src/change/limits.rs` | OneDrive-Obergrenze: `move_volume(plan) -> (files, bytes)` und `check_limit(...)` |
| `src/app/plan.rs` | CLI-Verdrahtung aller `plan …`-Befehle (inkl. verschobenem `dedupe`), gemeinsame Zusammenfassung und Speichern |
| `tests/cli_cleanup_actions.rs` | Integrationstests Plan → Apply → Undo je Aktion über die echte CLI |

## Files to Modify
| File | Change Description |
|------|--------------------|
| `src/change/mod.rs` | Neue Module; `SkipReason` um `IncompleteIndex`, `TooRecent`, `TargetExists`, `NotEmpty`, `InArchive` erweitern (Display-Texte) |
| `src/change/plan.rs` | `PLAN_VERSION = 2`, Annahme von v1; `PlanKind` + `Junk`, `EmptyDirs`, `Archive`, `Versions`; `ActionType` + `RemoveDir`, `Move`; `PlannedAction` mit optionalen Feldern; `keep_strategy` optional; neue `params`; Validierung je Aktionstyp |
| `src/change/journal.rs` | `Intent` um `op` und `dest` erweitern (mit Serde-Defaults für Phase-2-Journale), neuer Eintrag `IntentRemoveDir` |
| `src/change/fsops.rs` | Trait um `remove_dir`, `create_dir`, `read_dir`, `set_dir_meta` erweitern; `RealFs` und `FaultyFs` nachziehen |
| `src/change/apply.rs` | `verify`/`process` nach Aktionstyp aufteilen (`verify_quarantine`, `verify_remove_dir`, `verify_move`); Hash nur, wenn der Plan einen hat; Obergrenze vor dem Lauf |
| `src/change/undo.rs` | `collect_moves` → `collect_ops` (Move/RemoveDir); Restore für Ordner-Moves und `remove-dir` (top-down anlegen, danach Zeiten bottom-up setzen); Purge-Logik pro Aktion; Aufräumen leerer Elternordner; `RunSummary` zählt Quarantäne-Bytes nur für `dest = quarantine` |
| `src/change/dedupe.rs` | Auf neues `PlannedAction` umstellen (`hash`/`keep`/`keep_hash` als `Some`), `version: 2` |
| `src/change/quarantine.rs` | `cleanup_empty_parents(fs, start, stop)` als gemeinsamer Helfer für Quarantäne und Archiv |
| `src/change/protect.rs` | Hilfsfunktion, um auch das **Ziel** eines `move` zu prüfen (gleiche Regeln) |
| `src/change/test_support.rs` | Fixture-Helfer für Ordner, alte mtimes (`File::set_modified`), Pläne der neuen Typen |
| `src/index/mod.rs` + neu `src/index/listing.rs` | `dirs_under(dir_key)` und `files_under(dir_key)` mit den für die Planer nötigen Spalten (eine Abfrage je Tabelle) |
| `src/platform/windows.rs` | `downloads_dir()` über `SHGetKnownFolderPath`; `set_dir_times_and_attrs` (Handle mit `FILE_FLAG_BACKUP_SEMANTICS`, `SetFileAttributesW`) |
| `src/config.rs` | Neue Felder (siehe „Config“), Validierung der Alters- und Größenangaben beim Laden |
| `src/cli.rs` | `PlanCommand` + `Junk`, `EmptyDirs`, `Archive`, `Versions` mit Args; `ApplyArgs.allow_large`; Hilfetexte von `apply` (nicht mehr nur Quarantäne) |
| `src/app.rs` → `src/app/mod.rs` | Per `git mv`; Plan-Befehle nach `app/plan.rs`; `apply_command`: Obergrenze, Ausgabe je Aktionstyp; `undo_command`: neue Statusarten |
| `Cargo.toml` | `windows-sys`-Features `Win32_UI_Shell`, `Win32_System_Com` (für `SHGetKnownFolderPath`/`CoTaskMemFree`) |
| `README.md` | Abschnitt zu den vier neuen `plan`-Befehlen, `--allow-large`, empfohlener Reihenfolge |
| `docs/roadmap.md` | Status Phase 3 |

## Data Model Changes
Der SQLite-Index bleibt unverändert (Schema-Version 1). Neu sind nur Felder in Plan, Journal und Config.

### Plan-Datei (JSON, `version: 2`)
```text
Plan { version: 2, created, kind: dedupe|junk|empty-dirs|archive|versions, root,
       keep_strategy: Option<String>,          // nur dedupe
       params: BTreeMap<String,String>,        // z. B. older_than=2y, min_age=30d, categories=temp,system
       actions: [PlannedAction], skipped: [Skipped] }

PlannedAction { id, type: quarantine|remove-dir|move, path, reason,
                size, mtime_ticks, mtime,         // Datei: Größe/mtime; Ordner: Summe/jüngste mtime
                hash: Option, keep: Option, keep_hash: Option,   // dedupe (hash), versions (keep)
                target: Option,                   // move: Ziel unter _Archiv
                is_dir: bool (default false),     // move eines Ordners
                files: Option<u64> }              // move eines Ordners: Dateianzahl
```
- Ein struct mit optionalen Feldern statt Enum: `dedupe`, `apply` und die Tests bleiben fast unverändert, `deny_unknown_fields` bleibt erhalten. `validate()` prüft je `type` die Pflichtfelder:
  - `quarantine`: `hash` und `keep` gemeinsam oder gar nicht; wenn vorhanden, wie in Phase 2 (`hash == keep_hash`, keep ≠ path, keep nicht selbst Aktion)
  - `remove-dir`: keine `hash`/`keep`/`target`; Pfad ≠ Wurzel
  - `move`: `target` Pflicht, liegt unter `<root>\_Archiv`, keine `.`/`..`-Komponenten; keine zwei Aktionen mit demselben Ziel; kein Ziel unter einem anderen Quell-Ordner; bei `is_dir` ist `files` Pflicht
- **v1-Pläne:** werden mit derselben Struktur gelesen (alle optionalen Felder fehlen bzw. sind gefüllt wie früher) und als `dedupe` validiert. Ein v1-Plan ohne `hash` ist ungültig.
- `skipped.reason` neu: `incomplete-index`, `too-recent`, `target-exists`, `not-empty`, `in-archive`.

### Journal (JSON Lines)
```text
{"t":"intent", …, "op":"move", "dest":"quarantine"|"archive", "is_dir":false}  // Felder mit serde(default): Phase-2-Journale = move/quarantine
{"t":"intent_remove_dir","run":…,"action":…,"path":…,"attrs":…,"mtime_ticks":…,"ctime_ticks":…}
```
- `dest` steuert in `undo`, unter welchem Präfix `to` liegen muss (Quarantäne-Lauf-Ordner bzw. `<root>\_Archiv`), und ob `purge` die Aktion unwiederbringlich macht.
- Attribute und Zeiten für `remove-dir` werden beim Apply **unmittelbar vor** dem Entfernen gelesen, nicht aus dem Plan (das mtime des Elternordners ändert sich, sobald Kinder entfernt werden).

### Config (`config.toml`, flach)
```toml
junk_categories = ["system", "temp", "downloads", "installer"]   # Default: alle
installer_min_age = "90d"
downloads_dirs = []              # leer = Known Folder; sonst ersetzt die Liste den Known Folder
[[junk_rules]]                   # eigene Kategorien
name = "logs"
patterns = ["*.log"]
min_age = "30d"                  # optional
dirs = []                        # optional: nur direkt in diesen Ordnern
archive_older_than = "2y"
versions_min_age = "30d"
onedrive_max_move_files = 1000
onedrive_max_move_bytes = "5GB"
```
- Alter über `analysis::age::parse_old_after`, Größe über `bytesize`; Fehler beim Laden der Config statt erst beim Planen.

## API / Interface Changes

### CLI
```text
ordner-cleanup plan junk <pfad> [--category system,temp,downloads,installer,<eigene>] [--out <plan.json>]
ordner-cleanup plan empty-dirs <pfad> [--out <plan.json>]
ordner-cleanup plan archive <pfad> [--older-than 2y] [--out <plan.json>]
ordner-cleanup plan versions <pfad> [--min-age 30d] [--out <plan.json>]
ordner-cleanup apply <plan.json> [--yes] [--allow-large]
```
- Exit-Codes wie Phase 2: `plan` 0; `apply` 0 / 2 (Teilerfolg) / 1 (Abbruch oder Fehler). Die verweigerte Obergrenze ist ein Fehler vor jeder Änderung → 1, mit Hinweis auf `--allow-large`.
- Zusammenfassung nach `plan`: Aktionen je Typ, betroffene Bytes bzw. Ordner, übersprungene Einträge nach Grund, OneDrive-Warnung, Hinweis bei überschrittener Obergrenze („`apply` braucht `--allow-large`“). Bei `empty-dirs` Hinweis, wenn Ordner nur Müll enthalten („erst `plan junk`“).

### Bibliothek (Auszug)
```rust
// index/listing.rs
pub struct DirRow  { path, key, parent_key, mode, attrs, is_link, direct_entries }
pub struct FileRow { path, key, dir_key, name, size, mtime, attrs, cloud_only, is_link }
impl Index { pub fn dirs_under(&self, dir_key) -> Result<Vec<DirRow>>;
             pub fn files_under(&self, dir_key) -> Result<Vec<FileRow>>; }

// change/tree.rs
pub struct Tree { … }  // aus DirRow/FileRow
impl Tree { fn children(&self, dir) ; fn files(&self, dir);
            fn is_empty(&self, dir) -> bool;          // rekursiv, nur bei complete
            fn complete(&self, dir) -> bool;          // direct_entries == bekannte Kinder, mode=full, kein Link/Cloud
            fn stats(&self, dir) -> SubtreeStats { files, bytes, newest_mtime, has_cloud, has_link } }

// Planer (alle: &Index, root, &Protector, created, Optionen) -> Result<CleanupPlan, IndexError>
pub fn plan_junk(…, &JunkOptions) ; pub fn plan_empty_dirs(…) ;
pub fn plan_archive(…, older_than_days, now_ticks) ; pub fn plan_versions(…, min_age_days, now_ticks)
pub struct CleanupPlan { pub plan: Plan, pub bytes: u64 }

// fsops.rs
fn remove_dir(&self, path) -> io::Result<()>;     // schlägt fehl, wenn nicht leer (atomar)
fn create_dir(&self, path) -> io::Result<()>;     // nicht rekursiv, AlreadyExists bei vorhandenem
fn read_dir(&self, path) -> io::Result<Vec<(PathBuf, FileMeta)>>;  // nur Metadaten, kein Öffnen
fn set_dir_meta(&self, path, attrs, mtime_ticks, ctime_ticks) -> io::Result<()>;

// limits.rs
pub fn move_volume(plan: &Plan) -> (u64 /*files*/, u64 /*bytes*/);
pub fn exceeds(plan, root, onedrive_roots, &Config) -> Option<String>;  // Meldung oder None
```

## Implementation Sequence
0. **Struktur vorbereiten** — `git mv src/app.rs src/app/mod.rs`, `plan_dedupe_command` nach `src/app/plan.rs`, ohne Verhaltensänderung; `cargo test` grün.
   - Files: `src/app/mod.rs`, `src/app/plan.rs`
   - Depends on: nothing
1. **Plan-Format v2** — Optionale Felder, neue `PlanKind`/`ActionType`, `params`, Validierung je Typ, v1-Annahme. `dedupe` und `test_support` umstellen. Tests: Roundtrip v2, v1-Fixture aus Phase 2 lädt, Pflichtfelder je Typ, Ziel außerhalb `_Archiv`, doppelte Ziele.
   - Files: `src/change/plan.rs`, `src/change/dedupe.rs`, `src/change/mod.rs`, `src/change/test_support.rs`
   - Depends on: 0
2. **Journal und FsOps erweitern** — `Intent.op/dest/is_dir` mit Defaults, `IntentRemoveDir`; neue FsOps-Methoden in `RealFs` und `FaultyFs`; `platform::windows::set_dir_times_and_attrs`. Tests: altes Journal-Fixture wird gelesen; `remove_dir` auf nicht leerem Ordner schlägt fehl; Zeiten/Attribute werden gesetzt.
   - Files: `src/change/journal.rs`, `src/change/fsops.rs`, `src/platform/windows.rs`
   - Depends on: 1
3. **Index-Listing und Baum** — `dirs_under`/`files_under`, `Tree` mit `is_empty`, `complete`, `stats`. Tests mit In-Memory-Index: Summary-Ordner, ausgeschlossene Kinder (`direct_entries` > bekannte), Links, Cloud-Dateien, verschachtelte Leere.
   - Files: `src/index/listing.rs`, `src/index/mod.rs`, `src/change/tree.rs`
   - Depends on: nothing (parallel zu 1–2)
4. **Config und CLI-Gerüst** — Neue Config-Felder mit Validierung; `PlanCommand`-Varianten und `--allow-large`; `limits.rs` mit Tests (Grenzen, nur unter OneDrive-Wurzel, `remove-dir` zählt nicht, Ordner-Moves zählen `files`/`size`); Prüfung in `apply_command` vor `confirm`.
   - Files: `src/config.rs`, `src/cli.rs`, `src/change/limits.rs`, `src/app/mod.rs`
   - Depends on: 1
5. **Apply nach Aktionstyp** — `verify` aufteilen. `quarantine` ohne Hash: nur Größe + mtime. `apply_plan` schreibt Journal-Einträge passend zum Typ. (Die Planer folgen in 6–9; hier mit handgebauten Plänen testen.) Tests: Junk-Quarantäne ohne Hash, stale durch mtime.
   - Files: `src/change/apply.rs`
   - Depends on: 2
6. **`junk` Ende-zu-Ende** — Kategorien, Glob-Matching auf den Namen (case-insensitiv, `globset`), `[[junk_rules]]`, Installer nur direkt in Downloads-Ordnern (`platform::windows::downloads_dir()` bzw. `downloads_dirs`) und älter als `installer_min_age`; Cloud-Platzhalter, Links, Geschütztes → `skipped`. `plan junk` in `app/plan.rs`. Unit-Tests je Kategorie, Integrationstest Plan → Apply → Undo.
   - Files: `src/change/junk.rs`, `src/app/plan.rs`, `src/platform/windows.rs`, `Cargo.toml`, `tests/cli_cleanup_actions.rs`
   - Depends on: 3, 4, 5
7. **`empty-dirs` + Undo für `remove-dir`** — Planer: alle Ordner mit `tree.is_empty`, ohne Wurzel und `_Archiv`-Wurzel selbst (Inhalt darunter erlaubt), ohne Geschütztes, Links, Cloud-Attribute am Ordner; Reihenfolge nach Tiefe absteigend. Apply: Ordner existiert, kein Link, dann Attribute/Zeiten lesen → `IntentRemoveDir` → `remove_dir`; `DirectoryNotEmpty` → `stale`. Undo: in umgekehrter Reihenfolge `create_dir` (vorhanden → `NothingToDo`, Datei an der Stelle → Konflikt), danach zweiter Durchlauf bottom-up `set_dir_meta`. Tests: verschachtelt, nur `Thumbs.db` → nicht leer, Wiederherstellung von Attributen (versteckt) und Zeiten, Absturz zwischen Intent und Remove.
   - Files: `src/change/empty_dirs.rs`, `src/change/apply.rs`, `src/change/undo.rs`, `src/app/plan.rs`, Tests
   - Depends on: 6
8. **`archive` + Undo für `move`** — Planer: Ordner mit `complete` und `stats.newest_mtime` älter als Schwelle, die mindestens eine Datei haben; nur oberste Treffer (Elternordner ist kein Treffer), Wurzel und `_Archiv` ausgenommen; Ziel `_Archiv\<Jahr>\<rel>`, Protector auf Quelle und Ziel; Ziel vorhanden → `target-exists`. Apply: `read_dir`-Walk ohne Öffnen ergibt Dateianzahl, Summe, jüngste mtime, Links/Cloud/Geschütztes → stale bzw. skip; Ziel-Elternordner anlegen, gleiches Volume, `rename` (überschreibt nie). Undo: Rückbenennung, danach `cleanup_empty_parents` bis einschließlich `_Archiv`. Undo-Aufräumen der Quarantäne-Elternordner ebenfalls hier (gemeinsamer Helfer). Purge-Logik pro Aktion (`dest`). Tests: oberster Ordner, Idempotenz nach Re-Scan, Kollision, gesperrte Datei → Fail ohne Teilverschiebung, Purge lässt Archiv-Undo zu.
   - Files: `src/change/archive.rs`, `src/change/apply.rs`, `src/change/undo.rs`, `src/change/quarantine.rs`, `src/change/protect.rs`, Tests
   - Depends on: 7
9. **`versions`** — Pro Ordner `group_similar` über die Namen der lokalen Dateien (Platzhalter zählen für die Gruppenbildung mit; ist die jüngste Datei ein Platzhalter, wird die Gruppe übersprungen). `keep` = jüngste mtime, bei Gleichstand lexikografisch letzter Name. Ältere Versionen jünger als `min_age` → `too-recent`. `move` nach `_Archiv\Versionen\<rel>`; Dateien unter `_Archiv` ausgenommen. Apply: Quelle Größe + mtime, `keep` existiert und ist nicht älter geworden. Tests: Spec-Beispiel, andere Endung/Ordner → keine Gruppe, gleicher mtime, Idempotenz.
   - Files: `src/change/versions.rs`, `src/change/apply.rs`, `src/app/plan.rs`, Tests
   - Depends on: 8
10. **Ausgabe, `runs`, Doku** — `runs` zeigt Aktionen je Typ und Quarantäne-Bytes nur für Quarantäne; `apply`/`undo`-Statuszeilen für Ordner; README, Roadmap; `cargo clippy -- -D warnings`, `cargo fmt`, Performance-Messung (100.000 Dateien, jeder Planer < 10 s).
    - Files: `src/app/mod.rs`, `src/change/undo.rs`, `README.md`, `docs/roadmap.md`
    - Depends on: 9
11. **Manueller Test** — OneDrive-Testordner (`ORDNER_CLEANUP_HOME` isoliert) und ein echter Alltagsordner (z. B. Downloads): je Aktion `scan` → `plan` → `apply` → `undo`, Vorher/Nachher-Vergleich (Pfad, Größe, mtime, Attribute, SHA256) wie in Phase 2; Obergrenze mit und ohne `--allow-large`. Ergebnis in der Spec unter „Manueller Test“ dokumentieren.
    - Depends on: 10

## Test Strategy
- **Unit:** Plan-Validierung je Typ und v1-Kompatibilität; Journal-Rückwärtskompatibilität (Fixture aus Phase 2); `Tree` (leer, vollständig, Statistik); jeder Planer mit In-Memory-Index (Kategorien, Mindestalter, oberster Ordner, Gruppenbildung, Schutzregeln, Platzhalter, `_Archiv`-Ausschluss, deterministische Sortierung); `limits`; Config-Parsing und -Fehler.
- **Apply/Undo mit `FaultyFs`:** Fehler bei `remove_dir`/`rename`/`create_dir`, Absturz nach `intent` (abgeschnittenes Journal), Konflikte beim Undo (Pfad belegt, Datei statt Ordner), Purge gemischter Läufe.
- **Integration (`tests/cli_cleanup_actions.rs`, echte Temp-Ordner):** pro Aktion `scan` → `plan` (Baum unverändert) → `apply --yes` → erwarteter Zustand → `undo --yes` → Ausgangszustand byteidentisch inkl. mtime und Attributen; Idempotenz (zweiter `scan` + `plan` leer); stale zwischen Plan und Apply; manipulierter Plan mit Ziel außerhalb `_Archiv` oder geschütztem Pfad; Obergrenze über eine OneDrive-Wurzel per Umgebungsvariable `OneDrive` auf den Temp-Ordner.
- **Bestehende Tests:** Alle Phase-1/2-Tests müssen unverändert grün bleiben (Rückwärtskompatibilität von Plan v1 und Journal).
- **E2E/Manuell:** Schritt 11; zusätzlich der aus Phase 2 offene Test mit echtem Cloud-only-Platzhalter, falls der OneDrive-Client dann läuft.
- Vorgehen nach `test-driven-development`; vor `status: implemented` `verification-before-completion`.

## Risks & Open Questions
- **Ordner-mtime bei `remove-dir`:** Das Entfernen von Kindern ändert das mtime der Eltern. Abhilfe: kein mtime-Stale für Ordner (die Leere prüft `remove_dir` atomar), Zeiten beim Apply direkt vor dem Entfernen lesen und beim Undo erst nach dem Anlegen aller Kinder bottom-up setzen.
- **Unvollständiger Index:** Ausgeschlossene oder nur aufsummierte Unterordner sowie Lesefehler würden einen Ordner fälschlich leer bzw. alt erscheinen lassen. Abhilfe: `complete` über `direct_entries` und `mode`; Apply prüft Ordner zusätzlich live (`remove_dir` bzw. `read_dir`-Walk).
- **Ordner-Moves unter OneDrive:** Ein Rename großer Ordner löst Sync aus; Obergrenze und Warnung greifen. Ein Ordner mit Cloud-Platzhaltern wird nie verschoben.
- **Archiv-Walk beim Apply:** Bei sehr großen alten Ordnern kostet der Metadaten-Walk Zeit (kein Lesen von Inhalten). Fortschrittsanzeige pro Aktion wie bisher; akzeptiert.
- **Versionsheuristik:** Falsche Gruppen sind möglich; nur Verschieben ins Archiv mit Undo, und der Plan zeigt `keep` je Aktion.
- **Known Folder API:** Neue `windows-sys`-Features; bei Fehler liefert `installer` nichts und `plan` nennt den Grund.
- **Plan-Struktur mit optionalen Feldern:** weniger typsicher als ein Enum; ausgeglichen durch strikte `validate()` je Typ und Tests für jede Pflichtfeld-Kombination.
- [ ] Keine offenen Fragen; die Entscheidungen aus dem Planungsinterview stehen oben in der Tabelle.
