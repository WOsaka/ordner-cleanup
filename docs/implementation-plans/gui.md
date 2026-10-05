---
title: "Implementation Plan: Grafische Oberfläche (Phase 6b)"
feature_spec: docs/features/gui.md
status: approved   # pending-approval | approved | implemented
created: 2026-10-05
updated: 2026-10-05
---

# Implementation Plan: Grafische Oberfläche (Phase 6b)

## Summary

Die GUI wird eine dünne egui-Schicht über einer neuen, für CLI und GUI gemeinsamen **Operationen-Schicht** (`src/ops/`). Die Codeanalyse zeigt: Die Fachlogik ist schon sauber getrennt (`scan`, `report::build`, `plan_*`, `apply_plan`, `undo_run`, `list_runs`, `purge_run`, `schedule::add` …). Die Steuerung dazwischen steckt aber in `src/app/*`, und zwar vermischt mit `println!`, `indicatif`, `confirm()` und dem globalen Strg+C-Flag. Konkret: Wurzel und Profil auflösen, Config zusammenführen, Hinweise erzeugen, Plan-Pfad bestimmen, Lauf registrieren.

Die Umsetzung hat drei Teile:

1. **Operationen herauslösen** (`src/ops/`): Jeder Befehl wird zu einer Funktion mit Anfrage-Struktur und strukturiertem Ergebnis. Hinweise und Warnungen sind Daten, Abbruch und Fortschritt kommen über einen `OpCtx` herein, und Rückfragen stellt der Aufrufer zwischen „prüfen“ und „ausführen“. `src/app/*` wird zum reinen Terminal-Frontend darüber. Das Verhalten der CLI bleibt bytegleich, die bestehenden `tests/cli_*.rs` sichern das ab.
2. **GUI Stufe 1** (`src/gui/`, `src/bin/ordner-cleanup-gui.rs`): eframe-App mit Seitenleiste und einem Task-Runner, der jede Operation in einem Worker-Thread mit Abbruch-Flag, gemeinsamem Fortschritt und `catch_unwind` ausführt. Ansichten: Übersicht, Scan/Classify, Analyse (aus dem vorhandenen `report::Report`-Modell), Aufräumen mit Review-Modell (rein, testbar), Apply-Dialog und Ergebnis, Verlauf.
3. **GUI Stufe 2** (`src/settings/`, `src/gui/views/settings/`): Die TOML-Dateien (`config.toml` samt Profilen, `rules.toml`, `categories.toml`, Vorlagen) werden kommentarerhaltend über `toml_edit` bearbeitet. Vor dem Schreiben prüft immer der vorhandene Parser (`Config::parse`, `RuleSet::parse`, `CategoryDefs`, `template::Template::parse`), danach wird mit Sicherung `.bak` atomar geschrieben. Formulare entstehen aus Feldbeschreibungen; für Kategorien und Vorlagen gibt es einen validierten Roh-TOML-Editor.

Ein Spike in Schritt 0 klärt die technischen Unbekannten: egui-Version, Render-Backend in VM/RDP, CI-Sperre, Binary-Größe, Schriften, Ordnerdialog und Tabelle mit 100.000 Zeilen.

## Entscheidungen aus Spec und Codeanalyse

