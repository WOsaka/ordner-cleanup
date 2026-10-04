# ordner-cleanup

Analyse und sicheres Aufräumen von Ordnersystemen unter Windows 10/11. `scan` und `report` (Phase 1) sind rein lesend: Größenbaum, Dateitypen, Alter, exakte Duplikate, ähnliche Dateien, Strukturprobleme und Problemdateien als HTML, JSON und CSV. Phase 2 (`plan`, `apply`, `undo`, `runs`, `purge`) verschiebt exakte Duplikate nach einem prüfbaren Plan und einer Bestätigung in eine Quarantäne und kann jeden Lauf zurückdrehen. Phase 3 ergänzt die Aufräumaktionen `plan junk`, `empty-dirs`, `archive` und `versions` auf demselben Fundament. Es gibt keine Netzwerkzugriffe und keine Telemetrie.

Ausführliche Gesamtdokumentation (Einrichtung, Bedienung, alle Features): [docs/dokumentation.md](docs/dokumentation.md).

Spec Phase 1: [docs/features/scan-analyse-bericht.md](docs/features/scan-analyse-bericht.md), Plan: [docs/implementation-plans/scan-analyse-bericht.md](docs/implementation-plans/scan-analyse-bericht.md). Spec Phase 2: [docs/features/aenderungsplan-apply-undo.md](docs/features/aenderungsplan-apply-undo.md), Plan: [docs/implementation-plans/aenderungsplan-apply-undo.md](docs/implementation-plans/aenderungsplan-apply-undo.md). Spec Phase 3: [docs/features/aufraeumaktionen.md](docs/features/aufraeumaktionen.md), Plan: [docs/implementation-plans/aufraeumaktionen.md](docs/implementation-plans/aufraeumaktionen.md). Roadmap: [docs/roadmap.md](docs/roadmap.md).

## Verwendung

```
ordner-cleanup scan <pfad> [--exclude <glob>]... [--summary-only <glob>]... [--no-default-excludes] [--force] [--reset-index] [--threads <n>]
ordner-cleanup report [<pfad>] [--out <ordner>] [--format html,json,csv] [--old-after 1y] [--top 50]
ordner-cleanup index list
ordner-cleanup index remove <pfad>
```

- `scan` schreibt einen SQLite-Index nach `%LOCALAPPDATA%\ordner-cleanup\index.db`. Ein erneuter Scan übernimmt Hashes unveränderter Dateien und entfernt gelöschte Einträge.
- `report` erzeugt aus dem Index den Ordner `ordner-cleanup-report-<zeitstempel>` mit den gewünschten Formaten und zeigt eine Zusammenfassung im Terminal.
- Konfiguration (optional): `%APPDATA%\ordner-cleanup\config.toml`; CLI-Flags haben Vorrang.
- Netzlaufwerke und UNC-Pfade werden nur mit `--force` gescannt (in Phase 1 nicht unterstützt).
- OneDrive-Cloud-Platzhalter werden nie geöffnet: Sie erscheinen nur mit Metadaten und höchstens als „wahrscheinliche Duplikate“.
- Exit-Codes: `0` OK, `1` Fehler oder Abbruch, `2` OK mit Teilfehlern (z. B. Zugriff verweigert, gesperrte Dateien).
- Der letzte Zugriffszeitstempel (Last Access) kann sich durch das Hashen ändern. Windows aktualisiert ihn standardmäßig kaum.

## Aufräumen: plan, apply, undo (Phase 2)

```
ordner-cleanup plan dedupe <pfad> [--keep oldest|newest|path:<absoluter ordner>] [--out <plan.json>]
ordner-cleanup apply <plan.json> [--yes] [--allow-large]
ordner-cleanup undo <run-id> [--root <wurzel>] [--yes]
ordner-cleanup runs [<wurzel>]
ordner-cleanup purge [--older-than 30d] [--root <wurzel>] [--yes]
```

Ablauf: Ordner mit `scan` indizieren, mit `plan dedupe` einen Plan erzeugen (die Plan-Datei ist lesbares JSON, ohne `--out` landet sie als `plan-<zeitstempel>.json` im aktuellen Ordner), den Plan prüfen, mit `apply` ausführen, bei Bedarf mit `undo <run-id>` zurückdrehen. `plan` verändert nichts. Pro Duplikatgruppe bleibt immer mindestens eine Datei unberührt.

