# ordner-cleanup

Analyse und sicheres Aufräumen von Ordnersystemen unter Windows 10/11. `scan` und `report` (Phase 1) sind rein lesend: Größenbaum, Dateitypen, Alter, exakte Duplikate, ähnliche Dateien, Strukturprobleme und Problemdateien als HTML, JSON und CSV. Phase 2 (`plan`, `apply`, `undo`, `runs`, `purge`) verschiebt exakte Duplikate nach einem prüfbaren Plan und einer Bestätigung in eine Quarantäne und kann jeden Lauf zurückdrehen. Phase 3 ergänzt die Aufräumaktionen `plan junk`, `empty-dirs`, `archive` und `versions` auf demselben Fundament. Phase 5 bringt Verlauf und Health-Score, Profile, geplante Läufe mit Benachrichtigung und Vorlagen für eine Zielstruktur. Es gibt keine Netzwerkzugriffe und keine Telemetrie.

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


## Regeln: Einsortieren und Umbenennen (Phase 4)

```
ordner-cleanup scan <pfad>
ordner-cleanup plan rules <pfad> [--rules <datei>] [--rule <name>]… [--out <plan.json>]
ordner-cleanup apply <plan.json>
```

`plan rules` wendet die Regeln einer Regeldatei auf alle gescannten Dateien an und schreibt einen Plan aus `move`-Aktionen (Einsortieren und Umbenennen). Es verändert nichts. `apply` und `undo` arbeiten wie in Phase 3; `undo` entfernt zusätzlich die vom Lauf neu angelegten Zielordner, aber nur, wenn sie leer sind, und nie einen Ordner, der vorher schon da war. Die Regeldatei wird vor der Planung vollständig geprüft; Fehler nennen Datei, Zeile und Regelname.

Die Regeldatei liegt standardmäßig neben der Config (`%APPDATA%\ordner-cleanup\rules.toml`). Alternativ gelten `rules_file` in der Config oder `--rules`.

```toml
[[rules]]
name   = "fotos"
glob   = "Eingang/**"          # sonst passt die Regel auch auf schon einsortierte Fotos
ext    = ["jpg", "jpeg", "heic"]
target = "Fotos/{exif.date:%Y}/{exif.date:%Y-%m-%d}_{name}.{ext}"
exif_fallback = "mtime"     # mtime (Default) | skip
spaces = "_"

[[rules]]
name       = "rechnungen"
glob       = "Downloads/**"
name_regex = '(?i)rechnung[ _-]*(\d+)'
min_age    = "7d"
target     = "Finanzen/Rechnungen/{year}/"
iso_date_prefix   = true
strip_copy_suffix = true
```

