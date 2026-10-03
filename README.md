# ordner-cleanup

Rein lesende Analyse von Ordnersystemen unter Windows 10/11: Größenbaum, Dateitypen, Alter, exakte Duplikate, ähnliche Dateien, Strukturprobleme und Problemdateien. Das Ergebnis ist ein Bericht als HTML, JSON und CSV. Phase 1 verändert nichts am gescannten Baum, es gibt keine Netzwerkzugriffe und keine Telemetrie.

Spec: [docs/features/scan-analyse-bericht.md](docs/features/scan-analyse-bericht.md), Plan: [docs/implementation-plans/scan-analyse-bericht.md](docs/implementation-plans/scan-analyse-bericht.md), Roadmap: [docs/roadmap.md](docs/roadmap.md).

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
