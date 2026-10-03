---
title: "Implementation Plan: Scan & Analyse-Bericht (Phase 1)"
feature_spec: docs/features/scan-analyse-bericht.md
status: approved           # pending-approval | approved | implemented
created: 2026-10-02
updated: 2026-10-02
---

# Implementation Plan: Scan & Analyse-Bericht (Phase 1)

## Entscheidungen (2026-10-02)
- **Toolchain:** MSVC (`x86_64-pc-windows-msvc`) + Visual Studio Build Tools. Die Nutzung ist durch das öffentliche Open-Source-Repo abgedeckt.
- **Installation:** Claude installiert Rust und die Build Tools per `winget` (Schritt 0).
- **Cloud-only-Ordner:** Ordner werden aufgelistet. Dass OneDrive dabei die Dateiliste (nur Metadaten) nachlädt, ist akzeptiert. Es gibt kein `--skip-cloud-dirs`.
- **CSV:** `;`-getrennt, UTF-8 mit BOM, ohne Flag für das Trennzeichen.
- **Umsetzung:** schrittweise mit Checkpoints nach den Meilensteinen Gerüst (Schritt 1), Scan (Schritt 6), Duplikate (Schritt 7) und Bericht (Schritt 10).

## Summary
Neues Rust-Projekt (Cargo, Binary + Library-Crate für Testbarkeit), nur für Windows. `scan` läuft den Baum parallel ab: Die Worker lesen Verzeichnisse über `std::fs::read_dir`, das unter Windows die Find-Daten liefert, ohne Dateien zu öffnen. Ein einzelner Writer-Thread schreibt die Ergebnisse in Batches und Transaktionen in einen SQLite-Index (`rusqlite`, gebündeltes SQLite).

Inkrementalität funktioniert über eine Scan-Generation: Unveränderte Dateien (gleiche Größe und gleiches Änderungsdatum) behalten ihre Hashes, verschwundene Einträge werden am Ende gelöscht.

Danach folgt die Duplikat-Pipeline:
1. Gruppen gleicher Größe bilden (nur lokal verfügbare Dateien mit Modus „voll“)
2. Hardlinks zusammenfassen
3. Teil-Hash bilden
4. Voll-Hash bilden (xxh3-128, parallel)

`report` liest ausschließlich den Index. Daraus wird ein serialisierbares `Report`-Modell gebaut, das vier Ausgaben erzeugt: Terminal-Zusammenfassung, JSON, CSV pro Liste und eine HTML-Datei ohne externe Abhängigkeiten (askama-Template; CSS und JS per `include_str!` eingebettet; Daten als eingebettetes JSON, das vanilla JS rendert).

Alle Analysen (Alter, Probleme, Namensheuristik, Struktur) sind reine Funktionen und werden mit Tabellen-Tests geprüft. Der Zugriff aufs Dateisystem liegt hinter einem kleinen `DirSource`-Trait. So lassen sich auch OneDrive-Platzhalter und Fehlerfälle mit einer Fake-Quelle testen.

## Crate-Auswahl (beantwortet die offene Frage der Spec)
| Zweck | Crate | Begründung |
|---|---|---|
| CLI | `clap` (derive) | Standard, gute Hilfe-Texte |
| Parallelität | `rayon`, `crossbeam-channel` | Work-Stealing für Walker und Hashing; Kanal zum Writer-Thread |
| Index | `rusqlite` (Feature `bundled`) | SQLite in der `.exe` enthalten, keine Runtime-Abhängigkeit |
| Hash | `xxhash-rust` (Feature `xxh3`) | Pure Rust, sehr schnell, 128 Bit reichen für Duplikaterkennung |
| Windows-API | `windows-sys` (minimal: `Win32_Storage_FileSystem`, `Win32_Foundation`) | `GetDriveTypeW`, `GetFileInformationByHandle` (File-ID, Link-Anzahl) |
| Glob-Ausschlüsse | `globset` | Schnell, case-insensitiv konfigurierbar |
| Namensheuristik | `regex` | Normalisierungsregeln |
| Konfiguration | `serde`, `toml`, `dirs` | `config.toml`, Standard-Pfade `%APPDATA%`/`%LOCALAPPDATA%` |
| Berichte | `serde_json`, `csv`, `askama` | Typsicheres HTML-Template zur Compile-Zeit |
| Zeit | `chrono` | Altersklassen, Zeitstempel |
| Fortschritt/Terminal | `indicatif`, `bytesize` | Fortschrittsanzeige, lesbare Größen |
| Abbruch | `ctrlc` | Setzt ein `AtomicBool`, Writer schließt die Transaktion sauber ab |
| Fehler | `anyhow` (bin), `thiserror` (lib) | |
| Tests | `tempfile`, `assert_cmd`, `predicates`, `rstest` | Integrationstests mit echten Temp-Verzeichnissen |

