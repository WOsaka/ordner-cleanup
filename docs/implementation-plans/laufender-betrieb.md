---
title: "Implementation Plan: Laufender Betrieb (Phase 5)"
feature_spec: docs/features/laufender-betrieb.md
status: approved # pending-approval | approved | implemented
created: 2026-10-04
updated: 2026-10-04
---

# Implementation Plan: Laufender Betrieb (Phase 5)

## Summary

Phase 5 bringt fünf Bausteine, die aufeinander aufbauen:

1. **Kennzahlen und Health-Score** (`src/health/`): rein und deterministisch, berechnet aus denselben Index-Zeilen wie `report::build`.
2. **Verlauf** in einer eigenen `history.db` (`src/history/`): Ein vollständiger `scan` schreibt am Ende eine Momentaufnahme hinein. `report` und der neue Befehl `history` lesen daraus.
3. **Profile** in der `config.toml`, mit `--profile` für alle Befehle.
4. **Befehl `run`**: Er macht Scan, Momentaufnahme, Bericht und Pläne, nie `apply`. Er läuft über ein zweites Programm ohne Konsolenfenster (`ordner-cleanup-bg.exe`), das die Aufgabenplanung startet. Die Aufgaben verwaltet `schedule add|list|remove` über `schtasks.exe /XML`. Bei relevanten Veränderungen erscheint eine Windows-Benachrichtigung.
5. **Vorlagen** (`src/template/`): eingebaute TOML-Vorlagen und eigene im selben Format, mit Soll/Ist-Abgleich im Bericht und Teilwert „Strukturtreue“. Als Phase 5b abtrennbar.

Gegenüber der Spec ändert sich die Reihenfolge der Umsetzung: Profile kommen **vor** `run`/`schedule`, weil `run --profile` sie voraussetzt.

## Entscheidungen aus Spec und Codeanalyse