- **Reihenfolge = Priorität:** Pro Datei gewinnt die erste Regel, deren Bedingungen alle zutreffen.
- **Bedingungen** (UND-verknüpft, mindestens eine): `glob` (auf den Pfad relativ zur Wurzel, `/` und `\` gleichwertig, Groß-/Kleinschreibung egal; `*` bleibt in einem Ordner, `**` geht über Ebenen), `ext`, `name_regex`, `min_age`/`max_age` (inklusive Grenzen), `min_size`/`max_size`.
- **Ziel** (`target`, relativ zur Wurzel): Endet es auf `/`, bleibt der Dateiname; sonst ist das letzte Segment der neue Name; mit `./` am Anfang wird im selben Ordner umbenannt.
- **Platzhalter:** `{name}`, `{ext}`, `{parent}`, `{year}`, `{month}`, `{day}` (aus der mtime, lokale Zeit), `{exif.date}` bzw. `{exif.date:<chrono-Format>}`, `{1}`, `{2}` … (Regex-Gruppen); `{{` und `}}` sind Escapes. Das EXIF-Datum (`DateTimeOriginal`, sonst `DateTimeDigitized`) hat keine Zeitzone und wird unverändert übernommen, daher kann es um einen Tag von der mtime abweichen. Videos haben kein EXIF-Datum, dort greift `exif_fallback`.
- **Normalisierung** (nur auf den Dateinamen): `iso_date_prefix`, `spaces = "_" | "-"`, `umlauts`, `strip_copy_suffix`, `lowercase_ext`. Sie ist idempotent.
- **Kollisionen:** Gleicher Inhalt am Ziel → übersprungen (`duplicate-at-target`); anderer Inhalt oder mehrere Quellen mit demselben Ziel → Durchnummerieren ` (2)`, ` (3)` … in stabiler Reihenfolge.
- **Umbenennen nur in der Schreibweise** (`foto.JPG` → `foto.jpg`) geschieht in einem Schritt; `undo` stellt die alte Schreibweise wieder her.
- **Stabilität:** Ein zweiter `plan rules` nach `apply` und `scan` ergibt keine Aktionen. Würde eine Regel ihre eigenen Ziele beim nächsten Lauf erneut verschieben (z. B. `./{parent}_{name}.{ext}` oder eine Kette von Regel A zu Regel B), weist die Zusammenfassung darauf hin.
- **Schutz:** Geschützte Pfade, Cloud-only-Dateien (auch kein EXIF-Lesen) und Links werden übersprungen; Ziele unter `_Archiv` und `.ordner-cleanup` sind tabu; die OneDrive-Obergrenze gilt unverändert.

Das EXIF-Datum wird im Index zwischengespeichert (gültig bei gleicher Größe und mtime). Dafür hat der Index jetzt das Schema v2; ein älterer Index wird beim Öffnen automatisch migriert. Eine ältere Programmversion verlangt danach `--reset-index`.

## Laufender Betrieb: Verlauf, Health-Score, geplante Läufe, Profile, Vorlagen (Phase 5)

```
ordner-cleanup scan <pfad> | --profile <name>   [--template para|johnny-decimal|<datei>]
ordner-cleanup report [<pfad> | --profile <name>] [--template …]
ordner-cleanup history [<pfad> | --profile <name>] [--folder <name>] [--limit 20]
ordner-cleanup profiles
ordner-cleanup run --profile <name> [--notify]
ordner-cleanup schedule add --profile <name> (--weekly MO 09:00 | --daily 09:00)
ordner-cleanup schedule list
ordner-cleanup schedule remove --profile <name>
```

### Verlauf und Health-Score

Jeder **vollständige** `scan` schreibt eine Momentaufnahme der Kennzahlen in `%LOCALAPPDATA%\ordner-cleanup\history.db`, je Wurzel und je Ordner der ersten Ebene (bei mehr als 200 Ordnern die 200 größten plus „Sonstige“). Die Datei ist unabhängig vom Index: `--reset-index` lässt den Verlauf stehen. Ein abgebrochener Scan schreibt nichts. Ist `history.db` beschädigt oder neuer als das Programm, laufen Scan und Bericht weiter, warnen und fassen die Datei nie an. Die Scan-Ausgabe nennt Score und Veränderung (`Health-Score 72 (−5 seit 27.09.)`). `report` hat den Abschnitt **Verlauf** (Score, die drei größten Abzüge, Tabelle jetzt / letzter Lauf / Veränderung, Trendgrafiken als Inline-SVG ohne externe Skripte); `history` zeigt eine Zeile je Momentaufnahme.

**Vergleichbarkeit:** Eine Momentaufnahme speichert einen Fingerabdruck der Einstellungen, die die Kennzahlen beeinflussen (Excludes, Müll-Kategorien und -Regeln, `old_after`, Struktur-Schwellen, Gewichte, Vorlage samt Inhalt), und die Formelversion (`METRICS_VERSION`). Verglichen wird mit dem zuletzt geschriebenen Lauf derselben Wurzel mit gleichem Fingerabdruck und gleicher Version. Gibt es keinen, wird mit dem letzten Lauf verglichen und der Vergleich als „eingeschränkt vergleichbar“ markiert.

**Health-Score (0–100):** gewichteter Durchschnitt linearer Teilwerte. Jeder Teilwert ist 100 bei 0 Befunden und 0 ab der Schwelle „schlecht“ (`METRICS_VERSION = 1`):

| Teilwert | gemessen | 0 Punkte ab | Gewicht |
|---|---|---|---|
| Müll | größerer Wert aus Müll-Dateien je 1.000 Dateien und Müll-Bytes-Anteil | 50 je 1.000 bzw. 10 % | 20 |
| Duplikate | verschwendete Bytes / lokale Bytes | 25 % | 25 |
| Problemdateien | Dateien mit Problemen je 1.000 Dateien | 50 je 1.000 | 20 |
| Struktur | Strukturbefunde (leer, Ein-Datei-Ordner, zu tief, riesig) je 100 Ordner | 30 je 100 | 20 |
| Versionen | überzählige Versionen (Gruppengröße − 1) je 1.000 Dateien | 50 je 1.000 | 15 |
| Strukturtreue | abweichende / geprüfte Einträge, nur mit Vorlage | 30 % | 20 |

Nicht anwendbare Teilwerte (Strukturtreue ohne Vorlage) und solche mit Gewicht 0 fallen aus der Gewichtung; eine leere Wurzel hat 100. Der Abzug eines Teils ist `Gewicht/Σ Gewichte · (100 − Teilwert)`. Der Score ist deterministisch und monoton (mehr Müll, Duplikate, Probleme oder Abweichungen senken ihn nie weniger als weniger davon). **Alte Daten** außerhalb von `_Archiv` sind nur eine Kennzahl und senken den Score nicht. Die Gewichte stellt man in der Config ein:

```toml
[health.weights]
junk = 20
duplicates = 25
problems = 20
structure = 20
versions = 15
template = 20      # 0 schaltet einen Teilwert ab
```

### Profile

Ein Profil bündelt Wurzel und Abweichungen von der globalen Config. `--profile <name>` ersetzt Pfad und Optionen bei `scan`, `report`, `plan …`, `history` und `run`; Pfad und `--profile` zusammen sind ein Fehler. **Listen** (`exclude`, `summary_only`, `protected_paths`, `junk_categories`) werden ergänzt, **Einzelwerte** (z. B. `old_after`) ersetzt. Vorrang: CLI-Flag vor Profil vor globaler Config. Profile werden beim Laden der Config geprüft (Name `[a-z0-9-]+`, gültige Pläne, Dauern, unbekannte Schlüssel).

```toml
reports_keep = 12                  # Berichte und Pläne je Profil, die run aufbewahrt

[notify]
score_drop  = 5                    # Score fällt um mindestens so viele Punkte
score_below = 60                   # oder liegt unter diesem Wert

[profiles.downloads]
root       = 'C:\Users\Oskar\Downloads'
rules_file = "downloads.rules.toml"      # relativ zum Config-Ordner
plans      = ["rules", "junk"]           # rules | junk | empty-dirs | archive | versions | dedupe
template   = "para"                      # eingebaut oder Pfad (relativ zum Config-Ordner)
force      = false                       # Netzlaufwerk erlauben
exclude    = ["*.iso"]
old_after  = "2y"
```

`profiles` listet alle Profile mit Wurzel, Plänen, letztem Lauf und Score.

### Geplante Läufe: run und schedule

`run --profile X` macht Scan, Momentaufnahme, Bericht und die im Profil genannten Pläne, **nie `apply`**. Der Bericht liegt unter `%LOCALAPPDATA%\ordner-cleanup\reports\<profil>\<JJJJMMTT-HHMMSS>.html` und als `latest.html`, die Pläne unter `plans\<profil>\<Zeitstempel>-<art>.json`; behalten werden die letzten `reports_keep` Läufe. Jeder Lauf hängt eine Zeile an `runs\<profil>.jsonl` (Status `ok`, `partial`, `skipped-locked`, `root-missing`, `failed`, Score, Pläne, Fehler). Ausgefallene Läufe nennt der nächste Bericht im Abschnitt Verlauf. Eine exklusive Sperre (`scan.lock`) verhindert, dass ein manueller Scan und `run` gleichzeitig laufen: der manuelle Scan endet mit einer Meldung, `run` überspringt den Lauf. Exit-Codes von `run`: `0` OK, `2` Teilerfolg (Scan mit Warnungen oder ein Plan gescheitert; die übrigen Schritte laufen weiter), `1` gescheitert bzw. Wurzel nicht erreichbar, `3` übersprungen wegen Sperre.

`schedule add --profile X --weekly MO 09:00` (oder `--daily 09:00`; Wochentage `MO DI MI DO FR SA SO`) legt über `schtasks.exe /XML` eine Aufgabe `\ordner-cleanup\<profil>` an: aktueller Benutzer, ohne Adminrechte und ohne Passwort (`InteractiveToken`, `LeastPrivilege`), nur am Netzteil (Abbruch beim Wechsel auf Akku), verpasste Läufe werden nachgeholt, Priorität „unter normal“, nie zwei Läufe gleichzeitig, höchstens 4 Stunden. Ein zweites `add` für dasselbe Profil ersetzt die Aufgabe. `schedule list` zeigt Auslöser, nächsten und letzten Lauf und warnt bei „Aufgabe zeigt auf fehlendes Programm“; `schedule remove` löscht Aufgabe und Registereintrag (`schedules.json`).

Die Aufgabe startet **`ordner-cleanup-bg.exe`** neben der Haupt-exe: dasselbe Programm ohne Konsolenfenster (`windows_subsystem = "windows"`), damit beim geplanten Lauf kein schwarzes Fenster aufpoppt. Beide Programme gehören in denselben Ordner; wird er verschoben, ersetzt ein neues `schedule add` die Aufgabe.

### Benachrichtigung

Nur bei geplanten Läufen (`run --notify`, setzt `schedule add`); manuelle Befehle bleiben still. Ein Windows-Toast erscheint, wenn der Score um mindestens `score_drop` gefallen ist, unter `score_below` liegt, der Lauf gescheitert ist oder die Wurzel nicht erreichbar war, ein Plan nicht erzeugt werden konnte oder mindestens ein Plan Aktionen enthält („downloads: Health-Score 58 (−7), 2 Pläne bereit“). Ein einzelner wegen Sperre übersprungener Lauf bleibt still, erst der wiederholte meldet sich. Ein Klick öffnet `latest.html` (bei Fehlern das Lauf-Protokoll). Der Absender „Ordner-Cleanup“ wird ohne Adminrechte unter `HKCU\Software\Classes\AppUserModelId\WOsaka.OrdnerCleanup` registriert. Sind Benachrichtigungen systemweit abgeschaltet, ist das kein Fehler; das Ergebnis steht im Bericht und im Protokoll.

### Vorlagen und Soll/Ist

Eine Vorlage beschreibt eine Zielstruktur; `report` zeigt den Abschnitt **Soll/Ist** (höchstens 100 Abweichungen je Art, dazu die Gesamtzahl), und der Teilwert „Strukturtreue“ geht in den Score ein. Eingebaut sind `para` (`1 Projekte`, `2 Bereiche`, `3 Ressourcen`, `4 Archiv`) und `johnny-decimal` (`10-19 Name`, `11 Name`, `11.01 Name`); eigene Vorlagen sind TOML-Dateien. Gesetzt wird sie mit `--template` oder `template` im Profil. Es werden nur Abweichungen angezeigt, nichts angelegt oder verschoben (Umsortieren geht über `plan rules`).

```toml
name = "Meine Ablage"
max_depth = 6              # optional: Ordner unterhalb dieser Tiefe sind Abweichungen
[[levels]]                 # Ebene 1 = direkt unter der Wurzel
required    = ["Privat", "Firma"]   # muss in jedem Ordner der Ebene davor existieren
allowed     = ["Sonstiges"]         # zusätzlich erlaubte Namen
patterns    = ['^\d{4} ']           # erlaubte Namensmuster (Regex, Groß-/Kleinschreibung egal)
allow_other = false                 # Default: nur true, wenn die Ebene nichts davon nennt
files       = false                 # Dateien auf dieser Ebene sind Abweichungen (Default: erlaubt)
[[levels]]                 # Ebene 2
files = true
```

Abweichungen sind: fehlender Pflicht-Ordner, unerwarteter Ordner, Name passt zu keinem Muster, Datei auf einer Ebene ohne Dateien, zu tiefe Verschachtelung (je Zweig nur der erste zu tiefe Ordner). Ebenen ohne Eintrag sind frei; `_Archiv` und `.ordner-cleanup` sind überall erlaubt und werden nie geprüft. Fehler in einer Vorlagendatei nennen Datei und Zeile (ungültige Regex, unbekannter Schlüssel).

Ablageorte: `history.db`, `scan.lock`, `schedules.json`, `reports\<profil>\`, `plans\<profil>\` und `runs\<profil>.jsonl` liegen im Datenordner (`%LOCALAPPDATA%\ordner-cleanup`). Der Verlauf enthält nur Wurzel, Namen der Ordner der ersten Ebene und Zahlen; alles bleibt lokal.

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
| `plan rules` (100.000 Dateien, 10 Regeln, ohne EXIF) | 1,9 s | < 10 s |
| `plan rules` (10.000 JPEGs, erster Lauf / aus dem EXIF-Cache) | 0,4 s / 0,3 s | Cache < 2 s |

Kennzahlen, Score und Verlauf (Phase 5), gemessen am 2026-10-04 (Release-Build, `cargo test --release --test perf_history -- --ignored --nocapture`):

| Schritt | Dauer | Ziel |
|---|---|---|
| Kennzahlen + Score, 100.000 Dateien (nur Index) | 0,14 s | < 2 s |
| Momentaufnahme in `history.db` schreiben | 3 ms | < 2 s |
| Abschnitt Verlauf mit 52 Momentaufnahmen (lesen, bauen, als JSON) | 1 ms (18 KiB) | nicht spürbar |
| Kennzahlen im Verhältnis zum Scan (20.000 Dateien, Dateicache warm, Scan ~17 µs je Datei) | ~2 µs je Datei, ca. 10 % des Scans | ≤ 5 % |

Der Anteil am Scan hängt von dessen Geschwindigkeit ab: Die Kennzahlen kosten rund 2 µs je Datei, der Scan im Messaufbau (kleine Dateien aus dem Dateicache) nur etwa 17 µs. Bei einem Scan ab etwa 40 µs je Datei, wie er auf kalter Platte, mit größeren Dateien oder im OneDrive-Ordner üblich sein dürfte (nicht gemessen), bliebe der Anteil unter 5 %; der Messaufbau erreicht das Ziel nicht.

Die Messung ist als ignorierter Test abgelegt: `cargo test --release --test perf_plans -- --ignored --nocapture`. Den Testbaum erzeugt `cargo run --release --example gen-tree -- <zielordner> [anzahl]`. Der Baum besteht aus kleinen Dateien; bei großen Dateien dominiert das Hashen, das über Teil-Hash und Größengruppen begrenzt wird. Manuell geprüft am 2026-10-03: Scan von `OneDrive\Dokumente` (30 Dateien, davon 1 Cloud-only) ließ Größen, Zeitstempel und Attribute unverändert, die Cloud-only-Datei blieb Cloud-only.

## Lizenz

Doppelt lizenziert unter [MIT](LICENSE-MIT) oder [Apache-2.0](LICENSE-APACHE), nach Wahl.