Ein Netzwerk-Crate (`reqwest`, `hyper`, `ureq` …) ist nicht erlaubt. Die CI prüft das per `cargo tree`.

## Files to Create
| File | Purpose |
|------|---------|
| `Cargo.toml` | Paket `ordner-cleanup`, `[lib]` + `[[bin]]`, Abhängigkeiten, Release-Profil (`lto`, `codegen-units=1`, `strip`) |
| `rust-toolchain.toml` | Feste stabile Toolchain, Target `x86_64-pc-windows-msvc`, Komponenten `clippy`, `rustfmt` |
| `.cargo/config.toml` | `target-feature=+crt-static`, damit die `.exe` ohne VC-Redistributable läuft |
| `src/main.rs` | Einstieg: CLI parsen, an `app::run` delegieren, Exit-Codes 0/1/2 setzen |
| `src/lib.rs` | Modul-Wurzel |
| `src/cli.rs` | `clap`-Definitionen: `scan`, `report`, `index list`, `index remove` |
| `src/app.rs` | Orchestrierung der Befehle (Config laden, Index öffnen, Scan/Report ausführen) |
| `src/config.rs` | `Config`-Struct (TOML), Default-Ausschlüsse, Zusammenführen mit CLI-Flags, Schwellwerte |
| `src/paths.rs` | Pfad-Normalisierung, case-insensitiver `path_key`, Präfix-Bereiche, Anzeige mit Ersatzzeichen, App-Verzeichnisse (Override per Env `ORDNER_CLEANUP_HOME` für Tests) |
| `src/platform/mod.rs` | Plattform-Abstraktion: `FileAttrs` (Bitflags-Wrapper), `is_cloud_only()`, `is_hidden()`, `is_system()` |
| `src/platform/windows.rs` | `drive_kind(path)` (lokal/remote/UNC), `file_identity(path)` → (Volume-Serial, File-Index, Link-Anzahl), Öffnen nur mit `FILE_READ_ATTRIBUTES` |
| `src/scan/mod.rs` | `Scanner`: Ablauf Walk → Index-Upsert → Prune → Duplikat-Pipeline; Fortschritt; Abbruch |
| `src/scan/source.rs` | Trait `DirSource { read_dir(&Path) -> io::Result<Vec<RawEntry>> }`; `StdDirSource` (echt) und `FakeDirSource` (nur `#[cfg(test)]` bzw. Feature `test-support`) |
| `src/scan/classify.rs` | Pro Verzeichnis entscheiden: `Exclude` / `SummaryOnly` / `Full` (Globset, Sonderregel `bin`/`obj` nur bei `*.csproj`/`*.sln` im Elternordner, explizite Wurzel hat Vorrang) |
| `src/scan/walker.rs` | Paralleler Walker (rayon scope): Links werden nicht betreten, Summary-Teilbäume nur aufsummiert, Fehler werden gesammelt; sendet `ScanEvent`s an den Writer |
| `src/scan/hasher.rs` | Teil-Hash (Größe + erste/letzte 64 KiB), Voll-Hash (Streaming, 1 MiB Puffer), Öffnen mit Share-Mode Read/Write/Delete + `FILE_FLAG_SEQUENTIAL_SCAN`; prüft vor und nach dem Hashen Größe und Änderungsdatum |
| `src/scan/duplicates.rs` | Kandidatenwahl, Hardlink-Zusammenfassung, Stufen-Pipeline, Ergebnisse in den Index |
| `src/index/mod.rs` | `Index`: Öffnen, Pragmas (WAL, `synchronous=NORMAL`), Schema-Versionierung/Migration, `--reset-index` |
| `src/index/schema.sql` | DDL (siehe Data Model) |
| `src/index/store.rs` | Batch-Upsert, Laden des Vorzustands einer Wurzel, Prune nach Generation, Abfragen für den Report |
| `src/analysis/mod.rs` | Gemeinsame Typen |
| `src/analysis/age.rs` | Altersklassen, Parser für `--old-after` (`1y`, `18m`, `90d`) |
| `src/analysis/filetypes.rs` | Endung → Kategorie (Dokumente, Bilder, Video, Audio, Archive, Code, Ausführbar, Sonstige) |
| `src/analysis/problems.rs` | Reine Prüfungen: lange Pfade, ungültige Zeichen/Namen, 0 Byte, `~$`, Temp-Dateien, abgebrochene Downloads, Systemmüll, OneDrive-Konflikte |
| `src/analysis/similar.rs` | `normalize_name()` + Gruppierung pro Ordner |
| `src/analysis/structure.rs` | Leere Ordner, Ein-Datei-Ordner, Tiefe, Riesenordner |
| `src/report/mod.rs` | `Report`-Modell (`Serialize`) aus Index-Abfragen + Analysen aufbauen |
| `src/report/terminal.rs` | Kompakte Zusammenfassung |
| `src/report/json.rs` | `report.json` |
| `src/report/csv.rs` | Je eine CSV-Datei pro Liste (UTF-8 mit BOM, `;` als Trennzeichen für deutsches Excel) |
| `src/report/html.rs` | askama-Rendering; Größenbaum-Daten als eingebettetes JSON |
| `templates/report.html` | HTML-Template (Abschnitte laut Spec, Hinweis zur Vertraulichkeit) |
| `assets/report.css`, `assets/report.js` | Sortier-/Filterbare Tabellen, lazy aufklappbarer Größenbaum, Hell/Dunkel; ohne CDN, eingebettet |
| `tests/common/mod.rs` | Helfer: Testbaum erzeugen, Snapshot des Baums (Pfade, Größen, mtimes, Attribute, Inhalts-Hash) |
| `tests/cli_scan_report.rs` | End-to-End: `scan` + `report`, alle Formate, Exit-Codes, Fehlermeldungen |
| `tests/readonly.rs` | Vorher/Nachher-Snapshot ist identisch |
| `tests/incremental.rs` | Re-Scan ohne Hashing, Änderungen/Löschungen/Neuzugänge, Abbruch und Wiederaufnahme |
| `tests/windows_specifics.rs` | Lange Pfade, versteckte/System-Dateien, Junctions (`mklink /J`), Hardlinks, Zugriff verweigert (ACL per `icacls`) |
| `.github/workflows/ci.yml` | `windows-latest`: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, Netzwerk-Crate-Check, Release-Build als Artefakt |