| Thema                        | Entscheidung                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| ---------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Quelle der Kennzahlen        | `report/build.rs` lädt Dateien und Ordner in privaten Structs (`FRow`, `DRow`). Diese ziehen samt Ladefunktionen nach `report/rows.rs` (`pub(crate)`), damit Bericht und Kennzahlen dieselben Zeilen und dieselben Analysefunktionen nutzen (`problems::check_file`, `structure::check_dir`, `similar::group_similar`, `exact_duplicate_groups`).                                                                                                                                                        |
| Müll-Erkennung               | `plan_junk` enthält die Kategorielogik privat. Sie wird als `JunkClassifier::new(options, root).classify(&FileRow) -> Option<&str>` herausgelöst; `plan_junk` nutzt ihn unverändert.                                                                                                                                                                                                                                                                                                                     |
| Ordner der ersten Ebene      | Gemessen werden je Wurzel und je direktem Unterordner. Dateien direkt in der Wurzel zählen nur zur Wurzel. Bei mehr als 200 Ordnern gibt es Zeilen für die 200 größten und eine für „Sonstige“. Duplikate gehen in den Ordner jeder **weiteren** Instanz (Gruppe nach Pfad sortiert, die erste zählt nicht als verschwendet).                                                                                                                                                                            |
| Health-Score                 | Teilwerte linear: 100 bei 0, 0 ab einer Schwelle für „schlecht“, dazwischen begrenzt (Tabelle unten). Gesamtwert = gerundeter gewichteter Durchschnitt. Abzug je Teilwert = `gewicht/Σgewichte · (100 − teilwert)`. Die Schwellen sind Konstanten mit Versionsnummer (`METRICS_VERSION`) und stehen im README. Einstellbar sind nur die Gewichte, wie in der Spec.                                                                                                                                       |
| Vergleichbarkeit             | Eine Momentaufnahme speichert einen Config-Fingerabdruck (xxh3 über eine kanonische JSON-Form der relevanten Einstellungen: Excludes, Summary, Müll-Kategorien und -Regeln, `old_after`, Struktur-Schwellen, Gewichte, Vorlage samt Inhalts-Hash) sowie `METRICS_VERSION`. „Vergleichbar“ heißt: gleiche Wurzel, vollständig, gleicher Fingerabdruck, gleiche Version. Gibt es keinen solchen Lauf, wird mit dem letzten vollständigen Lauf verglichen und die Ausgabe als „eingeschränkt“ markiert.     |
| Zeitpunkt der Momentaufnahme | In `scan_command` nach einem Scan mit Status `complete` (Fehler bzw. Warnungen erlaubt, ihre Anzahl wird gespeichert). Scheitert das Schreiben, gibt es eine Warnung; der Exit-Code des Scans bleibt.                                                                                                                                                                                                                                                                                                    |
| Exklusiver Scan              | Neue Sperrdatei `<data>\scan.lock`, exklusiv geöffnet (`share_mode(0)`) für die Dauer von `scan`/`run`. Ist sie belegt, endet ein manueller `scan` mit einer klaren Meldung, `run` überspringt und protokolliert.                                                                                                                                                                                                                                                                                        |
| Profile                      | `[profiles.<name>]` in `config.toml`. Listen werden **ergänzt**, Einzelwerte **ersetzt**, wie bei den CLI-Flags heute. Vorrang: CLI vor Profil vor globaler Config.                                                                                                                                                                                                                                                                                                                                      |
| Konsolenfenster              | Eine Konsolen-exe in der Aufgabenplanung öffnet sichtbar ein Fenster. Deshalb kommt ein zweites Programm `src/bin/ordner-cleanup-bg.rs` mit `#![windows_subsystem = "windows"]` dazu. Es ruft dieselbe `app::run` auf und schreibt die Ausgaben in das Lauf-Protokoll. `schedule add` verweist auf dieses Programm neben der Haupt-exe.                                                                                                                                                                  |
| Aufgabenplanung              | Über `schtasks.exe /Create /TN \ordner-cleanup\<profil> /XML <datei> /F` bzw. `/Delete`/`/Query /XML`. Das XML erzeugen wir selbst. Einstellungen: `InteractiveToken`, `LeastPrivilege`, `DisallowStartIfOnBatteries`, `StopIfGoingOnBatteries`, `StartWhenAvailable`, Priorität 7, `MultipleInstancesPolicy=IgnoreNew`, `ExecutionTimeLimit=PT4H`. Die Wahrheit für die Anzeige ist ein eigenes Register `<data>\schedules.json`. `schtasks /Query /XML` prüft nur Existenz und exe-Pfad (`quick-xml`). |
| Benachrichtigung             | WinRT-Toast über die `windows`-Crate (`UI_Notifications`, `Data_Xml_Dom`). Die eigene AUMID wird ohne Adminrechte unter `HKCU\Software\Classes\AppUserModelId\WOsaka.OrdnerCleanup` registriert, beim ersten `schedule add`. Ein Klick öffnet den Bericht über `activationType="protocol"` mit `launch="file:///…/latest.html"`, ohne dass unser Programm läuft. Spike 0a bestätigt das. Rückfall: die Crate `tauri-winrt-notification` mit PowerShell-AUMID.                                            |
| Ablage                       | `<data>\history.db`, `<data>\reports\<profil>\`, `<data>\plans\<profil>\`, `<data>\runs\<profil>.jsonl` (Lauf-Protokoll), `<data>\schedules.json`, `<data>\scan.lock`. Pläne werden wie Berichte aufbewahrt (die letzten `reports_keep` Läufe); das präzisiert die Spec, die nur Berichte nennt.                                                                                                                                                                                                         |
| Vorlagen-Format              | Eine TOML-Datei mit Ebenen (`[[levels]]`). Die eingebauten Vorlagen sind ebenfalls TOML (`include_str!`); das hält das Format ehrlich.                                                                                                                                                                                                                                                                                                                                                                   |

### Teilwerte des Health-Scores (`METRICS_VERSION = 1`)

| Teilwert       | Gemessen                                                  | 0 Punkte ab           |
| -------------- | --------------------------------------------------------- | --------------------- |
| Müll           | max(Müll-Dateien je 1.000 Dateien, Müll-Bytes-Anteil)     | 50 je 1.000 bzw. 10 % |
| Duplikate      | verschwendete Bytes / lokale Bytes                        | 25 %                  |
| Problemdateien | Dateien mit Problemen je 1.000 Dateien                    | 50 je 1.000           |
| Struktur       | Struktur-Befunde je 100 Ordner                            | 30 je 100             |
| Versionen      | überzählige Versionen (Gruppengröße − 1) je 1.000 Dateien | 50 je 1.000           |
| Strukturtreue  | abweichende Einträge / geprüfte Einträge                  | 30 %                  |

Ohne Dateien ist jeder Teilwert 100. Teilwerte ohne Anwendung, etwa „Strukturtreue“ ohne Vorlage, fallen aus der Gewichtung. Weil jede Kurve monoton fällt und der Durchschnitt monoton ist, ist der Gesamtwert monoton.

## Files to Create

| File                                                         | Purpose                                                                                                                                                                 |
| ------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---- | ------- |
| `src/report/rows.rs`                                         | `FRow`, `DRow`, `load_files`, `load_dirs`, `load_errors` aus `build.rs` (`pub(crate)`)                                                                                  |
| `src/health/mod.rs`                                          | `Metrics` (Kennzahlen eines Bereichs), `Snapshot { root, folders: Vec<(name, Metrics)>, … }`, `compute(rows, ctx) -> Snapshot`                                          |
| `src/health/score.rs`                                        | Teilwerte, Gewichte, `Score { total, parts, deductions }`, `top_deductions(n)` mit Texten                                                                               |
| `src/history/mod.rs`                                         | `History::open(path)` mit eigener Schema-Version und Migration, `write(snapshot)`, `latest_comparable(root, fp)`, `series(root, folder, limit)`                         |
| `src/history/schema.sql`                                     | Tabellen `meta`, `snapshots`, `folder_metrics`                                                                                                                          |
| `src/scan/lock.rs`                                           | `ScanLock::acquire(path) -> Result<ScanLock, Busy>` (exklusiv geöffnete Datei, Freigabe per `Drop`)                                                                     |
| `src/template/mod.rs`                                        | Vorlagen-Modell, Laden mit `Spanned`-Zeilen, eingebaute Vorlagen (`para`, `johnny-decimal`), Auflösung „Name oder Pfad“                                                 |
| `src/template/check.rs`                                      | `check(rows, root, &Template) -> Vec<Deviation>` und der Anteil für „Strukturtreue“                                                                                     |
| `src/template/para.toml`, `src/template/johnny-decimal.toml` | Eingebaute Vorlagen                                                                                                                                                     |
| `src/schedule/mod.rs`                                        | Trait `TaskService` (`create`, `delete`, `query_command`), Register `schedules.json`, Trigger-Modell (`Weekly{day,time}`, `Daily{time}`), Berechnung des nächsten Laufs |
| `src/schedule/xml.rs`                                        | Task-XML erzeugen (escaped), exe-Pfad aus Query-XML lesen                                                                                                               |
| `src/schedule/schtasks.rs`                                   | Echte `TaskService` über `schtasks.exe`                                                                                                                                 |
| `src/platform/toast.rs`                                      | `Notifier`-Trait, `register_aumid()`, echter WinRT-Toast                                                                                                                |
| `src/app/history.rs`                                         | Befehl `history`                                                                                                                                                        |
| `src/app/profile.rs`                                         | Profil auflösen und zusammenführen, Befehl `profiles`                                                                                                                   |
| `src/app/run.rs`                                             | Befehl `run`: Sperre, Scan, Momentaufnahme, Bericht, Pläne, Aufbewahrung, Lauf-Protokoll, Benachrichtigung                                                              |
| `src/app/schedule.rs`                                        | Befehle `schedule add                                                                                                                                                   | list | remove` |
| `src/bin/ordner-cleanup-bg.rs`                               | Programm ohne Konsolenfenster, ruft `app::run` auf                                                                                                                      |
| `tests/cli_history.rs`                                       | Scan → Momentaufnahme → Bericht mit Verlauf; `--reset-index` erhält den Verlauf; `history`                                                                              |
| `tests/cli_profiles_run.rs`                                  | Profile, `run` (ohne echte Aufgabenplanung), Aufbewahrung, Lauf-Protokoll, Sperre                                                                                       |
| `tests/cli_templates.rs`                                     | Soll/Ist im Bericht für `para`, `johnny-decimal` und eine eigene Vorlage                                                                                                |

## Files to Modify

| File                                                                | Change Description                                                                                                                                                                                                                                                                      |
| ------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/report/build.rs`                                               | Zeilen kommen aus `rows.rs`, ohne Verhaltensänderung. Optional ein `history: Option<HistorySection>` und `template: Option<TemplateSection>` im `Report`.                                                                                                                               |
| `src/report/mod.rs`, `src/report/html.rs`, `src/report/report.html` | Abschnitte „Verlauf“ (Tabelle jetzt/letzter/Δ, Trend als inline-SVG, drei größte Abzüge, Hinweis „eingeschränkt“) und „Soll/Ist“. JSON enthält beide.                                                                                                                                   |
| `src/report/terminal.rs`                                            | Zeile mit dem Score und seiner Veränderung                                                                                                                                                                                                                                              |
| `src/change/junk.rs`                                                | `JunkClassifier` herauslösen, `plan_junk` darauf umstellen                                                                                                                                                                                                                              |
| `src/config.rs`                                                     | `profiles: BTreeMap<String, Profile>`, `health: HealthConfig { weights }`, `notify: NotifyConfig { score_drop, score_below }`, `reports_keep`. Profile werden beim Laden geprüft: Name `[a-z0-9-]+`, gültige Pläne, Wurzel nicht leer. `Config::with_profile(&Profile)` führt zusammen. |
| `src/cli.rs`                                                        | `--profile` bei `scan`, `report`, `plan …` und `history` (`path` wird optional: `required_unless_present = "profile"`, `conflicts_with`). `--template` bei `scan` und `report`. Neu: `history`, `profiles`, `run`, `schedule {add,list,remove}`.                                        |
| `src/app/mod.rs`                                                    | Sperre und Momentaufnahme in `scan_command`; Verlauf in `report_command`; Profil-Auflösung; Dispatch der neuen Befehle; `index_path` und die anderen Pfadhelfer wandern nach `paths.rs`                                                                                                 |
| `src/app/plan.rs`                                                   | `prepare` nimmt optional ein Profil (Wurzel, Config, Regeldatei). `--out` bekommt einen Default unter `plans\<profil>\`, wenn `run` aufruft.                                                                                                                                            |
| `src/paths.rs`                                                      | `history_path`, `reports_dir(profil)`, `plans_dir(profil)`, `runs_log(profil)`, `schedules_path`, `scan_lock_path`                                                                                                                                                                      |
| `Cargo.toml`                                                        | `windows` (Features `UI_Notifications`, `Data_Xml_Dom`, `Foundation`), `quick-xml`                                                                                                                                                                                                      |
| `README.md`                                                         | Verlauf, Health-Score (Formeln, Gewichte), Profile, `run`/`schedule`, Benachrichtigung, Vorlagen-Format                                                                                                                                                                                 |
| `docs/roadmap.md`                                                   | Status Phase 5                                                                                                                                                                                                                                                                          |
| `docs/features/laufender-betrieb.md`                                | Präzisierungen (Reihenfolge, Aufbewahrung der Pläne, Formeln). Am Ende `status: implemented` und „Manueller Test“.                                                                                                                                                                      |

## Data Model Changes

### `history.db` (SQLite, eigenes Schema v1)

```sql
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);   -- schema_version
CREATE TABLE snapshots (
    id INTEGER PRIMARY KEY,
    root_key TEXT NOT NULL, root_path TEXT NOT NULL,
    taken_at TEXT NOT NULL,            -- UTC, RFC 3339 (Sortierung)
    scan_finished_at TEXT,             -- verknüpft mit roots.finished_at im Index
    tool_version TEXT NOT NULL, metrics_version INTEGER NOT NULL,
    config_fp TEXT NOT NULL, template TEXT,   -- Name/Pfad der Vorlage, sonst NULL
    profile TEXT, scan_errors INTEGER NOT NULL
);
CREATE INDEX snapshots_root ON snapshots(root_key, taken_at);
CREATE TABLE folder_metrics (
    snapshot_id INTEGER NOT NULL REFERENCES snapshots(id),
    folder TEXT NOT NULL,              -- '' = Wurzel, '*' = Sonstige, sonst Ordnername
    score INTEGER NOT NULL,
    metrics TEXT NOT NULL,             -- JSON: Kennzahlen, Teilwerte, Abzüge
    PRIMARY KEY (snapshot_id, folder)
);
```

- Kennzahlen als JSON bleiben erweiterbar ohne Schema-Änderung. Nur `score` ist eine eigene Spalte, für schnelle Reihen.
- Ist die Datei beschädigt oder hat sie ein neueres Schema, gibt es einen Fehler mit Warnung; die Datei wird nie überschrieben oder neu angelegt.

### Kennzahlen (`health::Metrics`, serialisiert in `metrics`)

```text
size, files, dirs, cloud_files, cloud_bytes, junk_files, junk_bytes, dup_groups, dup_wasted,
problem_files, problems_by_kind{…}, empty_dirs, structure_issues, version_excess,
old_bytes, template_checked?, template_deviations?, parts{junk,dups,problems,structure,
versions,template?}, deductions[{part, points, text}]
```

### Config

```toml
reports_keep = 12