| Thema | Entscheidung |
|---|---|
| Gemeinsame Logik | Neues Modul `src/ops/`, öffentlich in der Bibliothek. `src/app/*` ruft nur noch `ops::*` auf und formatiert die Ergebnisse für das Terminal. Texte, die CLI und GUI teilen (z. B. `ActionCounts::plan_text`, `SkipReason`-Display, `status_label`), wandern dorthin, wo beide sie erreichen (`change::*` bzw. `ops::text`). |
| Rückfragen | Keine Callback-Schnittstelle. Jede schreibende Operation hat ein `*_check` (liefert Zusammenfassung, Warnungen und ob `allow_large` nötig ist, ohne etwas zu verändern) und ein `*_execute`. Die CLI fragt dazwischen per `confirm()`, die GUI per Dialog. |
| Abbruch | `OpCtx { cancel: Arc<AtomicBool>, progress: Arc<TaskProgress> }`. Die CLI übergibt `global_cancel_flag()`, die GUI ein frisches Flag je Task. `run_scan` bekommt das Flag als Parameter, statt es selbst zu holen. `UndoEnv` bekommt ein `cancel`: geprüft wird zwischen Einträgen, Ergebnis ist der vorhandene Status `PartiallyUndone`. Purge prüft zwischen Läufen. Planer ohne Klassifikation sind indexbasiert und schnell und haben keinen Abbruch; die GUI verwirft dann nur das Ergebnis. `plan rules` mit Nachklassifizierung reicht das Flag an `classify_files` weiter. |
| Fortschritt | `TaskProgress` mit Atomics (`done`, `total`, `bytes`, `errors`) und `Mutex<String>` für Phase und aktuellen Pfad. Die GUI liest ihn je Frame (`request_repaint_after(100 ms)`), die CLI zeichnet daraus wie bisher `indicatif`. Der Scan behält seinen `walker::Progress`; `ops::scan` spiegelt ihn in `TaskProgress`, damit der Walker unverändert bleibt. Classify ersetzt seine interne `ProgressBar` durch `TaskProgress`. |
| Analyse-Daten | Die GUI zeigt das vorhandene `report::Report` (inkl. `history`, `template`, `content`), das wie bei `report` in einem Worker per `ops::report_model` gebaut wird. Der Größenbaum ist darin vollständig (`TreeNode`), also braucht es kein Nachladen aus dem Index. Gezeichnet werden nur aufgeklappte Knoten. Nach Scan, Classify oder Apply wird das Modell der betroffenen Wurzel neu gebaut. |
| Datenbankzugriff | Der UI-Thread öffnet nie `Index` oder `History`, jeder Worker öffnet eine eigene Verbindung. `Index::open` und `History::open` bekommen `busy_timeout(5 s)` (heute nur WAL), damit GUI-Worker und ein geplanter CLI-Lauf nicht mit `SQLITE_BUSY` abbrechen. |
| Sperren | Unverändert: `ScanLock` für Scan, Classify und `plan rules` mit Nachklassifizierung. Ein belegtes Lock wird zum `ops::Error::Busy`, und die GUI zeigt „Ein anderer Scan läuft“ mit „Erneut versuchen“. Apply und Undo haben wie in der CLI keine globale Sperre; zwei gleichzeitige Applies desselben Plans laufen in `AlreadyDone` bzw. `Stale`. |
| Plan-Speicherort | GUI-Pläne liegen in `paths::gui_plans_dir()` = `%LOCALAPPDATA%\ordner-cleanup\plans\_gui\`. Der Unterstrich ist in Profilnamen verboten, es gibt also keine Kollision mit `plans\<profil>`, und `reports_keep` räumt GUI-Pläne nicht weg. Überschreibbar mit der neuen Config `[gui] plans_dir`. Die CLI bleibt beim aktuellen Ordner. „Plan öffnen“ listet die neuesten Pläne aus `_gui` und allen `plans\<profil>` und hat dazu einen Dateidialog. |
| Teilplan bei Abwahl | `Plan::subset(&self, keep_ids) -> Plan`: Die Aktionen werden gefiltert, IDs, `skipped`, `kind`, `root`, `created` und `keep_strategy` bleiben. Dazu kommen `params["auswahl_von"]` (Dateiname des Originals) und `params["auswahl"]` (`480/500`). `PLAN_VERSION` bleibt. Gespeichert wird `<stem>-auswahl-<JJJJMMTT-HHMMSS>.json` neben dem Original, sonst in `gui_plans_dir()`. `validate()` gilt für Teilmengen unverändert: eindeutige IDs, `keep` nie selbst als Aktion. |
| Abhängigkeiten im Review | `change::plan::dependents(plan, id) -> Vec<u32>`: Aktionen, die scheitern, wenn `id` fehlt. Das sind `remove-dir` auf Vorfahren-Ordnern einer abgewählten Aktion (empty-dirs) und, bei `move` ganzer Ordner (archive), Aktionen auf Vorfahren. Beim Abwählen zeigt die Review-Liste „N abhängige Einträge ebenfalls abwählen?“; Ablehnen ist erlaubt, `apply` überspringt dann mit `NotEmpty`. Eine Baumansicht wird nicht gebaut, die flache Liste hat eine Spalte „Ordner“ und den Filter „unterhalb von“. |
| GUI-Framework | `eframe` mit `default-features = false` und nur den nötigen Features (`accesskit`, `default_fonts`, `persistence`, Renderer). `egui_extras` mit `default-features = false` (die Features `http`/`image` würden `ehttp`/TLS ziehen und die CI-Sperre brechen). `egui_plot` für den Verlauf. Die Versionen legt Spike 0 fest. |
| Render-Backend | Vorläufig `wgpu` (nur DX12-Backend), denn D3D läuft in VMs und RDP über WARP. OpenGL fällt dort oft auf GDI 1.1 zurück, und `glow` startet dann nicht. Scheitert `wgpu` beim Start, versucht `main` es einmal mit `Renderer::Glow` (beide Features aktiv). Spike 0 misst Binary-Größe und Start auf einem lokalen PC, in einer VM und per RDP und darf das umdrehen. |
| Schriften | Beim Start werden `C:\Windows\Fonts\segoeui.ttf` (Text) und `consola.ttf` (Pfade/TOML) geladen, außerdem `seguisym.ttf` und, falls vorhanden, `msyh.ttc` als Fallback, damit Pfade mit Sonderzeichen, CJK usw. lesbar sind. Fehlen die Dateien, bleiben die egui-Default-Schriften. |
| Ordner- und Dateidialog | `rfd` (synchron, Windows-Backend über COM `IFileDialog`), falls Spike 0 keine unerwünschten Abhängigkeiten zeigt; sonst eigener `IFileOpenDialog` über das vorhandene `windows`-Crate (`FOS_PICKFOLDERS`). Der Dialog läuft im Worker, damit die UI nicht blockiert. |
| Explorer-Integration | `platform::shell`: `reveal(path)` (`SHOpenFolderAndSelectItems` über `ILCreateFromPathW`), `open(path)` (`ShellExecuteW` „open“), `create_start_menu_shortcut()` (`IShellLinkW` + `IPersistFile` nach `%APPDATA%\Microsoft\Windows\Start Menu\Programs`). `open` prüft vorher `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`/`OFFLINE` und verweigert Cloud-only-Dateien (der Knopf ist dann deaktiviert). |
| OCR-Hilfsprozess | `platform::ocr::default_worker()` findet `ordner-cleanup.exe` neben der GUI-exe, sonst nimmt es die laufende exe. Damit auch eine allein liegende GUI-exe OCR kann, prüft `ordner-cleanup-gui.rs` als Erstes `args[1] == "ocr-worker"` und gibt dann an `Cli::parse()` + `app::run` ab (wie die bg-exe). |
| TOML bearbeiten | Neue Abhängigkeit `toml_edit` (gleiche Familie wie `toml` 0.9, reines Rust). Ablauf immer: Dokument laden → ändern → `to_string()` → mit dem vorhandenen Parser prüfen (Fehler mit Zeile, wie heute) → erst bei Erfolg `.bak` anlegen und atomar schreiben (temporäre Datei + `MoveFileExW(REPLACE_EXISTING)`). Kommentare und Reihenfolge bleiben erhalten. |
| Externe Änderungen | Beim Laden merkt sich ein Editor `(mtime, len, xxh3)` der Datei. Vor dem Speichern wird verglichen; bei Abweichung erscheint der Dialog „neu laden / überschreiben / abbrechen“. |
| Formulare | Feldbeschreibungen (`FieldSpec { key, label, help, kind }` mit `kind` ∈ Bool, Int, Text, Duration, Size, Float, TextList, PathList, Choice) für Config, `[classify]`, `[llm]`, `[notify]`, `[health]`, Profile und Regeln. Ein generischer Formular-Renderer zeigt Wert bzw. Default (aus `Config::default()`), „zurücksetzen“ entfernt den Schlüssel. Dauer- und Größenfelder prüfen sofort mit `parse_old_after`/`parse_bytes`. Listen von Tabellen (`junk_rules`, `[[rules]]`, `[[senders]]`) bekommen Listen-plus-Formular-Ansichten. |
| Kategorien und Vorlagen | Roh-TOML-Editor mit Zeilennummern, Validierung beim Tippen (entprellt, 300 ms), Fehlerzeile markiert. Dazu gibt es „Eingebaute Vorlage als eigene kopieren“ und eine Liste der Kategorien (Name, Schlüsselwörter, Quelle) zum Ansehen. Ein volles Formular für Kategorien ist nicht vorgesehen; die Spec verlangt Bearbeitbarkeit, nicht die Form. |
| Regel-Vorschau | „Treffer anzeigen“ ruft im Worker `ops::plan(PlanRequest::Rules { rules_text: Some(entwurf), … }, dry: true)` auf. Das ist dieselbe Logik wie `plan rules`, nur mit dem Regel-Entwurf statt der Datei und ohne zu speichern; das Ergebnis erscheint in der Review-Tabelle, nur lesend. |
| LLM-Test | „Verbindung testen“ führt `LlmClient::new(&cfg)?.check()` im Worker aus (`content/llm.rs`, mit vorhandener Loopback-Prüfung). |
| Zeitpläne | `ops::schedule_add/remove/list` (aus `app/schedule.rs`). `bg_exe()` sucht `ordner-cleanup-bg.exe` neben der laufenden exe; die GUI-exe liegt daneben, das passt. Fehlt sie, kommt die Meldung aus `bg_exe()`. |
| Texte | Alle sichtbaren GUI-Texte stehen in `src/gui/texts.rs` (Konstanten und kleine Formatierfunktionen); Fachtexte (`SkipReason`, `ActionCounts`) kommen aus der Bibliothek. Ein i18n-Framework gibt es nicht. |
| Persistenz der GUI | eframe-`persistence` (Fenstergröße, zuletzt gewählte Wurzel bzw. Profil, Spaltenbreiten) unter `%APPDATA%\ordner-cleanup\gui\`. Nur Komfort: fehlt die Datei oder ist sie defekt, startet die GUI mit Defaults. |
| Panics | Jeder Worker läuft in `catch_unwind`. Ein Panic wird zu `TaskEvent::Failed("Interner Fehler: …")`, der Task gilt als abgebrochen. Ein Panic-Hook schreibt zusätzlich nach `%LOCALAPPDATA%\ordner-cleanup\gui-errors.log`, weil die GUI-exe keine Konsole hat. `apply` schreibt das Journal schon heute vor jeder Aktion, deshalb bleibt es konsistent (bestehende Crash-Tests in `apply.rs`). |
| Schließen | `viewport().close_requested()`: Läuft ein Task, wird das Schließen verhindert (`ViewportCommand::CancelClose`) und ein Dialog angezeigt. Bei „Abbrechen und schließen“ wird das Flag gesetzt, „Wird beendet …“ angezeigt und nach `TaskEvent::Done` geschlossen. |
| Tests der Oberfläche | Die Logik (Review-Modell, Formulare, Task-Runner, Editor-Dokumente) wird ohne Fenster per Unit-Test geprüft. Zusätzlich gibt es wenige Smoke-Tests mit `egui_kittest` (Dev-Abhängigkeit, AccessKit-basiert, ohne GPU), falls Spike 0 bestätigt, dass es auf dem CI-Runner läuft; sonst entfallen sie, und der manuelle Test deckt das ab. |
| CI | Die Sperrprüfung `cargo tree -e normal` bleibt unverändert und muss mit den neuen Crates grün sein. Artefakt: alle drei exe (`ordner-cleanup.exe`, `ordner-cleanup-bg.exe`, `ordner-cleanup-gui.exe`). |

## Files to Create

| File | Purpose |
|---|---|
| `src/ops/mod.rs` | `OpCtx`, `TaskProgress`, `Notes` (Hinweise, Warnungen), `ops::Error` (u. a. `Busy`, `NoIndex`, `LimitExceeded`), gemeinsame Helfer (`load_config`, `resolve_root`, `normalize`, `known_roots`, `onedrive_*`, `index_age_note`), die aus `app/mod.rs` hierher ziehen |
| `src/ops/target.rs` | `Target` (Pfad oder Profil → Wurzel + zusammengeführte Config), aus `app/profile.rs` |
| `src/ops/scan.rs` | `ScanRequest`, `scan(req, ctx) -> ScanReport` (Outcome, Momentaufnahme, Score-Zeile als Daten, Hinweise) |
| `src/ops/classify.rs` | `ClassifyRequest`, `classify(req, ctx) -> ClassifySummary`; die Logik aus `app/classify.rs` (ohne Ausgabe) |
| `src/ops/report.rs` | `report_model(req, ctx) -> (Report, ReportRoot, Notes)`, `export(model, formats, dir) -> Vec<PathBuf>` |
| `src/ops/plan.rs` | `PlanRequest` (enum je Art), `plan(req, ctx) -> PlanOutcome` (Plan, gespeicherter Pfad, Kennzahlen, Gründe der Übersprungenen, Hinweise, Obergrenze); `save_subset` |
| `src/ops/apply.rs` | `apply_check(plan) -> ApplyCheck`, `apply_execute(plan, plan_name, allow_large, ctx) -> ApplyOutcome` (inkl. Register-Eintrag) |
| `src/ops/runs.rs` | `runs(root)`, `undo_check/undo_execute`, `purge_candidates/purge_execute`, `index_roots/index_remove` |
| `src/ops/admin.rs` | `profiles()`, `schedule_list/add/remove`, `history(root|profile)`, `llm_check(cfg)` |
| `src/ops/text.rs` | Gemeinsame Beschriftungen (`status_label`, `local_time`, Zusammenfassungs-Texte), die heute in `app/*` stehen |
| `src/settings/mod.rs` | `EditableFile` (Pfad, Fingerabdruck, `toml_edit::DocumentMut`), `save_validated(file, validator)` mit `.bak`, atomarem Schreiben und Prüfung auf externe Änderung |
| `src/settings/fields.rs` | `FieldSpec`/`FieldKind`, Feldbeschreibungen für Config, Unterabschnitte, Profil und Regel; Lesen und Schreiben eines Feldes in einer `toml_edit::Table`; Validierung pro Feld |
| `src/settings/rules_doc.rs` | Regeln im Dokument: auflisten, hinzufügen, duplizieren, löschen, verschieben, eine Regel als Feldwerte lesen und schreiben; Entwurfstext für die Vorschau |
| `src/settings/config_doc.rs` | Config- und Profil-Abschnitte im Dokument; `[profiles.<name>]` anlegen, umbenennen (mit Prüfung `valid_profile_name`), löschen |
| `src/gui/mod.rs` | `pub fn run() -> anyhow::Result<()>`: Optionen, Renderer-Fallback, Schriften, Panic-Hook, Start von `GuiApp` |
| `src/gui/app.rs` | `GuiApp` (eframe::App): Seitenleiste, aktive Ansicht, Dialog-Stapel, Statuszeile mit laufendem Task, Schließen-Logik |
| `src/gui/tasks.rs` | `TaskRunner`: `spawn(name, kind, f: FnOnce(&OpCtx) -> Result<T>)`, `TaskEvent`, Abbruch, `catch_unwind`, Repaint über `egui::Context`; höchstens ein schreibender Task gleichzeitig |
| `src/gui/texts.rs` | Alle Oberflächentexte |
| `src/gui/fonts.rs` | Segoe UI, Consolas, Fallback-Schriften |
| `src/gui/widgets/table.rs` | Virtualisierte Tabelle über `egui_extras::TableBuilder` mit sortierbaren Köpfen, Auswahlspalte und Kontextmenü (Explorer, Pfad kopieren, öffnen) |
| `src/gui/widgets/form.rs` | Generischer Formular-Renderer für `FieldSpec` |
| `src/gui/widgets/toml_editor.rs` | Roh-TOML-Editor (`TextEdit::multiline`, Monospace, Zeilennummern, Fehlerzeile) |
| `src/gui/widgets/dialogs.rs` | Bestätigungs-, Fehler- und Konfliktdialoge |
| `src/gui/review.rs` | **Reines** Review-Modell: Zeilen aus `Plan`, Filter (Aktion, Regel, Ordner, Größe, Text, übersprungene), Sortierung, Auswahl (einzeln, alle, gefiltert, Ordner), Zusammenfassung, Abhängigkeitswarnung, `selected_ids()` |
| `src/gui/views/overview.rs` | Übersicht: Wurzeln und Profile mit Score und Trend, letzte Läufe, „Zum Prüfen“, Quarantäne |
| `src/gui/views/scan.rs` | Ordner- bzw. Profilauswahl, Scan- und Classify-Optionen, Fortschritt |
| `src/gui/views/analysis.rs` | Tabs aus `report::Report`: Größenbaum, Typen & Alter, Duplikate, Probleme & Struktur, Inhalte, Health & Verlauf (`egui_plot`), Soll/Ist; Export |
| `src/gui/views/cleanup.rs` | Plan-Art und Optionen, „Plan öffnen“ (neueste Pläne und Dialog), danach Review |
| `src/gui/views/review.rs` | Darstellung des Review-Modells: Filterleiste, Tabelle, Detailbereich, „Anwenden“ |
| `src/gui/views/apply.rs` | Bestätigungsdialog (Anzahl und Größe je Typ, Ziel, Warnungen, Haken für die Obergrenze), Fortschritt, Ergebnisansicht |
| `src/gui/views/history.rs` | Läufe (Filter Wurzel, Details aus dem Journal), Undo, Purge (Vorschau), Index-Wurzeln |
| `src/gui/views/settings/mod.rs` | Einstellungen-Navigation; in Stufe 1 nur „Datei im Editor öffnen“ und Neuladen |
| `src/gui/views/settings/rules.rs` | Regel-Editor: Liste, Formular, Platzhalter-Auswahl, Vorschau, Roh-TOML |
| `src/gui/views/settings/profiles.rs` | Profile: Liste, Formular, anlegen, umbenennen, löschen; Zeitplan je Profil |
| `src/gui/views/settings/config.rs` | Allgemeine Config, Schutz, Junk-Regeln, OneDrive, `[gui]` |
| `src/gui/views/settings/content.rs` | Klassifikation, LLM (mit Test), Kategorien (Roh-TOML) |
| `src/gui/views/settings/templates.rs` | Vorlagen: eingebaute ansehen und kopieren, eigene bearbeiten (Roh-TOML) |
| `src/platform/shell.rs` | `reveal`, `open`, `pick_folder`/`pick_file` (falls nicht `rfd`), `create_start_menu_shortcut` |
| `src/bin/ordner-cleanup-gui.rs` | `#![windows_subsystem = "windows"]`; `ocr-worker` weiterreichen, sonst `gui::run()` |
| `tests/gui_roundtrip.rs` | Rundreisen über `ops` und die CLI-exe (siehe Test Strategy) |

## Files to Modify

| File | Change Description |
|---|---|
| `Cargo.toml` | Neues `[[bin]] ordner-cleanup-gui`; Abhängigkeiten `eframe`, `egui_extras`, `egui_plot`, `toml_edit`, ggf. `rfd`; Dev-Abhängigkeit ggf. `egui_kittest`; zusätzliche `windows`-Features (`Win32_UI_Shell_Common`, `Win32_System_Com_StructuredStorage` für `IShellLinkW`/`IPersistFile`) |
| `.github/workflows/ci.yml` | Artefakt mit allen drei exe; die Sperrprüfung bleibt |
| `src/lib.rs` | `pub mod ops; pub mod settings; pub mod gui;` |
| `src/app/mod.rs` | Wird zum Terminal-Frontend: Helfer ziehen nach `ops`, Befehle rufen `ops::*` auf, `confirm()` zwischen `*_check` und `*_execute`, `indicatif` liest `TaskProgress`. `global_cancel_flag` bleibt hier (nur CLI). Ausgabe bytegleich |
| `src/app/plan.rs`, `src/app/classify.rs`, `src/app/run.rs`, `src/app/schedule.rs`, `src/app/profile.rs`, `src/app/history.rs`, `src/app/snapshot.rs` | Logik nach `ops::*`; übrig bleiben Argument-Umsetzung (`*Args` → `*Request`) und Ausgabe. `run` (geplanter Lauf) nutzt dieselben `ops`-Funktionen |
| `src/change/plan.rs` | `Plan::subset`, `dependents(plan, id)`; Tests |
| `src/change/undo.rs` | `UndoEnv.cancel`; Abbruch zwischen Einträgen → `PartiallyUndone`; Tests |
| `src/change/mod.rs` | Ggf. Texte für GUI-Labels öffentlich (`SkipReason::label()` neben `Display`) |
| `src/config.rs` | `[gui]` mit `plans_dir: Option<String>` (`GuiConfig`, `deny_unknown_fields`); Tests |
| `src/paths.rs` | `gui_plans_dir(config)`, `gui_state_dir()`, `gui_error_log()` |
| `src/index/mod.rs`, `src/history/mod.rs` | `busy_timeout(5 s)` beim Öffnen |
| `src/platform/mod.rs` | `pub mod shell;` |
| `README.md` | Abschnitt „GUI“: Start, Ablauf, Speicherorte, Gleichwertigkeit mit der CLI |
| `docs/roadmap.md` | Feature-Status Phase 6b |
| `docs/features/gui.md` | Status nach Umsetzung; Ergebnis des manuellen Tests |

## Data Model Changes

### Config
```toml
[gui]
# Ablage der in der GUI erzeugten Pläne (Default: %LOCALAPPDATA%\ordner-cleanup\plans\_gui)
plans_dir = "D:\\Pläne"
```
`GuiConfig { plans_dir: Option<String> }`, `#[serde(default, deny_unknown_fields)]`. Eine ältere exe lehnt eine Config mit `[gui]` ab (wegen `deny_unknown_fields`). Das ist akzeptiert, weil CLI, bg-exe und GUI-exe immer gemeinsam ausgeliefert werden.

### Plan-Datei
Kein Formatwechsel, `PLAN_VERSION` bleibt 2. Ein Teilplan ist ein normaler Plan mit zwei zusätzlichen Einträgen in `params`:
```json
"params": { "auswahl_von": "plan-20261005-101500.json", "auswahl": "480/500", "...": "..." }
```

### Undo
`UndoEnv { fs, now, cancel: &AtomicBool }`. Ein abgebrochenes Undo hinterlässt den Status `PartiallyUndone` (gibt es schon); ein weiteres `undo` setzt fort.

### Index und Verlauf
Kein Schemawechsel; nur `busy_timeout` beim Öffnen.

### GUI-Zustand (nur lokal, nicht vertraglich)
eframe-Persistenz unter `%APPDATA%\ordner-cleanup\gui\`: Fenster, letzte Wurzel bzw. Profil, Spaltenbreiten, zuletzt geöffnete Pläne.

## API / Interface Changes

### CLI
Keine. Befehle, Optionen, Ausgaben und Exit-Codes bleiben gleich; `tests/cli_*.rs` laufen unverändert.

### Neue exe
`ordner-cleanup-gui.exe` ohne Argumente startet die GUI. `ordner-cleanup-gui.exe ocr-worker …` verhält sich wie die CLI (intern).

### Bibliothek `ops` (Auszug)
```rust
pub struct OpCtx { pub cancel: Arc<AtomicBool>, pub progress: Arc<TaskProgress> }
pub struct TaskProgress { pub done: AtomicU64, pub total: AtomicU64, pub bytes: AtomicU64,
                          pub errors: AtomicU64, pub phase: Mutex<String>, pub current: Mutex<String> }
pub struct Notes { pub hints: Vec<String>, pub warnings: Vec<String> }

pub enum TargetSpec { Path { path: PathBuf, force: bool }, Profile(String) }

pub struct ScanRequest { pub target: TargetSpec, pub exclude: Vec<String>, pub summary_only: Vec<String>,
                         pub no_default_excludes: bool, pub reset_index: bool, pub threads: Option<usize>,
                         pub template: Option<String> }
pub fn scan(req: &ScanRequest, ctx: &OpCtx) -> Result<ScanReport>;          // nimmt ScanLock

pub struct ClassifyRequest { pub target: TargetSpec, pub no_llm: bool, pub force: bool,
                             pub ext: Option<String>, pub only: Option<String>, pub clear: bool }
pub fn classify(req: &ClassifyRequest, ctx: &OpCtx) -> Result<ClassifySummary>;

pub fn report_model(req: &ReportRequest, ctx: &OpCtx) -> Result<ReportView>; // Report + Root + Notes
pub fn export_report(view: &ReportView, formats: &[Format], out: Option<&Path>) -> Result<Vec<PathBuf>>;

pub enum PlanKindRequest {
    Dedupe { keep: String }, Junk { categories: Vec<String> }, EmptyDirs,
    Archive { older_than: Option<String> }, Versions { min_age: Option<String> },
    Rules { rules_file: Option<PathBuf>, rules_text: Option<String>, only: Vec<String>, no_classify: bool },
}
pub struct PlanRequest { pub target: TargetSpec, pub kind: PlanKindRequest,
                         pub out: PlanOut /* Cwd | GuiDir | Path(PathBuf) | DontSave */ }