## Files to Modify
| File | Change Description |
|------|--------------------|
| `.gitignore` | `/target` ergänzen |
| `README.md` | Kurzbeschreibung, Installation/Build, Beispielaufrufe `scan`/`report`, Hinweis „rein lesend“ |
| `docs/roadmap.md` | Feature-Status aktualisieren |
| `docs/features/scan-analyse-bericht.md` | Am Ende `status: implemented` |

## Data Model Changes

### SQLite-Index (`index.db`, Schema-Version 1)
```sql
CREATE TABLE meta   (key TEXT PRIMARY KEY, value TEXT NOT NULL);       -- schema_version
CREATE TABLE roots  (id INTEGER PRIMARY KEY, path TEXT NOT NULL, path_key TEXT NOT NULL UNIQUE,
                     generation INTEGER NOT NULL, started_at TEXT, finished_at TEXT,
                     status TEXT NOT NULL,            -- running | complete | aborted
                     error_count INTEGER NOT NULL DEFAULT 0);
CREATE TABLE dirs   (id INTEGER PRIMARY KEY, path TEXT NOT NULL, path_key TEXT NOT NULL UNIQUE,
                     parent_key TEXT, depth INTEGER NOT NULL,
                     mode TEXT NOT NULL,             -- full | summary
                     attrs INTEGER NOT NULL, mtime INTEGER, is_link INTEGER NOT NULL,
                     link_target TEXT,
                     direct_entries INTEGER NOT NULL, -- direkte Kinder (Dateien + Ordner)
                     summary_size INTEGER, summary_files INTEGER,  -- nur bei mode=summary
                     generation INTEGER NOT NULL);
CREATE TABLE files  (id INTEGER PRIMARY KEY, dir_key TEXT NOT NULL, path TEXT NOT NULL,
                     path_key TEXT NOT NULL UNIQUE, name TEXT NOT NULL, ext TEXT,
                     size INTEGER NOT NULL, mtime INTEGER NOT NULL, ctime INTEGER,
                     attrs INTEGER NOT NULL, cloud_only INTEGER NOT NULL, is_link INTEGER NOT NULL,
                     link_target TEXT,
                     volume_serial INTEGER, file_index INTEGER, nlinks INTEGER,  -- nur für Kandidaten
                     partial_hash BLOB, full_hash BLOB,
                     hash_status TEXT,               -- NULL | ok | locked | changed | error
                     generation INTEGER NOT NULL);
CREATE TABLE errors (id INTEGER PRIMARY KEY, root_key TEXT NOT NULL, path TEXT NOT NULL,
                     kind TEXT NOT NULL, message TEXT NOT NULL, generation INTEGER NOT NULL);
CREATE INDEX files_size   ON files(size) WHERE cloud_only = 0 AND is_link = 0;
CREATE INDEX files_dir    ON files(dir_key);
CREATE INDEX files_hash   ON files(full_hash) WHERE full_hash IS NOT NULL;
CREATE INDEX dirs_parent  ON dirs(parent_key);
```
- **`path_key`** ist der absolute Pfad, normalisiert: Laufwerksbuchstabe groß, `\` als Trenner, Unicode-Simple-Case-Folding in Kleinbuchstaben. Bei Ordnern endet er auf `\`. Für Unterbäume wird per Präfix-Bereich abgefragt: `path_key >= :p AND path_key < :p || char(0x10FFFF)`.
- **Verschachtelte Wurzeln:** Einträge werden pro `path_key` genau einmal gespeichert, unabhängig von der Wurzel. Die Tabelle `roots` hält nur fest, welche Wurzeln wann gescannt wurden. Ein Report einer Wurzel liest deren Präfix-Bereich.
- **Inkrementell:** Zu Beginn wird für die Wurzel `path_key → (size, mtime, partial_hash, full_hash)` in eine HashMap geladen. Ist eine Datei unverändert, werden die Hashes übernommen, sonst auf `NULL` gesetzt. Alle Einträge bekommen die neue `generation`. Nach einem vollständigen Lauf werden Einträge im Präfix-Bereich mit älterer Generation gelöscht (Prune). Nach einem Abbruch gibt es kein Prune, die Wurzel steht auf `status = aborted`.
- Pfade werden als UTF-8 gespeichert. Nicht darstellbare UTF-16-Sequenzen werden verlustfrei als WTF-8-BLOB in einer zusätzlichen Spalte `path_raw` gespeichert (nur gefüllt, wenn nötig).

### Kern-Typen (Rust)
```rust
pub struct RawEntry { name: OsString, kind: EntryKind /* File | Dir | Link{dir: bool} */,
                      size: u64, mtime: i64, ctime: i64, attrs: FileAttrs }