[health.weights]
junk = 20
duplicates = 25
problems = 20
structure = 20
versions = 15
template = 20

[notify]
score_drop = 5
score_below = 60

[profiles.downloads]
root = 'C:\Users\Oskar\Downloads'
rules_file = 'downloads.rules.toml'
plans = ["rules", "junk"]          # rules | junk | empty-dirs | archive | versions | dedupe
template = "para"                  # eingebaut oder Pfad (relativ zum Config-Ordner)
force = false                      # Netzlaufwerk erlauben
exclude = ["*.iso"]                # ergänzt; ebenso summary_only, protected_paths, junk_categories
old_after = "2y"                   # ersetzt; ebenso andere Einzelwerte
```

### Vorlage (TOML)

```toml
name = "PARA"
max_depth = 8
[[levels]]                        # Ebene 1 (direkt unter der Wurzel)
required = ["1 Projekte", "2 Bereiche", "3 Ressourcen", "4 Archiv"]
allow_other = false               # weitere Ordner sind Abweichungen
files = false                     # Dateien auf dieser Ebene sind Abweichungen
[[levels]]                        # Ebene 2
patterns = ['.+']
files = true
```

Ebenen ohne Eintrag sind frei, bis `max_depth`. `_Archiv` und `.ordner-cleanup` sind immer erlaubt. Johnny.Decimal nutzt `patterns` (`^\d0-\d9 `, `^\d\d `, `^\d\d\.\d\d `) und `files = false` auf den Ebenen 1 und 2.

### Lauf-Protokoll (`runs\<profil>.jsonl`)

```text
{"started":…, "ended":…, "status":"ok|skipped-locked|root-missing|failed",
 "score":72, "score_delta":-5, "report":"…", "plans":[{"kind":"rules","actions":3,"path":…}],
 "errors":["…"], "notified":true}