pub struct PlanOutcome { pub plan: Plan, pub saved: Option<PathBuf>, pub bytes: u64,
                         pub skipped_by_reason: BTreeMap<String, usize>, pub per_rule: Vec<RuleStats>,
                         pub limit: Option<String>, pub notes: Notes, /* … */ }
pub fn plan(req: &PlanRequest, ctx: &OpCtx) -> Result<PlanOutcome>;
pub fn save_subset(original: &Path, plan: &Plan, keep: &HashSet<u32>, cfg: &Config) -> Result<PathBuf>;

pub struct ApplyCheck { pub counts: ActionCounts, pub bytes: u64, pub limit: Option<String>,
                        pub notes: Notes, pub empty: bool }
pub fn apply_check(plan: &Plan) -> Result<ApplyCheck>;
pub fn apply_execute(plan: &Plan, plan_name: &str, allow_large: bool, ctx: &OpCtx) -> Result<ApplyOutcome>;

pub fn runs(root: Option<&Path>) -> Result<Vec<(PathBuf, Vec<RunSummary>)>>;
pub fn undo_check(run: &RunId, root: Option<&Path>) -> Result<UndoCheck>;
pub fn undo_execute(run: &RunId, root: &Path, ctx: &OpCtx) -> Result<UndoOutcome>;
pub fn purge_candidates(older_than: Option<&str>, root: Option<&Path>) -> Result<Vec<PurgeCandidate>>;
pub fn purge_execute(c: &[PurgeCandidate], ctx: &OpCtx) -> Result<Vec<(RunId, Result<()>)>>;
pub fn index_roots() -> Result<Vec<RootInfo>>;  pub fn index_remove(path: &Path) -> Result<bool>;
pub fn profiles() -> Result<Vec<ProfileInfo>>;
pub fn schedule_list() -> Result<Vec<Row>>;  pub fn schedule_add(profile: &str, trigger: &Trigger) -> Result<Added>;
pub fn schedule_remove(profile: &str) -> Result<bool>;
pub fn history(target: &TargetSpec, filter: Option<&str>, last: usize) -> Result<HistoryView>;
pub fn llm_check(cfg: &LlmConfig) -> Result<(), String>;
```
Bei schreibenden Operationen prüft `*_execute` noch einmal selbst die harten Bedingungen (Obergrenze ohne `allow_large`, Wurzel fehlt) und verlässt sich nicht auf das vorherige `*_check`.

### Bibliothek `change::plan`
```rust
impl Plan { pub fn subset(&self, keep: &HashSet<u32>) -> Plan; }
pub fn dependents(plan: &Plan, id: u32) -> Vec<u32>;
```

### Bibliothek `settings`
```rust
pub struct EditableFile { pub path: PathBuf, pub doc: toml_edit::DocumentMut, fingerprint: Fingerprint }
impl EditableFile {
    pub fn load_or_empty(path: &Path) -> Result<Self>;
    pub fn changed_on_disk(&self) -> bool;
    pub fn save(&mut self, validate: &dyn Fn(&str) -> Result<(), ValidationError>, force: bool) -> Result<SaveResult>;
}
pub struct ValidationError { pub message: String, pub line: Option<usize>, pub rule: Option<String>, pub field: Option<String> }
```

## Implementation Sequence

### Grundlage
0. **Spike: egui und Windows-Rahmenbedingungen** (Wegwerf-Branch bzw. `examples/`, Ergebnis als Notiz in diesem Plan)
   - Aktuelle `eframe`/`egui_extras`/`egui_plot` mit minimalen Features; `cargo tree -e normal` gegen die CI-Sperre; Größe der Release-Binary mit `wgpu` (DX12) und mit `glow`
   - Start lokal, in einer Hyper-V-VM ohne GPU und per RDP; Renderer-Fallback testen
   - Tabelle mit 100.000 Zeilen (Scrollen, Sortieren, Filtern); Schriften Segoe UI/Consolas und CJK-Fallback; DPI 100/150/200 %
   - `rfd`-Ordnerdialog (Abhängigkeiten) gegen eigenen `IFileOpenDialog`; `egui_kittest` auf `windows-latest` ohne GPU
   - `toml_edit`-Rundreise einer kommentierten `config.toml`
   - Depends on: nichts
1. **`ops`-Grundgerüst**: `OpCtx`, `TaskProgress`, `Notes`, `ops::Error`; Helfer aus `app/mod.rs` und `app/profile.rs` nach `ops/mod.rs` bzw. `ops/target.rs`; `run_scan` mit Cancel-Parameter; `busy_timeout` in Index und Verlauf
   - Files: `src/ops/mod.rs`, `src/ops/target.rs`, `src/app/mod.rs`, `src/app/profile.rs`, `src/index/mod.rs`, `src/history/mod.rs`, `src/lib.rs`
   - Depends on: nichts (parallel zu 0)
2. **Lesende Operationen**: `ops::scan`, `ops::classify` (Fortschritt über `TaskProgress` statt interner `ProgressBar`), `ops::report_model`/`export_report`, `ops::history`, `ops::profiles`, `ops::index_*`; `app/*` darauf umstellen, Ausgabe bytegleich
   - Files: `src/ops/{scan,classify,report,admin}.rs`, `src/app/{mod,classify,history,profile,snapshot}.rs`
   - Depends on: 1
3. **Plan-Operationen**: `ops::plan` für alle Arten inkl. `rules_text` (Entwurf) und `PlanOut`; `paths::gui_plans_dir`; `[gui]` in der Config; `Plan::subset`, `dependents`, `ops::save_subset`
   - Files: `src/ops/plan.rs`, `src/app/plan.rs`, `src/change/plan.rs`, `src/config.rs`, `src/paths.rs`
   - Depends on: 1
4. **Schreibende Operationen**: `apply_check/execute` (mit Register), `undo_check/execute` (mit Cancel in `UndoEnv`), `purge_candidates/execute`, `runs`; `run` (geplanter Lauf) und `schedule_*` auf `ops` umstellen
   - Files: `src/ops/{apply,runs,admin}.rs`, `src/change/undo.rs`, `src/app/{mod,run,schedule}.rs`
   - Depends on: 1, 3
5. **Regressionsnachweis Refactoring**: `cargo test` (alle `tests/cli_*.rs` unverändert grün), `clippy -D warnings`, manueller Kurztest der CLI (scan, plan dedupe, apply, undo) in einem Testordner; eigener Commit
   - Depends on: 2, 3, 4

### Stufe 1: GUI
6. **GUI-Gerüst**: `[[bin]]`, Abhängigkeiten nach Spike 0, `gui::run` (Renderer-Fallback, Schriften, Panic-Hook, Persistenz), `GuiApp` mit Seitenleiste und leeren Ansichten, `TaskRunner` mit Tests, Schließen-Logik, Statuszeile; `ocr-worker`-Weitergabe; CI-Artefakt
   - Files: `Cargo.toml`, `.github/workflows/ci.yml`, `src/bin/ordner-cleanup-gui.rs`, `src/gui/{mod,app,tasks,texts,fonts}.rs`, `src/gui/widgets/dialogs.rs`
   - Depends on: 0, 1
7. **Shell-Helfer**: `reveal`, `open` (mit Cloud-only-Sperre), Ordner- und Dateidialog
   - Files: `src/platform/shell.rs`, `src/platform/mod.rs`
   - Depends on: 0
8. **Übersicht und Scan/Classify**: Ansicht der Wurzeln und Profile (Score, Trend, letzte Läufe, „Zum Prüfen“, Quarantäne); Ordner- bzw. Profilauswahl; Optionen „Erweitert“; Fortschritt, Abbrechen, Ergebnis mit Hinweisen; Meldung bei belegter Sperre
   - Files: `src/gui/views/{overview,scan}.rs`
   - Depends on: 2, 6, 7
9. **Analyse**: Tabs aus `ReportView`; virtualisierte Tabelle (`widgets/table.rs`) mit Kontextmenü; Größenbaum mit Anteilsbalken; Verlauf mit `egui_plot`; Sprung „Plan dafür erzeugen“; Export mit „im Browser öffnen“
   - Files: `src/gui/views/analysis.rs`, `src/gui/widgets/table.rs`
   - Depends on: 2, 6, 7
10. **Review-Modell** (rein, zuerst als Tests): Zeilen, Filter, Sortierung, Auswahl, Zusammenfassung, Abhängigkeiten, Leistung mit 100.000 Aktionen
    - Files: `src/gui/review.rs`
    - Depends on: 3
11. **Aufräumen und Review-Ansicht**: Plan-Art und Optionen (Defaults aus Config bzw. Profil), Erzeugen im Worker, „Plan öffnen“ (neueste Pläne und Dialog), Review-Tabelle, Filterleiste, Detailbereich, Abhängigkeitsdialog
    - Files: `src/gui/views/{cleanup,review}.rs`
    - Depends on: 9 (Tabelle), 10
12. **Apply und Ergebnis**: Bestätigungsdialog aus `apply_check` (Haken bei Obergrenze, OneDrive- und Index-Alter-Hinweise), Teilplan speichern, `apply_execute` mit Fortschritt und Abbrechen, Ergebnisansicht mit Gründen und „Rückgängig“; danach Analyse neu bauen
    - Files: `src/gui/views/apply.rs`
    - Depends on: 4, 11
13. **Verlauf**: Läufe je Wurzel mit Details aus dem Journal, Undo (Bestätigung, Fortschritt, Konflikte), Purge (Mindestalter, Vorschau, Bestätigung), Index-Wurzeln entfernen; in den Einstellungen in Stufe 1 nur „Datei im Editor öffnen“ und Neuladen
    - Files: `src/gui/views/history.rs`, `src/gui/views/settings/mod.rs`
    - Depends on: 4, 6
14. **Abschluss Stufe 1**: `tests/gui_roundtrip.rs`; ggf. `egui_kittest`-Smoke-Tests; manueller Test (Checkliste unten, Teil A); README-Abschnitt; Roadmap „Stufe 1 umgesetzt“
    - Depends on: 8–13

### Stufe 2: Einstellungen
15. **`settings`-Schicht**: `EditableFile` (Laden, Fingerabdruck, Validierung, `.bak`, atomar schreiben), `FieldSpec` mit Beschreibungen für Config, Unterabschnitte, Profil und Regel; `rules_doc`, `config_doc`; alles mit Unit-Tests (Kommentare bleiben erhalten, ungültige Werte werden nie geschrieben, externe Änderung wird erkannt)
    - Files: `src/settings/{mod,fields,rules_doc,config_doc}.rs`, `Cargo.toml` (`toml_edit`)
    - Depends on: 5
16. **Formular- und Editor-Widgets**: `widgets/form.rs` (alle `FieldKind`, Default-Anzeige, Zurücksetzen, Fehler je Feld), `widgets/toml_editor.rs` (Zeilennummern, Fehlerzeile, entprellte Validierung), Konfliktdialog
    - Depends on: 6, 15
17. **Regel-Editor**: Regelliste (neu, duplizieren, löschen, auf/ab), Formular inkl. Inhaltsbedingungen und Platzhalter-Auswahl (aus `content::FIELD_NAMES` und den Phase-4-Platzhaltern), Vorschau über `ops::plan(Rules { rules_text })` in der Review-Tabelle (nur lesend), Umschalter Roh-TOML (synchronisiert über das Dokument), Speichern mit `RuleSet::parse` als Validator
    - Files: `src/gui/views/settings/rules.rs`
    - Depends on: 11 (Review-Tabelle), 16
18. **Profile und Zeitpläne**: Profilliste, Formular, anlegen, umbenennen, löschen (mit Hinweis auf vorhandenen Zeitplan); Zeitplan je Profil (täglich oder wöchentlich, Uhrzeit, Benachrichtigung) über `ops::schedule_*`; Liste mit nächstem und letztem Lauf
    - Files: `src/gui/views/settings/profiles.rs`
    - Depends on: 4, 16
19. **Config, Klassifikation, Vorlagen**: allgemeine Felder, geschützte Pfade, Ausschlüsse, Junk-Regeln (Liste und Formular), OneDrive, `[gui]`; `[classify]`, `[llm]` mit „Verbindung testen“; Kategorien und Vorlagen als Roh-TOML mit Validator; „Eingebaute Vorlage kopieren“; „Verknüpfung im Startmenü anlegen“
    - Files: `src/gui/views/settings/{config,content,templates}.rs`, `src/platform/shell.rs`
    - Depends on: 16
20. **Abschluss Stufe 2**: manueller Test (Teil B), Test mit einer Person ohne CLI-Kenntnisse, README, Roadmap, Spec-Status `implemented` (nach `verification-before-completion`)
    - Depends on: 14, 17–19

## Test Strategy

- **Unit (Bibliothek)**
  - `Plan::subset`: IDs, `skipped` und Parameter bleiben, `validate()` besteht, leere Auswahl ergibt einen leeren Plan; `dependents` für empty-dirs (Kette Kind → Eltern → Großeltern), archive und für Arten ohne Abhängigkeiten
  - `UndoEnv.cancel`: Abbruch nach N Einträgen → `PartiallyUndone`, zweites Undo setzt fort
  - `ops::*` mit temporären Datenordnern (vorhandenes `test_support`): Hinweise statt Ausgabe (OneDrive, Index-Alter, Obergrenze), `PlanOut` (Cwd, GuiDir, DontSave), `rules_text` wirkt ohne Datei, `apply_execute` verweigert die Obergrenze ohne `allow_large`, `Busy` bei belegter Scan-Sperre
  - `Config` mit `[gui]`; `gui_plans_dir` mit und ohne Override
  - `settings`: Rundreise mit Kommentaren und Reihenfolge; Feld setzen und zurücksetzen; ungültige Dauer, Größe, Regex oder Regel → nichts geschrieben, Fehler mit Zeile bzw. Feld; `.bak` entsteht; externe Änderung erkannt; Profil umbenennen mit ungültigem Namen abgelehnt
- **Unit (GUI-Logik, ohne Fenster)**
  - Review-Modell: Filter und Kombinationen, Sortierung stabil, Auswahl gesammelt, Zusammenfassung, Abhängigkeitsvorschlag, übersprungene nicht auswählbar; Leistung: 100.000 Aktionen filtern/sortieren < 300 ms (Release-Messung als `#[ignore]`-Benchmark, in Debug nur Funktion)
  - Task-Runner: Ergebnis kommt an; Abbruch setzt das Flag; Panic im Worker → `Failed`, Runner bleibt nutzbar; zweiter schreibender Task wird abgelehnt, solange einer läuft
  - Formular-Renderer-Logik (Wert, Default, Zurücksetzen) über `FieldSpec`, ohne egui
- **Integration (`tests/`)**
  - Bestehende `tests/cli_*.rs` unverändert als Regressionsschutz des Refactorings
  - `gui_roundtrip.rs`:
    1. `ops::plan` (dedupe) → `save_subset` → CLI `apply --yes` → CLI `undo --yes`: Ergebnis wie beim reinen CLI-Lauf, Original-Plan bytegleich
    2. CLI `plan junk` → `ops::apply_execute` → `ops::runs` zeigt den Lauf → CLI `undo` funktioniert
    3. Mit `settings` gespeicherte Regel → CLI `plan rules` nutzt sie
    4. `ops::schedule_add` mit Fake-`TaskService` → `schedule list`-Daten identisch zur CLI
- **E2E / Oberfläche**
  - Optional `egui_kittest`-Smoke-Tests (Start, Navigation, Review mit 1.000 Einträgen, Apply-Dialog mit Haken für die Obergrenze), wenn Spike 0 sie auf dem CI-Runner bestätigt
  - Sonst und zusätzlich manuell:
    - **Teil A (Stufe 1):**
      - Start ohne Konsole
      - Scan von 200.000 Dateien mit Abbrechen
      - Fenster schließen während Scan und während Apply
      - Review von 500 Einträgen in unter 5 Minuten
      - 20 Einträge abwählen und anwenden
      - Undo aus GUI und aus CLI
      - Purge
      - parallel laufender geplanter Lauf
      - OneDrive-Ordner mit Obergrenze
      - Cloud-only-Datei
      - DPI 150/200 %
      - VM bzw. RDP
      - `netstat -ano` ohne lauschenden Port
    - **Teil B (Stufe 2):**
      - Regel per Formular anlegen, Vorschau, speichern, per CLI planen
      - Roh-TOML mit Fehler
      - externe Änderung
      - Profil und Zeitplan anlegen, `schedule list`
      - LLM-Test an und aus
      - Vorlage kopieren
      - Verknüpfung im Startmenü
    - **Nutzertest:** Eine Person ohne CLI-Kenntnisse löst die Aufgabe „Duplikate in Ordner X finden, prüfen, in die Quarantäne verschieben und zurückholen“ ohne Hilfe

## Risks & Open Questions

- **Refactoring bricht die CLI** (größtes Risiko, `app/*` ≈ 4.000 Zeilen): Die Ausgabe soll bytegleich bleiben, die vorhandenen CLI-Integrationstests laufen nach jedem Schritt 1–4, und Schritt 5 ist ein eigener Prüfpunkt mit eigenem Commit, bevor GUI-Code entsteht.
- **egui/eframe-API ändert sich zwischen Versionen**: Die Version wird in Schritt 0 festgelegt und exakt gepinnt (`=x.y.z`); GUI-Code nutzt nur verbreitete APIs (`TableBuilder`, `CollapsingHeader`, `Plot`).
- **Render-Backend startet nicht** (VM, RDP, alte Treiber): wgpu/DX12 mit WARP plus automatischer Rückfall auf glow. Scheitern beide, erscheint eine `MessageBoxW` mit Hinweis auf die CLI.
- **CI-Sperre durch transitive Abhängigkeiten** (z. B. `egui_extras` `http`, `image`-Loader): Features werden minimal gesetzt, Spike 0 prüft `cargo tree`, und die CI fängt Rückfälle ab.
- **Binary-Größe** (wgpu kann mehrere MB bringen): Wird in Spike 0 gemessen. Akzeptiert bis ca. 25 MB für die GUI-exe; die CLI-exe bleibt unberührt, weil die GUI-Abhängigkeiten nur im GUI-Teil der Bibliothek genutzt werden. Wird sie zu groß, wird in Schritt 6 entschieden: GUI-Modul hinter einem Feature `gui`, CLI-Build ohne.
- **Bibliothek wird für alle Binaries größer und langsamer zu kompilieren**, solange `src/gui/` ohne Feature-Gate in der Bibliothek liegt: siehe vorheriger Punkt; LTO entfernt ungenutzten Code aus der CLI-exe weitgehend.
- **`SQLITE_BUSY` bei parallelem geplantem Lauf**: `busy_timeout` plus verständliche Meldung. Der Scan selbst ist durch `ScanLock` exklusiv.
- **Regeleditor-Synchronisation Formular ↔ Roh-TOML**: Beide arbeiten auf demselben `toml_edit`-Dokument. Ist der Roh-Text ungültig, ist das Formular gesperrt, bis der Text wieder parst.
- **Lange Pfade und seltene Schriftzeichen**: Fallback-Schriften; zum Kopieren wird immer der Originalpfad verwendet.
- **Undo bekommt Abbruch**: Die Änderung in `undo.rs` ist klein, berührt aber sicherheitsrelevanten Code. Tests für Abbruch an jeder Position, das bestehende Fortsetzen über `PartiallyUndone` bleibt.
- [x] Spike 0 (2026-10-05, lokal, ohne VM/RDP): `eframe`/`egui_extras` `=0.36.2` (ohne Default-Features, mit `accesskit`, `default_fonts`, `persistence`), `egui_plot` 0.37.0, `rfd` 0.17.2, `toml_edit` 0.25. `cargo tree -e normal` zeigt keinen gesperrten HTTP-/TLS-Stack, weder mit `wgpu` noch mit `glow`. Release-exe mit einer Tabelle über 100.000 Zeilen: `wgpu` 12,4 MB, `glow` 6,7 MB (beide unter 25 MB), beide starten lokal. Entscheidung: `wgpu` als Standard mit Rückfall auf `glow`, `rfd` für Dialoge. Offen bleiben VM/RDP, `egui_kittest` auf dem CI-Runner und das Ruckeln bei 100.000 Zeilen (manuell, Teil A).
- [ ] Sollen Pläne aus geplanten Läufen (`plans\<profil>`) in der Übersicht aktiv als „neue Pläne zum Prüfen“ angezeigt werden? Vorschlag: ja, mit Zähler, ohne Benachrichtigung
- [ ] Feature-Gate `gui` für die Bibliothek nur bei Bedarf (siehe Binary-Größe)