pub enum DirMode { Exclude, SummaryOnly, Full }
pub enum ScanEvent { Dir(DirRecord), Files(Vec<FileRecord>), Summary(SummaryRecord), Error(ScanError) }
pub struct Report { overview, size_tree, top_files, top_dirs, file_types, age, duplicates,
                    probable_duplicates, similar, structure, problems, errors }   // Serialize
```
- **`FileAttrs::is_cloud_only()`:** `RECALL_ON_DATA_ACCESS (0x400000) | RECALL_ON_OPEN (0x40000) | OFFLINE (0x1000)`
- **Link-Erkennung:** über `std`s `FileType::is_symlink()`. Das ist unter Windows nur bei Name-Surrogate-Reparse-Tags wahr, also bei Symlinks und Junctions, aber **nicht** bei OneDrive-Cloud-Tags. Ein Test sichert das ab (siehe Risiken).

## API / Interface Changes
Eine öffentliche API gibt es nicht. Die Schnittstelle zum Nutzer ist die CLI laut Spec:
```
ordner-cleanup scan <pfad> [--exclude <glob>]... [--summary-only <glob>]... [--no-default-excludes]
                           [--force] [--reset-index] [--threads <n>]
ordner-cleanup report [<pfad>] [--out <ordner>] [--format html,json,csv] [--old-after <dauer>] [--top <n>]
ordner-cleanup index list
ordner-cleanup index remove <pfad>
```
**`config.toml`** (alle Felder optional):
```toml
exclude = []                  # zusätzliche Globs
summary_only = []
no_default_excludes = false
old_after = "1y"
top = 50
max_depth_warning = 8
huge_dir_entries = 1000
onedrive_conflict_hostnames = []   # zusätzlich zu %COMPUTERNAME%
threads = 0                         # 0 = automatisch
```
**Reihenfolge der Konfigurationsquellen:** CLI-Flags vor `config.toml` vor eingebauten Defaults.

**Exit-Codes:** `0` OK, `1` Fehler, `2` OK mit Teilfehlern.

## Implementation Sequence
Umsetzung testgetrieben (Skill `test-driven-development`): pro Schritt zuerst die Tests, dann der Code. Nach jedem Schritt `cargo fmt`, `cargo clippy -- -D warnings` und `cargo test`, dann ein Commit auf `dev`.

0. **Toolchain bereitstellen** (einmalig, auf dem Rechner)
   - Visual Studio Build Tools mit Workload „Desktopentwicklung mit C++“ (MSVC-Linker + Windows SDK). Wird für das MSVC-Target und das gebündelte SQLite (C-Code) gebraucht. Installation: `winget install Microsoft.VisualStudio.2022.BuildTools` mit Workload `Microsoft.VisualStudio.Workload.VCTools`.
   - Rust: `winget install Rustlang.Rustup`, danach `rustup default stable`
   - Depends on: nichts
1. **Projektgerüst + CI**
   - `cargo init`, `Cargo.toml`, `rust-toolchain.toml`, `.cargo/config.toml`, `.gitignore`, leere Module, `cli.rs` mit allen Befehlen (die Befehle tun noch nichts), `ci.yml`
   - Test: `--help` und `--version` laufen
   - Depends on: 0
2. **Pfade + Plattform-Attribute**
   - `paths.rs` (`path_key`, Präfix-Bereiche, App-Verzeichnisse), `platform/mod.rs` (`FileAttrs`), `platform/windows.rs` (`drive_kind`, `file_identity`)
   - Tests: Normalisierung (Groß-/Kleinschreibung, Umlaute, `\\?\`-Präfix entfernen, UNC), Attribut-Klassifikation inklusive Cloud-Flags
   - Depends on: 1
3. **Konfiguration + Klassifikation**
   - `config.rs`, `scan/classify.rs`
   - Default-Ausschlüsse werden aus Umgebungsvariablen aufgebaut: `SystemRoot`, `ProgramFiles`, `ProgramFiles(x86)`, `ProgramData`, `USERPROFILE\AppData`, `LOCALAPPDATA\ordner-cleanup`
   - Tests: Merge-Reihenfolge; `Exclude`/`SummaryOnly`/`Full` für alle Default-Muster; `bin`/`obj`-Sonderregel; explizite Wurzel hat Vorrang; `--no-default-excludes`
   - Depends on: 2
4. **Index**
   - `index/` mit Schema, Versionierung, Batch-Upsert, Laden des Vorzustands, Prune, `index list/remove`, `--reset-index`
   - Tests (In-Memory-SQLite): Upsert und Generation, Prune nur im Präfix, verschachtelte Wurzeln ohne Doppelungen, Schema-Mismatch wird erkannt
   - Depends on: 2
5. **DirSource + Walker**
   - `scan/source.rs`, `scan/walker.rs`, Writer-Thread in `scan/mod.rs`, Fortschritt, Ctrl+C
   - Tests mit `FakeDirSource`:
     - Cloud-Platzhalter werden erfasst und nie geöffnet (die Fake-Quelle protokolliert Zugriffe)
     - Links werden nicht betreten
     - Summary-Teilbaum wird korrekt aufsummiert
     - Zugriff verweigert führt zu einem Fehler-Eintrag, der Scan läuft weiter
     - Datei verschwindet währenddessen
   - Integrationstests mit echtem Temp-Baum: Anzahl und Größe stimmen
   - Depends on: 3, 4
6. **Inkrementeller Scan**
   - Hashes übernehmen bzw. verwerfen, Prune, Status `aborted`
   - Tests: `tests/incremental.rs`
   - Depends on: 5
7. **Hasher + Duplikat-Pipeline**
   - `scan/hasher.rs`, `scan/duplicates.rs`
   - Ist eine Datei ≤ 128 KiB, deckt der Teil-Hash sie vollständig ab, und Voll-Hash = Teil-Hash
   - Prüfung vor und nach dem Hashen auf Änderungen
   - Hardlinks über `file_identity` nur für Kandidaten ermitteln: Hardlinks haben immer dieselbe Größe und landen deshalb zwangsläufig in derselben Größengruppe
   - Tests:
     - Gleiche Größe mit anderem Inhalt ergibt kein Duplikat
     - Eindeutige Größen werden nicht gehasht (Zähler)
     - Gesperrte Datei führt zu `hash_status = locked`
     - Hardlinks werden einmal gezählt und nicht als Duplikat gemeldet
     - Datei > 4 GB (sparse, per `fsutil` oder `set_len`)
   - Depends on: 6
8. **Analysen**
   - `analysis/age.rs`, `filetypes.rs`, `problems.rs`, `similar.rs`, `structure.rs`, alles reine Funktionen
   - Tabellen-Tests mit `rstest`: alle Problemmuster der Spec, reservierte Namen, Konfliktmuster mit verschiedenen Rechnernamen, Normalisierungsbeispiele (`Angebot.docx`, `Angebot_v2.docx`, `Angebot - Kopie (3).docx`, `2024-03-01 Angebot final.docx`), Grenzen der Altersklassen, `--old-after`-Parser
   - Depends on: 2 (unabhängig von 4–7, kann parallel laufen)
9. **Report-Modell + Terminal + JSON + CSV**
   - `report/mod.rs`, `terminal.rs`, `json.rs`, `csv.rs`
   - Tests: Modell aus einem Fixture-Index; JSON enthält alle Abschnitte; eine CSV pro Liste; wahrscheinliche Duplikate (Cloud) als „nicht verifiziert“ markiert; Fehlerfälle `report` ohne Scan, mehrere Wurzeln ohne Auswahl, Zielordner nicht beschreibbar
   - Depends on: 7, 8
10. **HTML-Bericht**
    - `report/html.rs`, `templates/report.html`, `assets/*` (Skill `frontend-design` für die Gestaltung)
    - Tests:
      - Die Datei enthält keine `http(s)://`-, `src=`- oder `href=`-Referenzen nach außen
      - Alle Abschnitte sind vorhanden
      - Pfade mit `<`, `&` und Anführungszeichen werden korrekt escaped (inklusive des eingebetteten JSON: `</script>` neutralisieren)
    - Manuelle Sichtprüfung im Browser
    - Depends on: 9
11. **CLI-Verdrahtung, Exit-Codes, Warnungen**
    - `app.rs`, `main.rs`: UNC- und Netzlaufwerk-Warnung mit `--force`, Hinweis bei Wurzel im Ausschlussbereich, Exit-Codes 0/1/2
    - Tests: `tests/cli_scan_report.rs`, `tests/readonly.rs`, `tests/windows_specifics.rs`
    - Depends on: 10
12. **Performance-Verifikation + Feinschliff**
    - Generator für einen Testbaum mit 100k Dateien (als `#[ignore]`-Test bzw. `cargo run --example gen-tree`)
    - Erstscan < 2 min, Re-Scan < 15 s, RAM < 500 MB messen und im README dokumentieren; bei Bedarf Batch-Größen, Thread-Zahl und Pragmas tunen
    - Manueller Testlauf auf dem echten `Dokumente`-Ordner (OneDrive) und Prüfung, dass Cloud-Dateien cloud-only bleiben
    - README, Roadmap und Spec-Status aktualisieren (`verification-before-completion`)
    - Depends on: 11

## Test Strategy
- **Unit:**
  - Reine Funktionen (Pfade, Attribute, Klassifikation, Altersklassen, Problemprüfungen, Namensheuristik, Struktur, Dateitypen) als Tabellen-Tests
  - Index mit In-Memory-SQLite
  - Walker mit `FakeDirSource`; damit lassen sich Cloud-Platzhalter, Fehler und Rennbedingungen deterministisch simulieren
- **Integration** (echte Temp-Verzeichnisse unter Windows):
  - Scan und Report über die Library-API
  - Inkrementelle Läufe
  - Duplikate und Hardlinks
  - Junctions (`mklink /J`), lange Pfade (> 260), versteckte/System-Attribute, verweigerter Zugriff (`icacls /deny`)
  - Gesperrte Datei (Datei ohne Share-Read im Test offen halten)
  - **Read-only-Nachweis:** Snapshot des gesamten Baums (Pfade, Größen, mtimes, ctimes, Attribute, Inhalts-Hash) vor und nach `scan` ist identisch
- **E2E** (`assert_cmd`):
  - Die Binary mit `scan` und danach `report --format html,json,csv`
  - Exit-Codes, Fehlermeldungen
  - HTML ohne externe Referenzen
  - `ORDNER_CLEANUP_HOME` isoliert Index und Config pro Test
- **Manuell:**
  - Echter OneDrive-Ordner mit Cloud-only-Dateien. Vorher und nachher wird der Status im Explorer bzw. per `attrib` geprüft, und der Netzwerk-Monitor sollte keine Dateidownloads zeigen.
  - HTML-Bericht im Browser
- **CI:** `windows-latest`, `fmt`, `clippy -D warnings`, `test`, Check gegen Netzwerk-Crates (`cargo tree -e normal | findstr` auf `reqwest|hyper|ureq|curl` muss leer sein), Release-Build als Artefakt

## Risks & Open Questions
- **Risiko: Ein Cloud-Platzhalter wird versehentlich hydriert.**
  - Mitigation: Dateien werden nur zum Hashen geöffnet, und die Kandidaten-Abfrage schließt `cloud_only = 1` aus.
  - `file_identity` öffnet nur mit `FILE_READ_ATTRIBUTES`, das löst keinen Daten-Recall aus.
  - Vor dem Öffnen werden die Attribute ein zweites Mal geprüft, falls sich der Status seit dem Walk geändert hat.
  - Ein Fake-Test stellt sicher, dass keine Platzhalter geöffnet werden, und der manuelle Test auf echtem OneDrive bestätigt es.
- **Risiko: `std` liefert bei Reparse-Points falsche Link-Infos.**
  - Die Annahme ist, dass `DirEntry::file_type()` und `DirEntry::metadata()` unter Windows aus den Find-Daten kommen, ohne die Datei zu öffnen, und Cloud-Tags nicht als Symlink melden.
  - Mitigation: Ein früher Spike in Schritt 5 auf echtem OneDrive. Falls die Annahme nicht hält, wird `FindFirstFileExW` direkt über `windows-sys` in `StdDirSource` gelesen. Der Trait macht diesen Austausch lokal.
- **Verzeichnisse, die nur in der Cloud liegen:** Beim Auflisten lädt der OneDrive-Client die *Dateiliste* nach (nur Metadaten, keine Inhalte). Das ist entschieden und akzeptiert.
- **Risiko: Performance bei großen Bäumen bzw. SQLite-Writer als Engpass.**
  - Mitigation: Batches von etwa 5.000 Zeilen pro Transaktion, WAL, Prepared Statements, ein Writer-Thread. Messung in Schritt 12.
- **Risiko: Der Last-Access-Zeitstempel ändert sich durchs Hashen.**
  - Die Spec nimmt das ausdrücklich aus. Unter Windows ist das Aktualisieren standardmäßig weitgehend deaktiviert, das wird im README erwähnt.
- **Risiko: HTML wird bei sehr vielen Duplikatgruppen groß.**
  - Mitigation: Der Größenbaum bettet nur Ordner-Aggregate ein, keine Einzeldateien. Listen werden lazy gerendert. Vollständige Daten stehen immer in JSON und CSV.
- [x] Lizenz der Visual Studio Build Tools: abgedeckt (öffentliches Open-Source-Repo), MSVC-Toolchain gesetzt
- [ ] Muster für OneDrive-Konfliktdateien gegen echte Beispiele aus dem eigenen OneDrive prüfen (Schritt 12)
- [ ] Schwellwerte (Tiefe 8, 1000 Einträge, Top 50) nach dem ersten echten Scan ggf. anpassen
- [x] CSV-Format: `;`-getrennt mit BOM (deutsches Excel), ohne Flag