```

## API / Interface Changes

### CLI

```text
ordner-cleanup scan   [<pfad> | --profile X] [--template para|<datei>] …
ordner-cleanup report [<pfad> | --profile X] [--template …] …
ordner-cleanup plan <art> [<pfad> | --profile X] …
ordner-cleanup history [<pfad> | --profile X] [--folder <name>] [--limit 20]
ordner-cleanup profiles
ordner-cleanup run --profile X [--notify]          # --notify setzt nur die geplante Aufgabe
ordner-cleanup schedule add --profile X (--weekly MO 09:00 | --daily 09:00)
ordner-cleanup schedule list
ordner-cleanup schedule remove --profile X
```

- **Exit-Codes von `run`:** 0 ok, 2 Teilerfolg (ein Plan gescheitert, Scan mit Warnungen), 1 Lauf gescheitert bzw. Wurzel fehlt, 3 übersprungen wegen Sperre.
- **`--notify`:** Ohne den Schalter erscheint nie ein Toast. Manuelle Läufe bleiben damit still, wie in der Spec gefordert.

### Bibliothek (Auszug)

```rust
// health
pub struct MetricsCtx<'a> { junk: &'a JunkClassifier, problem_ctx: &'a ProblemCtx,
                            thresholds: &'a Thresholds, old_after_days: i64, now_ticks: i64,
                            template: Option<&'a Template>, weights: &'a Weights }