- **Nichts wird hart gelöscht** (einzige Ausnahme: leere Ordner bei `plan empty-dirs`, siehe Phase 3; auch sie lassen sich per `undo` wiederherstellen). Verschoben wird per Umbenennen auf demselben Volume nach `<wurzel>\.ordner-cleanup\quarantine\<run-id>\<relativer Pfad>`; Zeitstempel und Inhalt bleiben erhalten. Einen Verschiebevorgang über Laufwerksgrenzen gibt es nicht. Ein belegtes Ziel wird nie überschrieben, es bekommt ein Suffix wie `datei (2).txt`. Hart gelöscht wird nur mit `purge` nach Bestätigung, nie automatisch.
- **Bestätigung:** `apply`, `undo` und `purge` fragen einmal `j/N`. In einer nicht interaktiven Sitzung (Skript, Pipe) brechen sie ohne `--yes` ab, statt zu raten.
- **Prüfung vor jeder Aktion:** Größe, Änderungszeit und Hash der Datei und der behaltenen Kopie müssen noch zum Plan passen, sonst wird nur diese Aktion als `stale` übersprungen und der Rest läuft weiter. Fehlt die behaltene Datei, passiert in der Gruppe nichts.
- **Geschützte Pfade** werden nie angefasst: `C:\Windows`, `Program Files`, `Program Files (x86)` und `ProgramData` immer und nicht abschaltbar; `AppData`, `.git`, `node_modules` und Projektordner mit `Cargo.toml`, `package.json`, `*.sln` oder `*.csproj` standardmäßig; weitere über `protected_paths` in der Config. `apply` prüft das unabhängig vom Plan noch einmal (manipulierte Pläne, `..`, `\\?\`, 8.3-Kurznamen). Liegt die angegebene Wurzel selbst unter `AppData`, gilt nur diese eine Regel nicht.
- **OneDrive:** Cloud-Platzhalter und Links (Symlinks/Junctions) werden nie gelesen, gehasht oder verschoben. Liegt die Wurzel in OneDrive, wird die Quarantäne mitsynchronisiert; `plan` und `apply` warnen davor.
- **Journal:** Jeder Lauf schreibt ein Write-ahead-Journal (`<wurzel>\.ordner-cleanup\journal\<run-id>.jsonl`, `fsync` je Eintrag) vor dem Verschieben. `undo` prüft den echten Dateizustand, überschreibt nie (Kollisionen werden gemeldet, die Datei bleibt in der Quarantäne) und funktioniert auch nach einem Absturz. Ein zweiter `apply` desselben Plans ändert nichts.
- **Lauf-Register:** Damit `undo <run-id>` die Wurzel findet, vermerkt `apply` jeden Lauf in `%LOCALAPPDATA%\ordner-cleanup\runs.jsonl`. Maßgeblich bleibt das Journal; fehlt das Register, hilft `--root`.
- **Config** (`%APPDATA%\ordner-cleanup\config.toml`): `protected_paths = ["D:\\Wichtig"]` und `quarantine_days = 30` (Aufbewahrungsdauer; `purge` löscht nur Läufe, die älter sind).
- **Exit-Codes:** `0` alles erledigt, `1` Fehler oder Abbruch (auch „N“ bei der Rückfrage), `2` Teilerfolg (stale, übersprungen, Fehler einzelner Aktionen, Kollisionen beim `undo`).
- Der Ordner `.ordner-cleanup` wird von `scan` nie erfasst.

## Aufräumaktionen (Phase 3)

```
ordner-cleanup plan junk <pfad> [--category system,temp,downloads,installer,<eigene>] [--out <plan.json>]
ordner-cleanup plan empty-dirs <pfad> [--out <plan.json>]
ordner-cleanup plan archive <pfad> [--older-than 2y] [--out <plan.json>]
ordner-cleanup plan versions <pfad> [--min-age 30d] [--out <plan.json>]
ordner-cleanup apply <plan.json> [--yes] [--allow-large]
```

Jede Aktion erzeugt ihren eigenen Plan (Dry-Run, verändert nichts) und läuft über dasselbe `apply`, `undo` und `runs` wie in Phase 2: Stale-Prüfung je Aktion, Schutzregeln, Cloud-Platzhalter, Write-ahead-Journal, Idempotenz. Die Pläne tragen `version: 2`; Pläne mit `version: 1` aus Phase 2 und ihre Journale funktionieren unverändert.

| Befehl | Was geplant wird | Aktionstyp | Undo |
|---|---|---|---|
| `plan junk` | Müll nach Kategorien (siehe unten) | `quarantine` (ohne Hash) | Datei zurück aus der Quarantäne |
| `plan empty-dirs` | rekursiv leere Ordner, von unten nach oben | `remove-dir` | Ordner werden von oben nach unten neu angelegt, Attribute und Zeitstempel zurückgesetzt |
| `plan archive` | Ordner, deren jüngste Datei älter als `--older-than` ist, nach `<wurzel>\_Archiv\<Jahr>\<relativer Pfad>` | `move` (ganzer Ordner) | Rückbenennung |
| `plan versions` | pro Versionsgruppe bleibt die jüngste Datei, ältere (mindestens `--min-age` alt) nach `<wurzel>\_Archiv\Versionen\<relativer Pfad>` | `move` (Datei) | Rückbenennung |

Empfohlene Reihenfolge für einen Komplettputz: `plan junk` → `apply` → `scan` → `plan empty-dirs` → `apply`. Ein Ordner, in dem nur noch Müll liegt (z. B. `Thumbs.db`), gilt für `empty-dirs` nicht als leer; `plan empty-dirs` weist darauf hin.

- **junk:** Kategorien `system` (`Thumbs.db`, `ehthumbs.db`, `.DS_Store`), `temp` (`*.tmp`, `~$*`), `downloads` (`*.crdownload`, `*.part`, `*.partial`) und `installer` (`*.exe`, `*.msi` direkt im Downloads-Ordner, älter als `installer_min_age`). Den Downloads-Ordner liefert Windows über die Known Folder API (auch umgeleitet); `downloads_dirs` in der Config ersetzt ihn. Liegt kein Downloads-Ordner unter der Wurzel, liefert `installer` nichts und `plan` sagt das. Eigene Kategorien kommen aus der Config (siehe unten).
- **empty-dirs:** Leer heißt: vollständig im Index bekannt und rekursiv ohne jede Datei, auch ohne versteckte, System- und Cloud-Dateien. Ausgeschlossene oder nur aufsummierte Unterordner, Links und Ordner mit Lesefehler beim Scan gelten nie als leer. Die Wurzel und `_Archiv` selbst bleiben stehen. `apply` entfernt einen Ordner nur, wenn er in diesem Moment wirklich leer ist (`RemoveDirectory`), sonst wird die Aktion `stale`. Das Journal hält Attribute und Zeitstempel vom Zustand **vor** dem Lauf fest, damit `undo` auch die Elternordner mit ihrer ursprünglichen Zeit wiederherstellt.
- **archive:** Es zählt nur die mtime der Dateien (keine atime). Geplant wird immer der oberste passende Ordner, nie zusätzlich seine Unterordner; Einzeldateien werden nicht archiviert. Ordner mit Cloud-Platzhaltern, Links, Geschütztem (auch tief darin) oder unvollständigem Index werden nicht als Ganzes verschoben und erscheinen als übersprungen. `apply` prüft Dateianzahl, Größensumme und jüngste mtime des Ordners per Metadaten-Walk (es wird nichts geöffnet) und verschiebt ihn mit einem einzigen Rename auf demselben Volume. Ein vorhandenes Ziel wird nie überschrieben oder zusammengeführt; ein `_Archiv`, das ein Link ist, wird nie betreten.
- **versions:** Gruppen bilden Dateien **desselben Ordners** mit gleichem normalisierten Namen (`_v2`, `final`, `- Kopie (3)`, Datumsangaben) und gleicher Endung. Die Datei mit der jüngsten mtime bleibt (`keep` steht im Plan, bei Gleichstand der lexikografisch letzte Pfad). Ältere Versionen jünger als `--min-age` bleiben liegen und erscheinen als `too-recent`. Ist die jüngste Datei ein Cloud-Platzhalter, wird die ganze Gruppe übersprungen. `apply` verlangt, dass die behaltene Datei noch lokal existiert und nicht älter geworden ist. Dateien unter `_Archiv` sind keine Kandidaten, ein zweiter Lauf plant deshalb nichts mehr.
- **OneDrive-Obergrenze:** Liegt die Wurzel unter einem OneDrive-Ordner und bewegt ein Plan mehr als 1.000 Dateien oder 5 GB (Ordner-Moves zählen mit Dateianzahl und Größensumme, `remove-dir` zählt nicht), verweigert `apply` vor der ersten Änderung, bis `--allow-large` gesetzt ist. Das gilt für alle Pläne, auch `dedupe`; `plan` weist vorher darauf hin.
- **undo und purge:** `undo` räumt leere Ordner unter `quarantine\<run-id>` und unter `_Archiv` wieder auf. Nach `purge` gelten nur die Quarantäne-Aktionen eines Laufs als nicht mehr wiederherstellbar; Archiv-Moves und entfernte Ordner bleiben per `undo` zurückholbar. `runs` zeigt die Aktionen je Typ und Bytes nur für die Quarantäne.
- **Config** (zusätzlich zu Phase 2, flache Schlüssel; `[[junk_rules]]` muss **nach** allen flachen Schlüsseln stehen, sonst gehören sie zur Tabelle). Ungültige Werte scheitern schon beim Laden:

```toml
junk_categories = ["system", "temp", "downloads", "installer"]   # Default: alle eingebauten
installer_min_age = "90d"
downloads_dirs = []            # leer = Known Folder; sonst ersetzt die Liste ihn
archive_older_than = "2y"
versions_min_age = "30d"
onedrive_max_move_files = 1000
onedrive_max_move_bytes = "5GB"

[[junk_rules]]                 # eigene Kategorie (Name darf keine eingebaute sein)
name = "logs"
patterns = ["*.log"]           # Glob auf den Dateinamen, ohne Beachtung der Groß-/Kleinschreibung
min_age = "30d"                # optional
dirs = ["D:\\Logs"]            # optional: nur direkt in diesen Ordnern
```

Eine eigene Kategorie wird mit `--category logs` oder über `junk_categories` aktiv. Dauern gelten als `d` (Tage), `m` (30 Tage) und `y` (365 Tage).


## Entwicklung

Voraussetzungen: Rust (stable, MSVC-Toolchain, wird per `rust-toolchain.toml` gewählt) und die Visual Studio Build Tools.

```
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
cargo run -- scan <pfad>
```

Tipp: Liegt das Repo in einem OneDrive-synchronisierten Ordner (z. B. `Documents`), kann der Build-Ordner beschädigt werden. Mit `CARGO_TARGET_DIR` außerhalb von OneDrive bauen. Mit `ORDNER_CLEANUP_HOME=<ordner>` lassen sich Index und Config isolieren (wird von den Tests genutzt).

## Performance

Gemessen am 2026-10-03 (Release-Build) mit einem synthetischen Baum aus 100.000 Dateien in 1.111 Ordnern (47 MiB, mit Duplikaten), lokal auf SSD:

| Lauf | Dauer | Spitzen-RAM | Ziel |
|---|---|---|---|
| Erstscan inkl. Duplikatsuche | 33 s | 79 MB | < 2 min, < 500 MB |
| Re-Scan (nichts geändert) | 5,6 s | 106 MB | < 15 s |
| Report (alle Formate) | 0,6 s | 87 MB | – |

Planer der Aufräumaktionen (Phase 3), gemessen am 2026-10-03 (Release-Build) mit einem synthetischen Index aus 100.000 Dateien in 1.021 Ordnern; sie lesen nur den Index:

| Planer | Dauer | Ziel |
|---|---|---|
| `plan junk` | 0,33 s | < 10 s |
| `plan empty-dirs` | 0,11 s | < 10 s |
| `plan archive` | 2,2 s | < 10 s |
| `plan versions` (19.000 Aktionen) | 3,9 s | < 10 s |

Die Messung ist als ignorierter Test abgelegt: `cargo test --release --test perf_plans -- --ignored --nocapture`. Den Testbaum erzeugt `cargo run --release --example gen-tree -- <zielordner> [anzahl]`. Der Baum besteht aus kleinen Dateien; bei großen Dateien dominiert das Hashen, das über Teil-Hash und Größengruppen begrenzt wird. Manuell geprüft am 2026-10-03: Scan von `OneDrive\Dokumente` (30 Dateien, davon 1 Cloud-only) ließ Größen, Zeitstempel und Attribute unverändert, die Cloud-only-Datei blieb Cloud-only.

## Lizenz

Doppelt lizenziert unter [MIT](LICENSE-MIT) oder [Apache-2.0](LICENSE-APACHE), nach Wahl.
