# ordner-cleanup

Analyse und sicheres Aufräumen von Ordnersystemen unter Windows 10/11. `scan` und `report` (Phase 1) sind rein lesend: Größenbaum, Dateitypen, Alter, exakte Duplikate, ähnliche Dateien, Strukturprobleme und Problemdateien als HTML, JSON und CSV. Phase 2 (`plan`, `apply`, `undo`, `runs`, `purge`) verschiebt exakte Duplikate nach einem prüfbaren Plan und einer Bestätigung in eine Quarantäne und kann jeden Lauf zurückdrehen. Es gibt keine Netzwerkzugriffe und keine Telemetrie.

Spec Phase 1: [docs/features/scan-analyse-bericht.md](docs/features/scan-analyse-bericht.md), Plan: [docs/implementation-plans/scan-analyse-bericht.md](docs/implementation-plans/scan-analyse-bericht.md). Spec Phase 2: [docs/features/aenderungsplan-apply-undo.md](docs/features/aenderungsplan-apply-undo.md), Plan: [docs/implementation-plans/aenderungsplan-apply-undo.md](docs/implementation-plans/aenderungsplan-apply-undo.md). Roadmap: [docs/roadmap.md](docs/roadmap.md).

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
ordner-cleanup apply <plan.json> [--yes]
ordner-cleanup undo <run-id> [--root <wurzel>] [--yes]
ordner-cleanup runs [<wurzel>]
ordner-cleanup purge [--older-than 30d] [--root <wurzel>] [--yes]
```

Ablauf: Ordner mit `scan` indizieren, mit `plan dedupe` einen Plan erzeugen (die Plan-Datei ist lesbares JSON, ohne `--out` landet sie als `plan-<zeitstempel>.json` im aktuellen Ordner), den Plan prüfen, mit `apply` ausführen, bei Bedarf mit `undo <run-id>` zurückdrehen. `plan` verändert nichts. Pro Duplikatgruppe bleibt immer mindestens eine Datei unberührt.

- **Nichts wird hart gelöscht.** Verschoben wird per Umbenennen auf demselben Volume nach `<wurzel>\.ordner-cleanup\quarantine\<run-id>\<relativer Pfad>`; Zeitstempel und Inhalt bleiben erhalten. Einen Verschiebevorgang über Laufwerksgrenzen gibt es nicht. Ein belegtes Ziel wird nie überschrieben, es bekommt ein Suffix wie `datei (2).txt`. Hart gelöscht wird nur mit `purge` nach Bestätigung, nie automatisch.
- **Bestätigung:** `apply`, `undo` und `purge` fragen einmal `j/N`. In einer nicht interaktiven Sitzung (Skript, Pipe) brechen sie ohne `--yes` ab, statt zu raten.
- **Prüfung vor jeder Aktion:** Größe, Änderungszeit und Hash der Datei und der behaltenen Kopie müssen noch zum Plan passen, sonst wird nur diese Aktion als `stale` übersprungen und der Rest läuft weiter. Fehlt die behaltene Datei, passiert in der Gruppe nichts.
- **Geschützte Pfade** werden nie angefasst: `C:\Windows`, `Program Files`, `Program Files (x86)` und `ProgramData` immer und nicht abschaltbar; `AppData`, `.git`, `node_modules` und Projektordner mit `Cargo.toml`, `package.json`, `*.sln` oder `*.csproj` standardmäßig; weitere über `protected_paths` in der Config. `apply` prüft das unabhängig vom Plan noch einmal (manipulierte Pläne, `..`, `\\?\`, 8.3-Kurznamen). Liegt die angegebene Wurzel selbst unter `AppData`, gilt nur diese eine Regel nicht.
- **OneDrive:** Cloud-Platzhalter und Links (Symlinks/Junctions) werden nie gelesen, gehasht oder verschoben. Liegt die Wurzel in OneDrive, wird die Quarantäne mitsynchronisiert; `plan` und `apply` warnen davor.
- **Journal:** Jeder Lauf schreibt ein Write-ahead-Journal (`<wurzel>\.ordner-cleanup\journal\<run-id>.jsonl`, `fsync` je Eintrag) vor dem Verschieben. `undo` prüft den echten Dateizustand, überschreibt nie (Kollisionen werden gemeldet, die Datei bleibt in der Quarantäne) und funktioniert auch nach einem Absturz. Ein zweiter `apply` desselben Plans ändert nichts.
- **Lauf-Register:** Damit `undo <run-id>` die Wurzel findet, vermerkt `apply` jeden Lauf in `%LOCALAPPDATA%\ordner-cleanup\runs.jsonl`. Maßgeblich bleibt das Journal; fehlt das Register, hilft `--root`.
- **Config** (`%APPDATA%\ordner-cleanup\config.toml`): `protected_paths = ["D:\\Wichtig"]` und `quarantine_days = 30` (Aufbewahrungsdauer; `purge` löscht nur Läufe, die älter sind).
- **Exit-Codes:** `0` alles erledigt, `1` Fehler oder Abbruch (auch „N“ bei der Rückfrage), `2` Teilerfolg (stale, übersprungen, Fehler einzelner Aktionen, Kollisionen beim `undo`).
- Der Ordner `.ordner-cleanup` wird von `scan` nie erfasst.

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

Den Testbaum erzeugt `cargo run --release --example gen-tree -- <zielordner> [anzahl]`. Der Baum besteht aus kleinen Dateien; bei großen Dateien dominiert das Hashen, das über Teil-Hash und Größengruppen begrenzt wird. Manuell geprüft am 2026-10-03: Scan von `OneDrive\Dokumente` (30 Dateien, davon 1 Cloud-only) ließ Größen, Zeitstempel und Attribute unverändert, die Cloud-only-Datei blieb Cloud-only.