pub fn compute(rows: &Rows, dups: &[DupGroup], root: &ReportRoot, ctx: &MetricsCtx) -> Snapshot;

// history
impl History {
    pub fn open(path: &Path) -> Result<Self, HistoryError>;
    pub fn write(&mut self, s: &Snapshot, meta: &SnapshotMeta) -> Result<i64, HistoryError>;
    pub fn latest_comparable(&self, root_key: &str, fp: &str, before: Option<i64>)
        -> Result<Option<(StoredSnapshot, Comparable /* Full | Limited */)>, HistoryError>;
    pub fn series(&self, root_key: &str, folder: &str, limit: usize) -> Result<Vec<Point>, HistoryError>;
}

// schedule
pub trait TaskService { fn create(&self, name: &str, xml: &str) -> io::Result<()>;
                        fn delete(&self, name: &str) -> io::Result<()>;
                        fn query_command(&self, name: &str) -> io::Result<Option<PathBuf>>; }
// toast
pub trait Notifier { fn notify(&self, title: &str, body: &str, open: &Path) -> io::Result<()>; }
```

## Implementation Sequence

0. **Spikes** (Wegwerf-Code im Scratchpad, nichts committen; Ergebnis kommt als Absatz unter „Risks“):
   - (a) Toast aus einem unpaketierten Programm mit AUMID aus der Registry, Quelle „Ordner-Cleanup“, Klick öffnet eine `file:///`-HTML, auf Windows 11 und, falls verfügbar, Windows 10.
   - (b) `schtasks /Create /XML` ohne Adminrechte, mit Akku-Einstellungen und `StartWhenAvailable`. Startet `ordner-cleanup-bg.exe` ohne sichtbares Fenster? `/Query /XML` und `/Delete`.
   - (c) Kosten von `compute` auf dem 100.000-Dateien-Index aus `perf_plans` gegenüber der Scan-Dauer.
   - Depends on: nothing
