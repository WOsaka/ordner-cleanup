---
title: "Gesamtdokumentation ordner-cleanup"
created: 2026-10-03
updated: 2026-10-03
---

# ordner-cleanup: Gesamtdokumentation

`ordner-cleanup` ist ein Kommandozeilenwerkzeug für Windows 10/11 (NTFS), das Ordnersysteme **analysiert** und **sicher aufräumt**. Es arbeitet in zwei Stufen:

1. **Analyse (nur lesend):** `scan` und `report` zeigen, wo der Platz hingeht, was doppelt oder alt ist und welche Dateien Probleme verursachen.
2. **Aufräumen (schreibend, aber umkehrbar):** `plan`, `apply`, `undo`, `runs`, `purge` verschieben Dateien und Ordner nach einem prüfbaren Plan und einer Bestätigung. Jeder Lauf lässt sich zurückdrehen.

Es gibt keine Netzwerkzugriffe und keine Telemetrie. Alles läuft lokal.

## Inhalt

1. [Einrichtung](#1-einrichtung)
2. [Schnellstart](#2-schnellstart)
3. [Grundkonzepte](#3-grundkonzepte)
4. [Befehlsreferenz](#4-befehlsreferenz)
5. [Analyse: scan und report](#5-analyse-scan-und-report)
6. [Aufräumen: Plan, Apply, Undo](#6-aufräumen-plan-apply-undo)
7. [Die Aufräumaktionen im Einzelnen](#7-die-aufräumaktionen-im-einzelnen)
8. [Sicherheitsmechanismen](#8-sicherheitsmechanismen)
9. [Konfiguration](#9-konfiguration)
10. [Dateien und Speicherorte](#10-dateien-und-speicherorte)
11. [Exit-Codes](#11-exit-codes)
12. [Fehlersuche und FAQ](#12-fehlersuche-und-faq)
13. [Entwicklung](#13-entwicklung)
14. [Weiterführende Dokumente](#14-weiterführende-dokumente)

---

## 1. Einrichtung

### Voraussetzungen

- Windows 10 oder 11 (nur NTFS-Laufwerke lokal; andere Plattformen werden nicht unterstützt)
- Zum Selbstbauen: Rust (stable, MSVC-Toolchain; `rust-toolchain.toml` wählt sie automatisch) und die Visual Studio Build Tools

### Installation

Es gibt keinen Installer. Das Programm ist eine einzelne `.exe` ohne Laufzeitabhängigkeiten (SQLite ist eingebettet).

**Selbst bauen:**

```
cargo build --release
```

Ergebnis: `target\release\ordner-cleanup.exe`. Diese Datei kann an einen beliebigen Ort kopiert oder in den `PATH` aufgenommen werden. Die CI (`.github/workflows/ci.yml`) baut die `.exe` bei jedem Push als Artefakt `ordner-cleanup-windows`.

**Ohne Installation direkt aus dem Quellcode:**

```
cargo run -- <befehl> …
```

> Liegt das Repo in einem OneDrive-synchronisierten Ordner (z. B. `Documents`), kann der Build-Ordner beschädigt werden. Dann mit `CARGO_TARGET_DIR` außerhalb von OneDrive bauen.

### Erste Konfiguration (optional)

Das Programm läuft ohne Konfiguration mit sinnvollen Defaults. Wer Ausschlüsse, geschützte Pfade oder Schwellwerte anpassen will, legt `%APPDATA%\ordner-cleanup\config.toml` an (siehe [Konfiguration](#9-konfiguration)). CLI-Flags haben immer Vorrang vor der Config.

### Hilfe

```
ordner-cleanup --help
ordner-cleanup <befehl> --help
ordner-cleanup --version
```

---

## 2. Schnellstart

**Nur analysieren (nichts wird verändert):**

```
ordner-cleanup scan "C:\Users\ich\Dokumente"
ordner-cleanup report "C:\Users\ich\Dokumente"
```

Der Bericht liegt danach im Ordner `ordner-cleanup-report-<zeitstempel>` im aktuellen Verzeichnis; `report.html` im Browser öffnen.

**Aufräumen (immer in drei Schritten: planen, prüfen, ausführen):**

```
ordner-cleanup plan dedupe "C:\Users\ich\Dokumente" --out plan.json
# plan.json in einem Editor prüfen
ordner-cleanup apply plan.json
# bei Bedarf rückgängig machen:
ordner-cleanup undo <run-id>
```

Die `run-id` gibt `apply` am Ende aus; `ordner-cleanup runs` listet alle Läufe.

**Empfohlener Komplettputz:**

```
ordner-cleanup scan <pfad>
ordner-cleanup plan junk <pfad>        → prüfen → apply
ordner-cleanup scan <pfad>             (Index aktualisieren)
ordner-cleanup plan empty-dirs <pfad>  → prüfen → apply
```

---

## 3. Grundkonzepte

### Index

`scan` schreibt alle Metadaten (Pfad, Größe, Zeitstempel, Attribute, Hashes) in eine SQLite-Datenbank. Alle weiteren Befehle (`report`, alle `plan …`) arbeiten **nur auf diesem Index** und lesen die Dateien nicht erneut. Deshalb muss ein Ordner vor `report` und `plan` gescannt sein, und der Index sollte aktuell sein: nach größeren Änderungen (etwa nach einem `apply`) erneut scannen.

### Plan, Review, Apply

Jede schreibende Aktion läuft in drei getrennten Schritten:

1. **`plan …`** erzeugt eine lesbare JSON-Plan-Datei. Der Befehl verändert nichts (Dry-Run).
2. **Prüfen:** Der Plan listet jede Aktion mit Pfad, Größe, Zeitstempel, Begründung und die übersprungenen Kandidaten mit Grund.
3. **`apply`** führt den Plan nach einer Rückfrage aus.

### Nichts wird hart gelöscht

Dateien wandern in eine **Quarantäne** (`<wurzel>\.ordner-cleanup\quarantine\<run-id>\…`) oder in ein **Archiv** (`<wurzel>\_Archiv\…`). Endgültig gelöscht wird nur mit `purge` nach Bestätigung. Einzige Ausnahme sind leere Ordner bei `plan empty-dirs`; auch diese lassen sich per `undo` wiederherstellen.

### Läufe und Journal

Jeder `apply` ist ein **Lauf** mit einer Lauf-ID. Vor jeder Änderung schreibt das Programm einen Eintrag in ein Journal. Damit kann `undo` einen Lauf vollständig zurückdrehen, auch nach einem Absturz.

### Wurzel

Die „Wurzel“ ist der Ordner, den man an `scan` übergibt. Quarantäne, Archiv und Journal liegen immer unterhalb der Wurzel und damit auf demselben Volume, damit Verschieben ein schnelles Umbenennen bleibt.

---

## 4. Befehlsreferenz

| Befehl | Zweck | Verändert Dateien? |
|---|---|---|
| `scan <pfad>` | Ordnerbaum erfassen, Duplikate hashen | nein (nur Index) |
| `report [<pfad>]` | Bericht aus dem Index erzeugen | nein (nur Berichtsordner) |
| `index list` / `index remove <pfad>` | Gescannte Wurzeln verwalten | nein (nur Index) |
| `plan dedupe\|dedupe-dirs\|junk\|empty-dirs\|archive\|versions <pfad>` | Änderungsplan erzeugen | nein |
| `apply <plan.json>` | Plan ausführen | **ja** (umkehrbar) |
| `undo <run-id>` | Lauf zurückdrehen | **ja** |
| `runs [<wurzel>]` | Läufe auflisten | nein |
| `purge` | Abgelaufene Quarantäne endgültig löschen | **ja (hart)** |

### scan

```
ordner-cleanup scan <pfad> [--exclude <glob>]... [--summary-only <glob>]... [--no-default-excludes] [--force] [--reset-index] [--threads <n>]
```

| Option | Wirkung |
|---|---|
| `--exclude <glob>` | Zusätzlich ausschließen (mehrfach möglich); Pfad wird nicht betreten |
| `--summary-only <glob>` | Ordner nur aufsummieren (Größe, Dateianzahl), ohne Einzeldateien |
| `--no-default-excludes` | Eingebaute Ausschlüsse abschalten |
| `--force` | Netzlaufwerk bzw. UNC-Pfad trotzdem scannen |
| `--reset-index` | Index verwerfen und neu aufbauen |
| `--threads <n>` | Anzahl Threads (`0` = automatisch) |

Globs sind unabhängig von Groß-/Kleinschreibung; `\` und `/` werden gleich behandelt.

### report

```
ordner-cleanup report [<pfad>] [--out <ordner>] [--format html,json,csv] [--old-after 1y] [--top 50]
```

| Option | Wirkung |
|---|---|
| `<pfad>` | Gescannte Wurzel; darf fehlen, wenn nur eine im Index ist |
| `--out <ordner>` | Basisordner für den Bericht (Default: aktuelles Verzeichnis) |
| `--format` | `html`, `json`, `csv`, kommagetrennt (Default: alle drei) |
| `--old-after <dauer>` | Ab wann eine Datei als „alt“ gilt (Default `1y`) |
| `--top <n>` | Länge der Top-Listen (Default 50) |

### index

```
ordner-cleanup index list
ordner-cleanup index remove <pfad>
```

`list` zeigt alle gescannten Wurzeln mit Status, Zeitpunkt und Fehleranzahl. `remove` entfernt eine Wurzel samt ihren Einträgen aus dem Index (die Dateien selbst bleiben unberührt).

### plan

```
ordner-cleanup plan dedupe     <pfad> [--keep oldest|newest|path:<absoluter ordner>] [--out <plan.json>]
ordner-cleanup plan dedupe-dirs <pfad> [--keep oldest|newest|path:<absoluter ordner>] [--min-size <größe>] [--out <plan.json>]
ordner-cleanup plan junk       <pfad> [--category system,temp,downloads,installer,<eigene>] [--out <plan.json>]
ordner-cleanup plan empty-dirs <pfad> [--out <plan.json>]
ordner-cleanup plan archive    <pfad> [--older-than 2y] [--out <plan.json>]
ordner-cleanup plan versions   <pfad> [--min-age 30d] [--out <plan.json>]
ordner-cleanup plan seal       <plan.json> [--yes]
```

Ohne `--out` heißt die Datei `plan-<zeitstempel>.json` und liegt im aktuellen Ordner. `plan seal` versiegelt einen von Hand geänderten Plan neu (siehe [6.1](#61-ablauf)).

### apply, undo, runs, purge

```
ordner-cleanup apply <plan.json> [--yes] [--allow-large]
ordner-cleanup undo  <run-id> [--root <wurzel>] [--yes]
ordner-cleanup runs  [<wurzel>]
ordner-cleanup purge [--older-than 30d] [--root <wurzel>] [--yes]
```

### Dauer-Angaben

Überall, wo eine Dauer erwartet wird (`--old-after`, `--older-than`, `--min-age`, Config), gelten die Einheiten `d` (Tage), `m` (30 Tage) und `y` (365 Tage), z. B. `90d`, `18m`, `2y`.

---

## 5. Analyse: scan und report

### 5.1 Was `scan` tut

1. Läuft den Ordnerbaum ab und zeigt eine Fortschrittsanzeige (Dateien, Ordner, Bytes, Fehler, aktueller Pfad).
2. Schreibt die Metadaten in den Index (`%LOCALAPPDATA%\ordner-cleanup\index.db`).
3. Ermittelt exakte Duplikate in drei Stufen, um möglichst wenig zu lesen:
   1. Gruppierung nach **Dateigröße** (Dateien mit einzigartiger Größe werden nie gehasht)
   2. **Teil-Hash** über Anfang und Ende (je 64 KiB)
   3. **Voll-Hash** (xxHash3-128), nur für die verbleibenden Kandidaten
4. Gibt zum Schluss eine Zusammenfassung aus (Dateien, Ordner, Größe, Fehler).

**Inkrementelle Re-Scans:** Beim erneuten Scan werden Dateien mit unveränderter Größe und Änderungszeit nicht neu verarbeitet, ihre Hashes werden übernommen. Gelöschte Dateien verschwinden aus dem Index, veraltete Hashes werden verworfen.

**Abbruch mit Strg+C** ist sicher: Der Index bleibt konsistent (Transaktionen), der nächste Scan setzt sauber auf. Exit-Code ist dann `1`.

### 5.2 Ausschlüsse

| Stufe | Betroffen |
|---|---|
| **Ausgeschlossen** (wird nicht betreten) | `C:\Windows`, `Program Files`, `Program Files (x86)`, `ProgramData`, `$Recycle.Bin`, `System Volume Information`, `pagefile.sys`, `hiberfil.sys`, `swapfile.sys`, der eigene Datenordner `%LOCALAPPDATA%\ordner-cleanup`, eigene Berichtsordner `ordner-cleanup-report-*` sowie `.ordner-cleanup` (Quarantäne/Journal) |
| **Nur Summe** (Größe und Anzahl, keine Details) | `.git`, `node_modules`, `target`, `.venv`, `venv`, `__pycache__`, `.gradle`, `bin`/`obj` (nur wenn im übergeordneten Ordner eine `*.csproj`/`*.sln` liegt), `%USERPROFILE%\AppData` |
| **Voll** | alles andere |

Liegt die angegebene Wurzel selbst in einem Default-Bereich, hat der ausdrücklich angegebene Pfad Vorrang; `scan` weist darauf hin. Eigene Regeln kommen über `--exclude`, `--summary-only` oder die Config; `--no-default-excludes` schaltet die eingebauten ab.

### 5.3 Besondere Dateiarten

| Fall | Verhalten |
|---|---|
| **OneDrive-Cloud-Platzhalter** (Datei nur in der Cloud) | Nur Metadaten. Wird nie geöffnet, gelesen oder gehasht, damit kein Download ausgelöst wird. Erscheint höchstens als „wahrscheinliches Duplikat“. |
| **Lokal verfügbare OneDrive-Dateien** | Wie normale Dateien |
| **Symlinks und Junctions** | Als Link erfasst (mit Ziel), aber nicht betreten, Ziel-Größe wird nicht mitgezählt. Zyklen sind damit ausgeschlossen. |
| **Hardlinks** | Als Gruppe erkannt, Größe nur einmal gezählt, nicht als Duplikat gemeldet |
| **Versteckte/System-Dateien** | Werden mitgezählt und markiert |
| **Lange Pfade (> 260 Zeichen)** | Werden über das `\\?\`-Präfix korrekt gelesen und als Problemdatei gemeldet |
| **Zugriff verweigert, gesperrte Dateien** | Scan läuft weiter; der Pfad erscheint im Bericht unter „Fehler“. Eine nicht lesbare Datei zählt nicht als verifiziertes Duplikat. |
| **Datei ändert sich während des Scans** | Hash wird verworfen, Warnung protokolliert |
| **Netzlaufwerke / UNC** | Nur mit `--force` |
| **Ungültiges UTF-16 im Namen** | Wird verlustfrei gespeichert, in der Anzeige mit Ersatzzeichen dargestellt |

Dateien werden ausschließlich lesend geöffnet (Share-Modus Read/Write/Delete), sodass andere Programme nicht blockiert werden. Der letzte Zugriffszeitstempel (Last Access) kann sich durch das Hashen ändern; Windows aktualisiert ihn standardmäßig kaum.

### 5.4 Was `report` ausgibt

`report` liest den Index und schreibt in den Ordner `ordner-cleanup-report-<zeitstempel>`:

| Datei | Inhalt |
|---|---|
| `report.html` | Eine einzelne, eigenständige HTML-Datei ohne externe Ressourcen: sortierbare und filterbare Tabellen, aufklappbarer Größenbaum |
| `report.json` | Alle Abschnitte maschinenlesbar |
| `duplicates.csv`, `similar.csv`, `problems.csv`, `old-files.csv`, `top-files.csv`, `empty-dirs.csv`, `errors.csv` | Je eine Liste als CSV |

Zusätzlich druckt `report` eine Terminal-Zusammenfassung und nennt die geschriebenen Dateien. Berichte enthalten Pfade und Metadaten, aber keine Dateiinhalte. Weil Pfad- und Dateinamen vertraulich sein können, liegt die Weitergabe in der Verantwortung des Nutzers.

### 5.5 Abschnitte des Berichts

| Abschnitt | Was er zeigt |
|---|---|
| **Übersicht** | Wurzel, Scan-Zeitpunkt und -Status, Anzahl Dateien und Ordner, Gesamtgröße, Anteil „Nur Summe“, Anzahl Cloud-Platzhalter, Fehler |
| **Größenbaum** | Aufklappbarer Baum mit kumulierter Größe und Dateianzahl, nach Größe sortiert |
| **Top-Listen** | Größte Dateien und größte Ordner (Top N) |
| **Dateitypen** | Anzahl und Größe je Endung und je Kategorie (Dokumente, Bilder, Video, Audio, Archive, Code, Ausführbar, Sonstige) |
| **Alter** | Altersklassen nach Änderungsdatum (< 1 Monat, < 6 Monate, < 1 Jahr, < 3 Jahre, > 3 Jahre) und eine Liste „alter“ Dateien (Schwelle `--old-after`) |
| **Exakte Duplikate** | Gruppen identischer Dateien (Voll-Hash) mit Pfaden, Größe und verschwendetem Platz (`Größe × (Anzahl − 1)`), nach Verschwendung sortiert |
| **Ordner-Duplikate** | Gruppen gleicher Ordner (oberste Ebene, mit Platzgewinn) und „teilweise gleiche“ Ordnerpaare ab der Schwelle `dedupe_dirs_partial_threshold` (nur Hinweis; im HTML die 50 größten ab 1 MiB, vollständig in `folder-partial.csv` und im JSON) |
| **Wahrscheinliche Duplikate** | Gruppen mit Cloud-Platzhaltern: gleicher Name und gleiche Größe, ausdrücklich als „nicht verifiziert“ markiert |
| **Ähnliche Dateien / Versionen** | Gruppen im selben Ordner mit gleichem normalisiertem Namen (siehe unten) |
| **Struktur** | Leere Ordner, Ordner mit nur einer Datei, zu tiefe Verschachtelung, Riesenordner |
| **Problemdateien** | Siehe Tabelle unten |
| **Fehler** | Alle Pfade, die nicht gelesen werden konnten, mit Grund |

**Strukturprüfung** (Schwellwerte per Config änderbar):

| Befund | Bedingung | Config-Schlüssel |
|---|---|---|
| Leerer Ordner | keine direkten Einträge | – |
| Ein-Datei-Ordner | genau eine Datei, sonst nichts | – |
| Tief verschachtelt | Tiefe relativ zur Wurzel > 8 | `max_depth_warning` |
| Riesenordner | mehr als 1000 direkte Einträge | `huge_dir_entries` |

Links und „Nur Summe“-Ordner werden nicht bewertet.

**Problemdateien:**

| Problem | Erklärung |
|---|---|
| Pfad länger als 260 Zeichen | Verursacht Sync- und Backup-Fehler |
| Ungültige Zeichen | `" * : < > ? / \ \|` und Steuerzeichen im Namen |
| Reservierter Name | `CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9` |
| Führendes/abschließendes Leerzeichen, abschließender Punkt | Für OneDrive/SharePoint ungültig |
| 0-Byte-Datei | |
| Office-Lockdatei | `~$*` |
| Temp-Datei | `*.tmp` |
| Abgebrochener Download | `*.crdownload`, `*.part`, `*.partial` |
| OneDrive-Konfliktdatei | `<name>-<PCNAME>.<ext>`; der lokale Rechnername kommt aus `%COMPUTERNAME%`, weitere über `onedrive_conflict_hostnames` |
| Systemmüll | `Thumbs.db`, `desktop.ini`, `.DS_Store` |

**Namensheuristik für ähnliche Dateien:** Dateien im **selben Ordner** mit gleicher Endung und gleichem normalisierten Namen bilden eine Gruppe (mindestens zwei Dateien). Normalisiert wird durch Kleinschreibung, vereinheitlichte Trenner (`_`, `-`, Leerzeichen) und Entfernen von

- Kopie-Zusätzen: `- Kopie`, `- Kopie (3)`, `- Copy`, `Copy of`, `Kopie von`, ` (n)`
- Versionsmarkern: `v2`, `version 2`, `rev3`, `final`, `endgültig`, `neu`, `new`, `alt`, `old`
- Datumsangaben am Anfang oder Ende: `YYYY-MM-DD`, `YYYYMMDD`, `DD.MM.YYYY`

Beispiel: `Angebot.docx`, `Angebot_v2.docx` und `Angebot - Kopie (3).docx` bilden eine Gruppe.

Mehrere gescannte Wurzeln: Ohne Pfad listet `report` die vorhandenen Wurzeln und verlangt eine Auswahl, wenn es mehr als eine gibt. Wurde ein Scan nicht vollständig beendet, warnt der Bericht und `report` endet mit Exit-Code `2`.

---

## 6. Aufräumen: Plan, Apply, Undo

### 6.1 Ablauf

```
scan  →  plan <aktion>  →  Plan prüfen  →  apply  →  (undo)  →  (purge)
```

Voraussetzung: Die Wurzel ist gescannt. `plan` liest nur den Index. Der Plan ist eine JSON-Datei (`version: 3`) mit

- `integrity`: Prüfsumme des Inhalts (`sha256:<64 Hex-Zeichen>`), als erstes Feld; `plan` nennt sie gekürzt in der Ausgabe
- `kind`, `root`, `created` und den Aufrufparametern
- `actions`: jede geplante Aktion mit `id`, `type`, `path`, `size`, `mtime`, `reason` und je nach Typ `hash`, `keep`, `target`
- `skipped`: Kandidaten, die nicht geplant wurden, mit Grund

Pläne der Versionen 1 und 2 und ihre Journale funktionieren weiter. Sie haben keine Prüfsumme; `apply` (und die GUI im Bestätigungsdialog und in der Kopfzeile des Plans) warnt deshalb „Ungeschützter Plan (Format N): Änderungen an der Datei werden nicht erkannt.“

**Prüfsumme:** Die Prüfsumme ist SHA-256 über den Plan in kanonischer Form (geparst und kompakt neu geschrieben, ohne `integrity`). Einrückung, Zeilenenden (CRLF/LF) und die Reihenfolge der JSON-Schlüssel ändern sie deshalb nicht; ein Editor, der die Datei nur neu formatiert, löst keinen Fehlalarm aus. Jede inhaltliche Änderung (ein Wert, eine entfernte oder hinzugefügte Aktion, ein Übersprungener) ändert sie. Ein Plan im Format 3 ohne `integrity` oder mit falsch formatiertem Wert ist ungültig; ein Plan im Format 1 oder 2 darf das Feld nicht tragen. Die Prüfsumme schützt vor Versehen (Editor, Sync-Konflikt, halber Schreibvorgang), nicht vor Absicht: Wer die Datei schreiben kann, kann sie auch neu versiegeln.

**Plan von Hand ändern:** Wer einen Plan bewusst bearbeitet (z. B. Aktionen streicht), versiegelt ihn danach mit `plan seal <plan.json>`. Der Befehl zeigt Art, Wurzel, Anzahl der Aktionen und Übersprungenen, den bisherigen Zustand der Prüfsumme und die neue Prüfsumme, fragt `j/N` und ersetzt die Datei dann atomar. Ein Plan im Format 1 oder 2 wird dabei auf Format 3 gehoben. Ein strukturell ungültiger Plan wird nicht versiegelt. Ist die Prüfsumme schon richtig, meldet `plan seal` „Plan ist bereits versiegelt“ und lässt die Datei unverändert. Ohne Terminal braucht `plan seal` wie `apply` die Option `--yes`. Eine Option, die Prüfung beim `apply` abzuschalten, gibt es nicht.

**Teilplan aus der GUI:** Werden in der Review Einträge abgewählt, entsteht eine neue Datei `<name>-auswahl-<zeitstempel>.json` mit eigener Prüfsumme; das Original bleibt unverändert. Vor dem Anwenden prüft die GUI, ob die geöffnete Plan-Datei auf der Platte noch dem geladenen Plan entspricht. Wurde sie inzwischen verändert, führt sie nichts aus und bietet „Plan neu laden“ an.

### 6.2 apply

`apply <plan.json>` führt den Plan aus:

1. Prüft die Prüfsumme der Plan-Datei. Weicht sie ab, bricht `apply` vor jeder Dateioperation ab („Plan wurde nach dem Erstellen verändert“, mit erwarteter und gefundener Prüfsumme, je 12 Zeichen), Exit-Code `3`, kein Lauf im Journal.
2. Zeigt eine Übersicht und fragt einmal `j/N`. In nicht interaktiven Sitzungen (Skript, Pipe) bricht `apply` **ohne `--yes` ab**, statt zu raten.
3. Prüft unabhängig vom Plan noch einmal alle Schutzregeln.
4. Prüft **je Aktion**, ob Größe, Änderungszeit und (bei `dedupe`) Hash von Datei und behaltener Kopie noch zum Plan passen. Weicht etwas ab, wird nur diese Aktion als `stale` übersprungen, der Rest läuft weiter. Fehlt die behaltene Datei, passiert in der Gruppe nichts.
5. Schreibt vor jeder Änderung einen Journal-Eintrag (mit `fsync`) und führt dann die Aktion aus.
6. Gibt am Ende die Lauf-ID und eine Bilanz je Aktionstyp aus.

Ein zweiter `apply` desselben Plans ändert nichts (Idempotenz). Ein belegtes Ziel wird nie überschrieben; es bekommt ein Suffix wie `datei (2).txt`.

`--allow-large` hebt die OneDrive-Obergrenze auf (siehe [Abschnitt 8](#8-sicherheitsmechanismen)).

### 6.3 undo

`undo <run-id>` dreht einen kompletten Lauf zurück:

- Quarantäne-Dateien kommen an ihren ursprünglichen Pfad zurück
- Archiv-Verschiebungen werden rückbenannt
- entfernte leere Ordner werden von oben nach unten neu angelegt, Attribute und Zeitstempel werden zurückgesetzt
- leere Ordner unter `quarantine\<run-id>` und `_Archiv` werden anschließend aufgeräumt

`undo` prüft den echten Dateizustand und überschreibt **nie**. Kollisionen (am Ursprungsort liegt inzwischen etwas anderes) werden gemeldet, die Datei bleibt in der Quarantäne. Das funktioniert auch nach einem Absturz mitten im `apply`, weil das Journal vor der Änderung geschrieben wurde.

Findet `undo` die Wurzel nicht im Lauf-Register, hilft `--root <wurzel>`. Auch `undo` fragt `j/N` (oder `--yes`).

### 6.4 runs

`runs [<wurzel>]` listet alle Läufe (oder die einer Wurzel) mit Lauf-ID, Status, Aktionen je Typ und der Größe der Quarantäne. Bytes werden nur für Quarantäne-Aktionen gezählt.

### 6.5 purge

`purge` löscht die Quarantäne von Läufen **endgültig**, die älter sind als `--older-than` (Default: `quarantine_days` aus der Config, 30 Tage). Es fragt vorher `j/N` (oder `--yes`) und läuft nie automatisch. Nach `purge` sind nur die Quarantäne-Aktionen dieses Laufs nicht mehr wiederherstellbar; Archiv-Verschiebungen und entfernte leere Ordner bleiben per `undo` zurückholbar.

---

## 7. Die Aufräumaktionen im Einzelnen

| Befehl | Was geplant wird | Aktionstyp | Undo |
|---|---|---|---|
| `plan dedupe` | Exakte Duplikate in die Quarantäne | `quarantine` | Datei zurück aus der Quarantäne |
| `plan dedupe-dirs` | Doppelte ganze Ordner in die Quarantäne | `quarantine` (`is_dir`) | Ordner zurück aus der Quarantäne |
| `plan junk` | Müll nach Kategorien in die Quarantäne | `quarantine` (ohne Hash) | Datei zurück aus der Quarantäne |
| `plan empty-dirs` | Rekursiv leere Ordner, von unten nach oben | `remove-dir` | Ordner neu anlegen, Attribute und Zeitstempel zurücksetzen |
| `plan archive` | Lange unberührte Ordner nach `<wurzel>\_Archiv\<Jahr>\…` | `move` (ganzer Ordner) | Rückbenennung |
| `plan versions` | Ältere Dateiversionen nach `<wurzel>\_Archiv\Versionen\…` | `move` (Datei) | Rückbenennung |

### 7.1 plan dedupe: Duplikate

```
ordner-cleanup plan dedupe <pfad> [--keep oldest|newest|path:<absoluter ordner>]
```

Pro Gruppe identischer Dateien (gleicher Voll-Hash) bleibt **immer mindestens eine Datei unberührt**; die übrigen kommen in die Quarantäne. `--keep` wählt, welche bleibt:

| Strategie | Behalten wird |
|---|---|
| `oldest` (Default) | die Kopie mit der ältesten Änderungszeit |
| `newest` | die Kopie mit der jüngsten Änderungszeit |
| `path:D:\Ordner` | die älteste Kopie unterhalb dieses absoluten Ordners |

Bei Gleichstand entscheidet der kürzeste, dann der alphabetisch erste Pfad. Geschützte Dateien werden weder entfernt noch als „keep“ gewählt. Hardlinks auf die behaltene Datei zählen nicht (sie würden keinen Platz freigeben). Cloud-Platzhalter und Links kennt der Index in Gruppen gar nicht, sie werden nie angefasst. Der Plan nennt den tatsächlich freiwerdenden Platz.

### 7.1a plan dedupe-dirs: doppelte Ordner

```
ordner-cleanup plan dedupe-dirs <pfad> [--keep oldest|newest|path:<absoluter ordner>] [--min-size <größe>] [--out <plan.json>]
```

Die Erkennung arbeitet nur auf dem Index (kein zusätzliches Lesen von Dateien): Aus (Name, Größe, Voll-Hash) der Dateien und den Fingerabdrücken der Unterordner wird von unten nach oben ein Fingerabdruck je Ordner gebildet (Merkle-Prinzip). Ordner mit gleichem Fingerabdruck bilden eine Gruppe.

**Gleichheit:** dieselben relativen Dateipfade (Groß-/Kleinschreibung egal) mit derselben Größe und demselben Hash; der Name des Ordners selbst zählt nicht. Ignoriert werden `Thumbs.db`, `ehthumbs.db`, `.DS_Store`, `desktop.ini`, `*.tmp`, `~$*`, `*.crdownload`, `*.part`, `*.partial` und leere Unterordner (ignorierte Dateien wandern bei der Verschiebung trotzdem mit). Ein Ordner braucht mindestens eine verglichene Datei mit Größe > 0. Ordner mit Cloud-Platzhaltern, Links, Lücken im Index oder Dateien ohne Hash sind nicht vergleichbar und werden nie angefasst (Dateien werden nie gelesen oder gehasht).

**Welcher Ordner bleibt:** 1. ein schon behaltener Ordner (damit nichts innerhalb eines behaltenen Ordners entfernt wird), 2. der Name: „Kopie“, „Copy“, „Backup“, „Sicherung“, „alt“/„old“/„bak“ als ganzes Wort, „(2)“ am Ende und eigene Wörter aus `dedupe_dirs_copy_patterns` machen einen Ordner zur Kopie, normale Namen bleiben bevorzugt („Altbau“ und „Copyright“ zählen nicht), 3. `--keep` (nach der jüngsten Änderungszeit aller Dateien im Ordner), 4. kürzester, dann alphabetisch erster Pfad. Bei `path:<prefix>` hat das Präfix Vorrang vor dem Namen; liegt kein Ordner der Gruppe darunter, bleibt die Gruppe unberührt (`group-incomplete`).

**Plan:** `type: quarantine` mit `is_dir: true`, `files` (Dateianzahl), `hash` = Inhalts-Fingerabdruck der Gruppe, `keep` = behaltener Ordner, `keep_fingerprint` (Metadaten des behaltenen Ordners, siehe unten) und `source_fingerprint` (dieselben Metadaten des entfernten Ordners); `reason` ist `exact-duplicate-dir`, bzw. `exact-duplicate-dir:copy-name`, wenn der Name entschieden hat. Es wird von oben nach unten gearbeitet (Gruppen mit der größeren Verschachtelungstiefe des Inhalts zuerst, nicht nach Pfadtiefe): Unterordner entfernter Ordner entfallen, nichts innerhalb eines behaltenen Ordners wird entfernt, und ein behaltener Ordner verliert nie einen Unterordner durch eine andere Gruppe. Geschützte Ordner (auch mit geschütztem Inhalt) werden weder entfernt noch behalten. Hardlinks auf den behaltenen Ordner geben keinen Platz frei (`hardlink`); der Plan nennt den tatsächlich freiwerdenden Platz. `--min-size` lässt kleine Gruppen weg.

**Apply:** Der zu entfernende Ordner wird wie bei `archive` per Metadaten-Walk geprüft (Dateianzahl, Größensumme, jüngste Änderungszeit) und zusätzlich gegen `source_fingerprint` (relativer Pfad, Größe, Änderungszeit der nicht ignorierten Dateien; so fällt auch eine umbenannte Datei auf); sonst `stale`. Pläne ohne `source_fingerprint` (ältere Dateien) werden nur über Anzahl, Summe und jüngste Zeit geprüft. Der behaltene Ordner wird gegen `keep_fingerprint` geprüft (relativer Pfad, Größe, Änderungszeit der nicht ignorierten Dateien, ohne Hashen): fehlt er `keep-missing`, hat er sich geändert `keep-changed`; dann wird nichts entfernt. Danach folgt **ein** Rename in die Quarantäne; scheitert er (gesperrte Datei), bleibt der Ordner vollständig liegen. Undo, Purge und die OneDrive-Obergrenze (zählt die Dateianzahl) funktionieren wie bei `archive`.

### 7.2 plan junk: Müll

```
ordner-cleanup plan junk <pfad> [--category system,temp,downloads,installer,<eigene>]
```

| Kategorie | Trifft zu auf |
|---|---|
| `system` | `Thumbs.db`, `ehthumbs.db`, `.DS_Store` |
| `temp` | `*.tmp`, `~$*` |
| `downloads` | `*.crdownload`, `*.part`, `*.partial` |
| `installer` | `*.exe`, `*.msi` direkt im Downloads-Ordner, älter als `installer_min_age` (Default 90 Tage) |

- Ohne `--category` gelten `junk_categories` aus der Config, sonst alle eingebauten.
- Den Downloads-Ordner ermittelt Windows über die Known-Folder-API (auch bei umgeleitetem Ordner). `downloads_dirs` in der Config ersetzt ihn. Liegt kein Downloads-Ordner unter der Wurzel, liefert `installer` nichts, und `plan` sagt das.
- **Eigene Kategorien** werden in der Config über `[[junk_rules]]` definiert (siehe [Konfiguration](#9-konfiguration)) und mit `--category <name>` oder über `junk_categories` aktiviert.

### 7.3 plan empty-dirs: Leere Ordner

```
ordner-cleanup plan empty-dirs <pfad>
```

- **Leer** heißt: vollständig im Index bekannt und rekursiv ohne jede Datei, auch ohne versteckte, System- und Cloud-Dateien.
- Ausgeschlossene oder nur aufsummierte Unterordner, Links und Ordner mit Lesefehler beim Scan gelten **nie** als leer.
- Die Wurzel und `_Archiv` selbst bleiben stehen.
- Aktionen laufen von unten nach oben, sodass verschachtelt leere Ordner vollständig verschwinden.
- `apply` entfernt einen Ordner nur, wenn er in diesem Moment wirklich leer ist (`RemoveDirectory`); sonst wird die Aktion `stale`.
- Das Journal hält Attribute und Zeitstempel **vor** dem Lauf fest, damit `undo` auch Elternordner mit ihrer ursprünglichen Zeit wiederherstellt.
- Ein Ordner, in dem nur noch Müll liegt (z. B. `Thumbs.db`), gilt nicht als leer. `plan empty-dirs` weist darauf hin; deshalb zuerst `plan junk`, dann neu scannen, dann `plan empty-dirs`.

### 7.4 plan archive: Archivieren statt löschen

```
ordner-cleanup plan archive <pfad> [--older-than 2y]
```

Verschiebt Ordner, deren **jüngste Datei** älter ist als `--older-than` (Default `archive_older_than`, sonst `2y`), nach `<wurzel>\_Archiv\<Jahr>\<relativer Pfad>`.

- Es zählt nur die Änderungszeit (mtime), nie der Zugriffszeitpunkt.
- Geplant wird immer der **oberste** passende Ordner, nicht zusätzlich seine Unterordner. Einzeldateien werden nicht archiviert.
- Ordner mit Cloud-Platzhaltern, Links, Geschütztem (auch tief darin) oder unvollständigem Index werden nicht als Ganzes verschoben und erscheinen als übersprungen.
- `apply` prüft Dateianzahl, Größensumme und jüngste mtime des Ordners per Metadaten-Walk (es wird nichts geöffnet) und verschiebt ihn mit **einem** Rename.
- Ein vorhandenes Ziel wird nie überschrieben oder zusammengeführt. Ein `_Archiv`, das selbst ein Link ist, wird nie betreten.

### 7.5 plan versions: Versionswildwuchs

```
ordner-cleanup plan versions <pfad> [--min-age 30d]
```

- Eine **Versionsgruppe** besteht aus Dateien **desselben Ordners** mit gleichem normalisierten Namen (`_v2`, `final`, `- Kopie (3)`, Datumsangaben, siehe [5.5](#55-abschnitte-des-berichts)) und gleicher Endung.
- Die Datei mit der **jüngsten mtime** bleibt (`keep` steht im Plan; bei Gleichstand der lexikografisch letzte Pfad). Die älteren wandern nach `<wurzel>\_Archiv\Versionen\<relativer Pfad>`.
- Ältere Versionen, die jünger sind als `--min-age` (Default `versions_min_age`, sonst `30d`), bleiben liegen und erscheinen als `too-recent`.
- Ist die jüngste Datei ein Cloud-Platzhalter, wird die ganze Gruppe übersprungen.
- `apply` verlangt, dass die behaltene Datei noch lokal existiert und nicht älter geworden ist.
- Dateien unter `_Archiv` sind keine Kandidaten; ein zweiter Lauf plant deshalb nichts mehr.

---

## 8. Sicherheitsmechanismen

| Mechanismus | Wirkung |
|---|---|
| **Dry-Run als Standard** | `plan` verändert nichts; nur `apply` schreibt |
| **Bestätigung** | `apply`, `undo`, `purge` fragen `j/N`; ohne Terminal brechen sie ohne `--yes` ab |
| **Stale-Prüfung** | Jede Aktion prüft unmittelbar vor der Ausführung, ob die Datei noch dem Plan entspricht |
| **Quarantäne statt Löschen** | Verschieben per Rename auf demselben Volume; Zeitstempel und Inhalt bleiben erhalten; kein Verschieben über Laufwerksgrenzen |
| **Kein Überschreiben** | Belegte Ziele bekommen ein Suffix (`datei (2).txt`); `undo` überschreibt nie |
| **Write-ahead-Journal** | `<wurzel>\.ordner-cleanup\journal\<run-id>.jsonl`, `fsync` je Eintrag, vor jeder Änderung |
| **Lauf-Register** | `%LOCALAPPDATA%\ordner-cleanup\runs.jsonl`, damit `undo` die Wurzel findet (maßgeblich bleibt das Journal; sonst `--root`) |
| **Idempotenz** | Ein zweiter `apply` desselben Plans ändert nichts |
| **Geschützte Pfade** | Siehe unten |
| **OneDrive-Schutz** | Siehe unten |

### Geschützte Pfade

Diese Pfade werden nie angefasst. `apply` prüft das unabhängig vom Plan noch einmal und erkennt auch manipulierte Pläne, `..`, `\\?\` und 8.3-Kurznamen.

- **Immer, nicht abschaltbar:** `C:\Windows`, `Program Files`, `Program Files (x86)`, `ProgramData`
- **Standardmäßig:** `AppData`, `.git`, `node_modules` und Projektordner mit `Cargo.toml`, `package.json`, `*.sln` oder `*.csproj`
- **Zusätzlich:** eigene Pfade über `protected_paths` in der Config

Liegt die angegebene Wurzel selbst unter `AppData`, gilt nur diese eine Regel nicht.

### OneDrive

- Cloud-Platzhalter und Links (Symlinks/Junctions) werden nie gelesen, gehasht oder verschoben.
- Liegt die Wurzel in OneDrive, wird auch die Quarantäne mitsynchronisiert; `plan` und `apply` warnen davor.
- **Obergrenze:** Bewegt ein Plan unter einer OneDrive-Wurzel mehr als **1.000 Dateien oder 5 GB** (Ordner-Moves zählen mit Dateianzahl und Größensumme, `remove-dir` zählt nicht), verweigert `apply` vor der ersten Änderung, bis `--allow-large` gesetzt ist. Das gilt für alle Plan-Arten. `plan` weist vorher darauf hin. Die Grenzen sind über `onedrive_max_move_files` und `onedrive_max_move_bytes` änderbar.

---

## 9. Konfiguration

Datei: `%APPDATA%\ordner-cleanup\config.toml`. Alle Schlüssel sind optional; eine fehlende Datei ergibt die Defaults. CLI-Flags haben Vorrang (Listen aus Flags werden an die Config-Listen angehängt). Unbekannte Schlüssel und ungültige Werte (Dauern, Größen, Globs, Kategorienamen) führen schon beim Laden zu einer Fehlermeldung.

### Alle Schlüssel

| Schlüssel | Default | Bedeutung |
|---|---|---|
| `exclude` | `[]` | Zusätzliche Ausschluss-Globs für `scan` |
| `summary_only` | `[]` | Globs für „Nur Summe“ |
| `no_default_excludes` | `false` | Eingebaute Ausschlüsse abschalten |
| `threads` | `0` | Scan-Threads (`0` = automatisch) |
| `old_after` | `"1y"` | Schwelle für „alte“ Dateien im Bericht |
| `top` | `50` | Länge der Top-Listen |
| `max_depth_warning` | `8` | Verschachtelungstiefe, ab der ein Ordner als zu tief gilt |
| `huge_dir_entries` | `1000` | Direkte Einträge, ab denen ein Ordner als Riesenordner gilt |
| `onedrive_conflict_hostnames` | `[]` | Weitere Rechnernamen für die Erkennung von OneDrive-Konfliktdateien |
| `protected_paths` | `[]` | Zusätzliche geschützte Ordner |
| `quarantine_days` | `30` | Aufbewahrungsdauer; `purge` löscht nur ältere Läufe (mindestens 1) |
| `junk_categories` | alle eingebauten | Aktive Kategorien für `plan junk` |
| `installer_min_age` | `"90d"` | Mindestalter für Installer in Downloads |
| `downloads_dirs` | `[]` | Ersetzt den Windows-Downloads-Ordner, wenn nicht leer |
| `archive_older_than` | `"2y"` | Default für `plan archive --older-than` |
| `versions_min_age` | `"30d"` | Default für `plan versions --min-age` |
| `dedupe_dirs_copy_patterns` | `[]` | Zusätzliche Wörter/Muster, die einen Ordnernamen als Kopie kennzeichnen (`plan dedupe-dirs`) |
| `dedupe_dirs_partial_threshold` | `0.8` | Ab diesem Anteil gemeinsamer Bytes meldet der Bericht Ordner als „teilweise gleich“ (0,5 bis 1,0) |
| `onedrive_max_move_files` | `1000` | OneDrive-Obergrenze: Dateianzahl |
| `onedrive_max_move_bytes` | `"5GB"` | OneDrive-Obergrenze: Größe |

### Beispiel

```toml
exclude = ["*.iso", "D:\\VMs\\*"]
protected_paths = ["D:\\Wichtig"]
quarantine_days = 60
old_after = "2y"
junk_categories = ["system", "temp", "downloads", "installer", "logs"]
installer_min_age = "60d"
archive_older_than = "3y"

# [[junk_rules]] muss NACH allen flachen Schlüsseln stehen,
# sonst gehören diese Schlüssel zur Tabelle.
[[junk_rules]]
name = "logs"                 # darf keine eingebaute Kategorie sein, muss eindeutig sein
patterns = ["*.log"]          # Glob auf den Dateinamen, Groß-/Kleinschreibung egal
min_age = "30d"               # optional
dirs = ["D:\\Logs"]           # optional: nur direkt in diesen Ordnern
```

### Umgebungsvariable

`ORDNER_CLEANUP_HOME=<ordner>` verlegt Index, Config und Register in einen eigenen Ordner. Das ist nützlich zum Ausprobieren, ohne die echten Daten zu berühren (die Tests nutzen es ebenfalls).

---

## 10. Dateien und Speicherorte

| Ort | Inhalt |
|---|---|
| `%LOCALAPPDATA%\ordner-cleanup\index.db` | SQLite-Index aller Scans |
| `%LOCALAPPDATA%\ordner-cleanup\runs.jsonl` | Lauf-Register für `undo` und `runs` |
| `%APPDATA%\ordner-cleanup\config.toml` | Konfiguration |
| `<wurzel>\.ordner-cleanup\quarantine\<run-id>\…` | Quarantäne eines Laufs (Struktur wie im Original) |
| `<wurzel>\.ordner-cleanup\journal\<run-id>.jsonl` | Journal eines Laufs |
| `<wurzel>\_Archiv\<Jahr>\…` | Archivierte Ordner (`plan archive`) |
| `<wurzel>\_Archiv\Versionen\…` | Archivierte ältere Versionen (`plan versions`) |
| `ordner-cleanup-report-<zeitstempel>\` | Ein Bericht (HTML, JSON, CSV) |
| `plan-<zeitstempel>.json` | Plan-Datei, wenn `--out` fehlt |

`scan` erfasst den Ordner `.ordner-cleanup` nie.

**Neben den Programmdateien** (`ordner-cleanup.exe`, `ordner-cleanup-bg.exe`, `ordner-cleanup-gui.exe`) liegen `README.md` und `docs\dokumentation.md`. Die Hilfe der Oberfläche („?“ an Seiten und Karten, `F1`) öffnet sie über „Ausführliche Doku öffnen“. Fehlen die Dateien, zeigt die Oberfläche den erwarteten Pfad an; die Kurztexte funktionieren auch ohne sie.

---

## 11. Exit-Codes

| Code | Bedeutung |
|---|---|
| `0` | Alles erledigt |
| `1` | Fehler oder Abbruch (auch „N“ bei der Rückfrage, Strg+C beim Scan) |
| `2` | Erfolg mit Teilfehlern: z. B. Zugriff verweigert beim Scan, unvollständiger Scan im `report`, bei `apply`/`undo` übersprungene oder `stale` Aktionen, Fehler einzelner Aktionen, Kollisionen beim `undo` |
| `3` | Plan wurde nach dem Erstellen verändert (Integritätsprüfung bei `apply` fehlgeschlagen); nichts ausgeführt |

---

## 12. Fehlersuche und FAQ

**„Kein Scan für … vorhanden“:** `report` und `plan` brauchen vorher `scan` auf demselben Pfad. `index list` zeigt, was im Index steht.

**`apply` bricht ohne Rückfrage ab:** In einer Sitzung ohne Terminal (Skript, Pipe) muss `--yes` gesetzt sein.

**„Plan wurde nach dem Erstellen verändert“ (Exit-Code `3`):** Der Inhalt der Plan-Datei passt nicht mehr zu ihrer Prüfsumme, z. B. weil sie in einem Editor geändert, von einem Sync-Konflikt ersetzt oder nur halb geschrieben wurde. Es wurde nichts ausgeführt. Den Plan prüfen: War die Änderung gewollt, mit `plan seal <plan.json>` neu versiegeln; sonst einen neuen Plan erzeugen. Reine Formatierung (Einrückung, Zeilenenden) löst den Fehler nicht aus. Bekannte Lücke: Wer `version` auf `2` zurücksetzt und `integrity` löscht, bekommt nur die Warnung „ungeschützter Plan“; die Prüfung schützt vor Versehen, nicht vor Absicht.

**Viele Aktionen sind `stale`:** Die Dateien haben sich seit dem Plan geändert. Neu scannen und neuen Plan erzeugen.

**`plan empty-dirs` findet weniger Ordner als erwartet:** Ordner mit Rest-Müll (`Thumbs.db` …) gelten nicht als leer. Erst `plan junk` und `apply`, dann `scan`, dann `plan empty-dirs`. Ordner mit Lesefehler beim Scan oder ausgeschlossene Unterordner gelten nie als leer.

**`apply` verweigert wegen OneDrive-Obergrenze:** Der Plan bewegt mehr als 1.000 Dateien oder 5 GB in einem OneDrive-Ordner, was viel Sync-Traffic auslöst. Entweder den Plan aufteilen (Teilordner) oder bewusst `--allow-large` setzen.

**Eine Datei fehlt nach `apply`:** Sie liegt unter `<wurzel>\.ordner-cleanup\quarantine\<run-id>\…` bzw. in `_Archiv`. `undo <run-id>` bringt sie zurück.

**`undo` meldet eine Kollision:** Am ursprünglichen Ort liegt inzwischen eine andere Datei. Sie wird nicht überschrieben; die Datei bleibt in der Quarantäne und kann manuell geholt werden.

**`undo` findet den Lauf nicht:** `--root <wurzel>` angeben (das Register fehlt oder ist unvollständig).

**Index defekt oder veraltet:** `scan <pfad> --reset-index` baut ihn neu auf. Einzelne Wurzeln entfernt `index remove <pfad>`.

**Netzlaufwerk wird abgelehnt:** Nur mit `scan --force`; offiziell wird das nicht unterstützt.

**Scan ist langsam:** Der Erstscan hängt vom Hashen der Duplikat-Kandidaten ab; Re-Scans sind deutlich schneller. Mit `--exclude` bzw. `--summary-only` große irrelevante Ordner auslassen.

**Werden Dateien durch den Scan verändert?** Nein. Dateien werden nur lesend geöffnet. Lediglich der Last-Access-Zeitstempel kann sich ändern. OneDrive-Cloud-Dateien bleiben Cloud-only.

---

## 13. Entwicklung

```
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
cargo run -- scan <pfad>
```

Die CI (GitHub Actions, `windows-latest`) prüft `cargo fmt --check`, Clippy mit `-D warnings`, alle Tests, dass keine Netzwerk-Crates (`reqwest`, `hyper`, `ureq`, `curl`) im Abhängigkeitsbaum stehen, und baut den Release.

### Aufbau des Quellcodes

| Modul | Aufgabe |
|---|---|
| `cli.rs` | Befehle und Optionen (clap) |
| `app/` | Orchestrierung der Befehle (`scan`, `report`, `plan`, …) |
| `config.rs` | Laden und Validieren der `config.toml` |
| `scan/` | Walker, Klassifikation (Ausschlüsse), Hashing, Duplikatsuche |
| `index/` | SQLite-Index (Schema, Speichern, Abfragen) |
| `analysis/` | Dateitypen, Alter, ähnliche Dateien, Strukturprobleme, Problemdateien |
| `report/` | Berichtsmodell und Ausgabe (Terminal, HTML, JSON, CSV) |
| `change/` | Planer (`dedupe`, `dedupe_dirs`, `junk`, `empty_dirs`, `archive`, `versions`), `apply`, `undo`, Journal, Quarantäne, Schutzregeln, Register |
| `platform/` | Windows-spezifisches (Attribute, File-ID, Known Folders, Cloud-Erkennung) |
| `paths.rs` | Pfad-Normalisierung (`\\?\`, Schlüssel, Vergleich) |
| `templates/`, `assets/` | HTML-Vorlage, CSS und JS des Berichts |
| `tests/` | End-to-End-Tests der CLI |

### Performance (Release, SSD, 100.000 Dateien)

| Lauf | Dauer | Spitzen-RAM |
|---|---|---|
| Erstscan inkl. Duplikatsuche | 33 s | 79 MB |
| Re-Scan ohne Änderungen | 5,6 s | 106 MB |
| Report (alle Formate) | 0,6 s | 87 MB |
| `plan junk` / `empty-dirs` / `archive` / `versions` | 0,33 s / 0,11 s / 2,2 s / 3,9 s | – |

Die Planer-Messung ist als ignorierter Test abgelegt: `cargo test --release --test perf_plans -- --ignored --nocapture`. Einen Testbaum erzeugt `cargo run --release --example gen-tree -- <zielordner> [anzahl]`.

### Arbeitsweise im Repo

Features entstehen über den Workflow `/feature:spec` → `/feature:implementation-plan` → `/feature:code`, Bugs über `/debug:analysis` → `/debug:fix-plan` → `/debug:fix` (siehe `CLAUDE.md`). Es wird auf dem Branch `dev` gearbeitet; `main` ist der Release-Branch.

---

## 14. Weiterführende Dokumente

| Thema | Spec | Implementierungsplan |
|---|---|---|
| Phase 1: Scan & Analyse-Bericht | [features/scan-analyse-bericht.md](features/scan-analyse-bericht.md) | [implementation-plans/scan-analyse-bericht.md](implementation-plans/scan-analyse-bericht.md) |
| Phase 2: Änderungsplan, Apply & Undo | [features/aenderungsplan-apply-undo.md](features/aenderungsplan-apply-undo.md) | [implementation-plans/aenderungsplan-apply-undo.md](implementation-plans/aenderungsplan-apply-undo.md) |
| Phase 3: Aufräumaktionen | [features/aufraeumaktionen.md](features/aufraeumaktionen.md) | [implementation-plans/aufraeumaktionen.md](implementation-plans/aufraeumaktionen.md) |

Ausblick und Priorisierung (Regel-Engine, Umbenennen, Watch-Mode, GUI): [roadmap.md](roadmap.md). Kurzfassung für den Einstieg: [../README.md](../README.md).

## Inhaltsklassifikation (Phase 6a)

`classify` liest Inhalt und Metadaten (PDF, Office, EXIF, Video, OCR, optional ein lokales LLM) und legt Kategorie und Felder im Index-Cache ab; `plan rules` und der Bericht nutzen sie. Befehle, Konfiguration, Kategorien, neue Regelbedingungen und Platzhalter, OCR, LLM und Datenschutz stehen im [README, Abschnitt „Inhalte klassifizieren“](../README.md#inhalte-klassifizieren-phase-6a).