1. **Vorbereitung ohne Verhaltensänderung** — `report/rows.rs` herauslösen, `JunkClassifier` aus `junk.rs`, Pfadhelfer nach `paths.rs`. Alle vorhandenen Tests bleiben grün.
   - Files: `src/report/{rows.rs, build.rs, mod.rs}`, `src/change/junk.rs`, `src/paths.rs`, `src/app/mod.rs`
   - Depends on: nothing
2. **Kennzahlen und Health-Score** — `health::compute` und `score` nach TDD. Tests:
   - jede Kennzahl an einem kleinen In-Memory-Index, einschließlich Cloud-only (zählt, wird nicht gelesen) und Links
   - Zuordnung zu Ordnern der ersten Ebene, Duplikate über zwei Ordner hinweg, mehr als 200 Ordner ergeben „Sonstige“
   - leere Wurzel ergibt 100
   - jede Kurve an 0, Schwelle und Mitte
   - Gewicht 0 schaltet ab; ohne Vorlage fehlt „Strukturtreue“
   - **Determinismus** (zweimal berechnet ergibt dasselbe) und **Monotonie** (Tabelle: mehr Müll, Duplikate, Probleme oder Abweichungen senken den Score nie weniger)
   - alte Daten ohne Abzug
   - Texte der Abzüge
   - Files: `src/health/*`, `src/config.rs` (`health.weights`), `src/lib.rs`
   - Depends on: 1
3. **Verlauf und Scan-Integration** — `History` mit Schema und Migrations-Rahmen, Fingerabdruck, `ScanLock`, Momentaufnahme in `scan_command`. Tests:
   - Schreiben und Lesen, `latest_comparable` (gleicher bzw. anderer Fingerabdruck, andere `METRICS_VERSION`, unvollständige Läufe ausgenommen)
   - beschädigte bzw. neuere `history.db`: Warnung, Scan läuft weiter, Datei unverändert
   - Sperre belegt: Fehler
   - `--reset-index` lässt den Verlauf stehen
   - abgebrochener Scan schreibt nichts
   - Files: `src/history/*`, `src/scan/lock.rs`, `src/app/mod.rs`
   - Depends on: 2
4. **Bericht und `history`** — Abschnitt „Verlauf“ in HTML (inline-SVG-Sparklines aus `series`, ohne externe Skripte) und JSON, Zeile im Terminal, Befehl `history`. Tests:
   - erster Lauf ergibt „erster Lauf“
   - zweiter Lauf zeigt Δ
   - „eingeschränkt“ nach geänderter Config
   - HTML enthält kein `<script src`
   - Tabelle von `history`
   - Files: `src/report/*`, `src/app/history.rs`, `src/cli.rs`, `tests/cli_history.rs`
   - Depends on: 3
5. **Profile** — Config-Modell, Prüfung, Zusammenführung, `--profile` bei `scan`/`report`/`plan`/`history`, Befehl `profiles`. Tests:
   - Vorrang CLI vor Profil vor Config
   - Listen werden ergänzt, Einzelwerte ersetzt
   - unbekanntes Profil, ungültiger Name, ungültiger Plan-Typ, unbekannter Schlüssel
   - `rules_file` relativ zum Config-Ordner
   - `path` und `--profile` zusammen sind ein Fehler
   - Files: `src/config.rs`, `src/cli.rs`, `src/app/{profile.rs, mod.rs, plan.rs}`, `tests/cli_profiles_run.rs`
   - Depends on: 3 (parallel zu 4 möglich)
6. **`run` und Hintergrundprogramm** — Ablauf: Sperre, Wurzel prüfen, Scan, Momentaufnahme, Bericht nach `reports\<profil>\` mit `latest.html`, Pläne nach `plans\<profil>\`, Aufbewahrung, Lauf-Protokoll, Exit-Code. Dazu `src/bin/ordner-cleanup-bg.rs`. Tests:
   - vollständiger `run` im Temp-Ordner: Baum unverändert außer den Tool-Dateien
   - 13 Läufe lassen 12 Berichte, 12 Pläne und `latest.html` übrig
   - Wurzel fehlt: Exit 1 mit Protokolleintrag
   - Sperre belegt: Exit 3 mit Protokolleintrag
   - Regeldatei fehlt: Scan und Bericht entstehen, `plan rules` scheitert und wird protokolliert, Exit 2
   - nie `apply`: Der Baum wird vor und nach dem Lauf verglichen
   - Files: `src/app/run.rs`, `src/bin/ordner-cleanup-bg.rs`, `src/paths.rs`, `tests/cli_profiles_run.rs`
   - Depends on: 4, 5
7. **Benachrichtigung** — `Notifier`-Trait, Entscheidung wann (Score-Abfall um mindestens `score_drop` gegenüber dem letzten vergleichbaren Lauf, Score unter `score_below`, Lauf gescheitert bzw. Wurzel fehlt, mindestens ein nicht leerer Plan, alles nur mit `--notify`), Texte, echter WinRT-Toast mit AUMID. Die Entscheidungslogik wird unit-getestet (Tabelle), der echte Toast nur manuell.
   - Files: `src/platform/toast.rs`, `src/app/run.rs`, `src/config.rs` (`notify`), `Cargo.toml`
   - Depends on: 6
8. **`schedule add|list|remove`** — XML-Erzeugung, `TaskService` über `schtasks`, Register `schedules.json`, Hinweis auf fehlendes Programm, ersetzen statt doppelt anlegen, AUMID-Registrierung beim ersten `add`. Tests:
   - XML gegen eine Golden-Datei (Akku, `StartWhenAvailable`, Priorität, `InteractiveToken`, Pfad mit Leerzeichen und `&` korrekt escaped)
   - Fake-`TaskService` für add/list/remove/replace
   - fehlende exe wird gemeldet
   - Berechnung des nächsten Laufs
   - echter `schtasks`-Test als `#[ignore]` unter `\ordner-cleanup-test\`
   - Files: `src/schedule/*`, `src/app/schedule.rs`, `src/cli.rs`, `Cargo.toml`
   - Depends on: 7
9. **Vorlagen (abtrennbar als 5b)** — Format mit Zeilenangaben bei Fehlern, eingebaute `para` und `johnny-decimal`, `template::check`, Abschnitt „Soll/Ist“ im Bericht (höchstens 100 je Art, plus Anzahl), Teilwert „Strukturtreue“, `--template` bzw. Profil-Feld, Fingerabdruck enthält die Vorlage. Tests:
   - Spec-Beispiel PARA: fehlender Pflicht-Ordner, unerwarteter Ordner, Dateien auf der obersten Ebene
   - Johnny.Decimal-Muster je Ebene
   - `max_depth`
   - `_Archiv` und `.ordner-cleanup` sind erlaubt
   - ungültige Regex bzw. unbekannter Schlüssel mit Zeile
   - Strukturtreue sinkt monoton
   - Files: `src/template/*`, `src/health/*`, `src/report/*`, `src/cli.rs`, `src/config.rs`, `tests/cli_templates.rs`
   - Depends on: 4, 5
10. **Performance und Doku** — `perf_plans` bzw. eigener Perf-Test: Kennzahlen und Momentaufnahme mit höchstens 5 % Anteil am Scan, Score und Verlauf für 100.000 Dateien unter 2 s, Bericht mit 52 Momentaufnahmen. Dazu README, Roadmap, `cargo clippy -- -D warnings`, `cargo fmt`.
    - Depends on: 9
11. **Manueller Test** — Ergebnis in der Spec:
    - Profil auf Downloads mit Vorlage
    - `schedule add --daily` mit naher Uhrzeit, zwei echte Läufe
    - Toast erscheint (Score-Abfall erzwungen mit Testmüll) und öffnet per Klick den Bericht
    - kein Konsolenfenster
    - Akku: Lauf startet nicht bzw. bricht ab
    - Rechner aus und wieder an: der Lauf wird nachgeholt
    - `schedule list` nach Verschieben der exe
    - `schedule remove`
    - Depends on: 10

## Test Strategy

- **Unit:**
  - Kennzahlen, Teilwerte, Score, Determinismus und Monotonie (Tabellentests)
  - `History` (Schema, Vergleichbarkeit, Beschädigung)
  - Config-Fingerabdruck (gleiche Config ergibt gleichen Abdruck; jede relevante Änderung einen anderen; irrelevante wie `top` keinen anderen)
  - Profil-Zusammenführung
  - Entscheidung über die Benachrichtigung
  - Task-XML (Golden-Datei, Escaping), Berechnung des nächsten Laufs
  - Vorlagen-Parser und -Prüfung
- **Integration (CLI, Temp-Ordner, `ORDNER_CLEANUP_HOME` isoliert):**
  - `cli_history`: Scan → Momentaufnahme → Bericht mit Verlauf → `--reset-index` → Verlauf erhalten
  - `cli_profiles_run`: `run` Ende-zu-Ende, Aufbewahrung, Sperre, fehlende Wurzel, kein `apply`
  - `cli_templates`: Soll/Ist im Bericht
- **Fakes statt Systemeingriff:** `TaskService` und `Notifier` werden in allen automatischen Tests gefälscht. Echte Aufgabenplanung und Toasts gibt es nur in `#[ignore]`-Tests bzw. im manuellen Test.
- **Rückwärtskompatibilität:** Alle Tests aus Phase 1–4 bleiben grün. Befehle ohne `--profile` verhalten sich wie bisher, bestehende `config.toml` bleiben gültig, und der Bericht ohne Verlauf (erster Lauf bzw. `history.db` beschädigt) bleibt vollständig.
- Vorgehen nach `test-driven-development`, vor `status: implemented` `verification-before-completion`.

## Risks & Open Questions

- **Toast aus einem unpaketierten Programm:** Ob die AUMID aus der Registry ohne Startmenü-Verknüpfung auf allen Windows-11-Builds reicht, ist nicht sicher (Spike 0a). Rückfall: eine Verknüpfung im Startmenü mit AUMID beim `schedule add` (ohne Adminrechte), oder die PowerShell-AUMID (dann steht „Windows PowerShell“ als Absender da). Die Benachrichtigung ist ein Komfort-Feature; der Bericht entsteht immer.
- **Konsolenfenster:** Gelöst durch das zweite Programm mit `windows_subsystem`. Kosten: ein zweites Binary im Release; im README dokumentiert.
- **`schtasks` und Sprache:** Die Ausgabe von `/Query` ist lokalisiert. Deshalb nutzen wir nur `/XML` (nicht lokalisiert) und den Exit-Code. Die Anzeige von `schedule list` kommt aus dem eigenen Register; die Aufgabenplanung wird nur auf Existenz und exe-Pfad geprüft.
- **Laufzeit der Kennzahlen:** `compute` lädt dieselben Zeilen wie der Bericht. Wenn Spike 0c zeigt, dass das mehr als 5 % kostet, werden die Zeilen einmal geladen und von Momentaufnahme und Bericht gemeinsam genutzt (in `run` ohnehin der Fall), oder die Ordner werden in einem Durchlauf statt pro Ordner aggregiert.
- **Grenzen der Schwellen:** Die Kurven sind Erfahrungswerte. Abhilfe: `METRICS_VERSION` macht spätere Anpassungen sichtbar (Vergleich „eingeschränkt“ statt falscher Trends), und im manuellen Test wird geprüft, ob echte Ordner plausible Werte bekommen.
- **Zwei Profile mit derselben Wurzel:** Der Verlauf gehört zur Wurzel. Verschiedene Vorlagen ergeben verschiedene Fingerabdrücke, und der Vergleich nutzt den letzten Lauf mit demselben Abdruck. So stören sich die Profile nicht.
- **Sperre und manueller Scan:** Die Sperre gilt für `scan` und `run`, nicht für `report`, `plan` und `apply`. SQLite im WAL-Modus erlaubt dort gleichzeitiges Lesen, wie bisher.
- [ ] Ergebnis der Spikes 0a bis 0c hier eintragen, bevor Schritt 7 bzw. 8 beginnt.
