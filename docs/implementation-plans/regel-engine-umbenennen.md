---
title: "Implementation Plan: Regel-Engine & Umbenennen (Phase 4)"
feature_spec: docs/features/regel-engine-umbenennen.md
status: pending-approval   # pending-approval | approved | implemented
created: 2026-10-04
updated: 2026-10-04
---

# Implementation Plan: Regel-Engine & Umbenennen (Phase 4)

## Summary
Phase 4 bringt ein neues, reines Modul `src/rules/`, das die Regeldatei lädt und prüft: TOML mit Zeilenangaben, Bedingungen, eine Ziel-Vorlage mit Platzhaltern und die Normalisierung. Dazu kommt ein neuer Planer `change::rules::plan_rules`, der wie alle Planer nur den Index liest. Für `{exif.date}` liest er zusätzlich den EXIF-Header von Bildern. Die EXIF-Daten werden im Index zwischengespeichert, was ein Index-Schema v2 mit Migration erfordert. Ausgeführt und zurückgedreht wird über die vorhandene Aktion `move`.

`apply` und `undo` bekommen drei Erweiterungen:
- **Neues Ziel `Dest::Rules`:** Das Ziel liegt irgendwo unter der Wurzel statt nur unter `_Archiv`.
- **Neuer Journal-Eintrag `CreatedDir`:** Er protokolliert jeden neu angelegten Zielordner. `undo` entfernt dann genau diese Ordner, wenn sie leer sind, und nie Ordner, die vorher schon da waren. Der Archiv-Helfer `cleanup_empty_parents` wäre hier falsch, weil er bis einschließlich zur Wurzel aufräumt.
- **Umbenennen, das nur die Groß-/Kleinschreibung ändert:** Es wird ausdrücklich erkannt und behandelt, weil `exists(ziel)` dann die Quelle selbst findet.

Die Umsetzung geht von innen nach außen: Regeldatei, EXIF, Plan-Modell, Planer, Apply, Undo, CLI. Jeder Schritt ist für sich testbar.

## Entscheidungen aus Spec und Codeanalyse
| Thema | Entscheidung |
|---|---|
| Regelformat | TOML, `[[rules]]`, flach; `toml::Spanned` für Zeilenangaben |
| Ort der Regeldatei | Neben `config.toml` in `paths::config_dir()`, also `%APPDATA%\ordner-cleanup\rules.toml` bzw. `<ORDNER_CLEANUP_HOME>\config\rules.toml`. Überschreibbar per Config (`rules_file`) und `--rules`. **Präzisiert die Spec**, dort steht verkürzt `<ORDNER_CLEANUP_HOME>\rules.toml`. |
| Plan-Format | `version: 2`, neuer `kind: rules`, Aktion `move` mit optionalem Feld `rule` |
| Ziel prüfen | `validate()` je `kind`: Bei `rules` liegt das Ziel unter der Wurzel, aber nicht unter `_Archiv` und nicht unter `.ordner-cleanup`. `archive`/`versions` bleiben unverändert. |
| Journal | `Dest::Rules` (Serde-Name `rules`) und neuer Eintrag `CreatedDir` |
| Ziel entsteht zwischen `plan` und `apply` | Übersprungen mit dem vorhandenen Grund `target-exists`, wie bei `archive`. **Präzisiert die Spec**: Dort heißt es „stale“, das Verhalten ist dasselbe (kein Überschreiben). |
| Nur Groß-/Kleinschreibung | Quelle und Ziel haben denselben `path_key`, und `file_identity` liefert dieselbe Datei. Dann ist es ein echtes Umbenennen. Zuerst klärt ein Spike (Schritt 0), ob `MoveFileExW` ohne `REPLACE_EXISTING` das auf NTFS in **einem** Schritt kann. Ja: ein Schritt, und die Spec-Zeile wird angepasst. Nein: zwei Schritte über einen temporären Namen, mit Feld `via` im `Intent` (siehe Risiken). |
| EXIF | `kamadak-exif`. Bevorzugt wird `DateTimeOriginal`, sonst `DateTimeDigitized`. Lesen nur über einen Reader mit Obergrenze (16 MiB). Vorher wird live geprüft, ob die Datei Cloud-only ist. |
| EXIF-Cache | Eigene Tabelle `exif_cache` (Schlüssel `path_key`, gültig bei gleicher Größe und mtime), Index-Schema v2. v1 wird beim Öffnen migriert, ohne `--reset-index`. |
| Datum | `{year}`, `{month}`, `{day}` und `iso_date_prefix` nutzen die mtime in **lokaler Zeit**. EXIF-Zeiten sind ohne Zeitzone und werden unverändert übernommen. |
| Inhaltsvergleich bei belegtem Ziel | Nur bei gleicher Größe. Der Index-Hash wird genutzt, wenn er gültig ist, sonst wird live gehasht. Ist ein Ziel Cloud-only, gilt es als „anderer Inhalt“ und wird durchnummeriert, ohne es zu lesen. |
| Quellen | Nur Dateien außerhalb von `_Archiv` und `.ordner-cleanup`. Links werden übersprungen (`link`), Cloud-only-Dateien erst nach dem Regel-Treffer (`cloud-placeholder`). |
| Stabilitätsprüfung | Der Planer wendet die Regeln probehalber auf jedes Ziel an. Würde eine Datei beim nächsten Lauf erneut bewegt, gibt es einen Hinweis je Regelpaar, zum Beispiel bei `./{parent}_{name}` oder bei Ketten von Regel A zu Regel B. |

## Files to Create
| File | Purpose |
|------|---------|
| `src/rules/mod.rs` | `RuleSet::load(path)` / `RuleSet::parse(text, source_name)`, `RulesError` mit Datei, Zeile, Regelname, `RuleSet::select(&[name])` für `--rule` |
| `src/rules/file.rs` | Rohstrukturen für TOML (`deny_unknown_fields`, `Spanned`), Umwandlung in geprüfte `Rule` |
| `src/rules/condition.rs` | `Conditions` (Glob auf den relativen Pfad mit `/`, `ext`, `name_regex`, Alter, Größe) und `matches(&Candidate) -> Option<Captures>` |
| `src/rules/template.rs` | Vorlage beim Laden in Tokens zerlegen (`Literal`, `Name`, `Ext`, `Parent`, `Year`, `Month`, `Day`, `Exif(fmt)`, `Group(n)`, Escape `{{`/`}}`). Art des Ziels: relativ zur Wurzel, `./` oder mit Endung `/`. `expand(&Values) -> Result<Vec<String>, TargetError>` (Segmente). |
| `src/rules/normalize.rs` | `strip_copy_suffix`, `umlauts`, `spaces`, `lowercase_ext`, `iso_date_prefix`. Erhält die Groß-/Kleinschreibung und ist idempotent. |
| `src/scan/exif.rs` | `read_taken(path) -> Result<Option<NaiveDateTime>, ExifError>`: Live-Prüfung auf Cloud-only, `LimitedReader` (Read + Seek + BufRead, Fehler ab 16 MiB), `kamadak-exif`. Testhelfer `testing::jpeg_with_date(..)` baut ein minimales JPEG mit EXIF im Code. |
| `src/index/exif_cache.rs` | `exif_lookup(&[(key, size, mtime)]) -> HashMap<key, Option<i64>>`, `exif_store(..)` in einer Transaktion, `exif_prune(root_key)` für Einträge ohne Datei |
| `src/change/rules.rs` | Planer `plan_rules(index, root, &RuleSet, &Protector, &dyn ExifSource, created, now) -> Result<RulesPlan, IndexError>` und Trait `ExifSource` (echt: Cache + `scan::exif`; Tests: Map) |
| `tests/cli_rules.rs` | Integrationstests über die echte CLI: Plan → Apply → Undo, Idempotenz, Fehler in der Regeldatei, Groß-/Kleinschreibung, OneDrive-Obergrenze |

## Files to Modify
| File | Change Description |
|------|--------------------|
| `Cargo.toml` | `kamadak-exif` (aktuelle Version per `cargo add`) |
| `src/lib.rs` | `pub mod rules;` |
| `src/change/mod.rs` | `pub mod rules;`. `SkipReason` bekommt `NoExifDate`, `DuplicateAtTarget`, `InvalidTarget`, `PathTooLong` und `UnstableTarget` (Letzteres nur, falls nötig; siehe Schritt 4), jeweils mit Display-Text. `ActionCounts` bekommt `sorted` (Einsortieren/Umbenennen), `count(action, kind)` und angepasste Texte. |
| `src/change/plan.rs` | `PlanKind::Rules`. `PlannedAction.rule: Option<String>` (Serde-Default, nicht serialisiert wenn leer). `validate_fields` prüft `Move` je `kind`: Bei `rules` liegt das Ziel unter der Wurzel, nicht unter `_Archiv` oder `.ordner-cleanup`, und Ziel = Quelle ist nur erlaubt, wenn sich die Schreibweise unterscheidet. Für alle Arten gilt neu: Das Ziel darf nicht die Quelle einer anderen Aktion sein. |
| `src/change/journal.rs` | `Dest::Rules`. `Entry::CreatedDir { run, action, path }`, geschrieben **vor** `create_dir`. `Entry::run()` wird ergänzt. |
| `src/change/apply.rs` | `prepare_archive_parent` wird zu `prepare_parent(stop, parent, …)` verallgemeinert: Ordner werden einzeln von oben nach unten angelegt, je neuem Ordner erst `CreatedDir`, dann `create_dir`, und jede Ebene muss ein echter Ordner sein. `verify_move` je `Dest`, mit Ausnahme für das Umbenennen nur in der Schreibweise. `process_move` schreibt `dest` passend zur Plan-Art. Die Zielprüfung „Pfad zu lang“ nutzt `PathTooLong` (260) für `rules`. |
| `src/change/undo.rs` | `OpKind::Move` mit `Dest::Rules`: erlaubter Bereich ist die Wurzel. Umbenennen nur in der Schreibweise wird erkannt (gleicher Schlüssel, gleiche Identität). `CreatedDir`-Einträge werden gesammelt und nach allen Rückbewegungen in umgekehrter Reihenfolge mit `remove_dir` entfernt, nur wenn leer; Fehler werden ignoriert, Links nie angefasst. `summarize` zählt `sorted`. |
| `src/change/fsops.rs` | Trait um `same_file(a, b) -> io::Result<bool>` (über `file_identity`) erweitern. Bei `FaultyFs` wird `Op::CreateDir` schon unterstützt, dazu kommt ein Absturz nach `create_dir`. |
| `src/change/limits.rs` | Keine Logik-Änderung. Ein Test belegt, dass `rules`-Moves zählen. |
| `src/index/mod.rs`, `src/index/schema.sql` | `SCHEMA_VERSION = 2`, Tabelle `exif_cache`. `init` migriert v1 auf v2 (`CREATE TABLE IF NOT EXISTS` und Update von `meta`) in einer Transaktion. Unbekannte Versionen bleiben ein `SchemaMismatch`. |
| `src/analysis/problems.rs` | `pub fn name_issue(name) -> Option<Problem>` aus `name_problems` herauslösen (ungültige Zeichen, reservierte Namen, Leerzeichen bzw. Punkt am Ende), damit der Planer es wiederverwenden kann |
| `src/config.rs` | `rules_file: Option<String>`. `parse_bytes` wird `pub(crate)`, damit `min_size`/`max_size` dieselbe Syntax nutzen. |
| `src/cli.rs` | `PlanCommand::Rules(PlanRulesArgs { path, rules: Option<PathBuf>, rule: Vec<String>, out })` |
| `src/app/plan.rs` | `plan_rules_command`: Regeldatei auflösen, laden und prüfen **vor** dem Öffnen des Index, `--rule` prüfen, Planer, Zusammenfassung je Regel mit Vorschau (bis zu 3 Beispiele „alt → neu“ relativ zur Wurzel), Anzahl ohne Treffer, Stabilitätshinweise; danach `finish` |
| `src/app/mod.rs` | Ausgaben von `apply`, `undo` und `runs` über die erweiterten `ActionCounts` („N Dateien einsortiert/umbenannt“) |
| `README.md` | Abschnitt `plan rules` mit Beispiel-`rules.toml`, Platzhaltern, Normalisierung, Hinweis auf Stabilität und Umbenennen nur in der Schreibweise |
| `docs/roadmap.md` | Status Phase 4 |
| `docs/features/regel-engine-umbenennen.md` | Die drei Präzisierungen aus der Tabelle oben. Am Ende `status: implemented` und „Manueller Test“. |

## Data Model Changes

### Regeldatei (`rules.toml`)
```text
RawRuleFile { rules: Vec<Spanned<RawRule>> }               // deny_unknown_fields
RawRule { name, glob?, ext?: Vec, name_regex?, min_age?, max_age?, min_size?, max_size?,
          target, exif_fallback?: "mtime"|"skip",
          iso_date_prefix?: bool, spaces?: "_"|"-", umlauts?: bool,
          strip_copy_suffix?: bool, lowercase_ext?: bool }

Rule { name, line, conditions: Conditions, target: Template, exif_fallback, normalize: Normalize }
RuleSet { source: PathBuf, rules: Vec<Rule> }
```
Die Regeldatei wird beim Laden vollständig geprüft. Jeder der folgenden Fälle ist ein `RulesError` mit Datei, Zeile und Regelname:
- TOML-Syntax ungültig (Zeile aus dem `toml`-Span)
- unbekannter Schlüssel
- keine Regeln, oder eine Regel ohne Bedingung
- doppelter Name (ohne Rücksicht auf Groß-/Kleinschreibung)
- ungültiger Glob, ungültige Regex, ungültige Alters- oder Größenangabe
- `min` größer als `max`
- unbekannter Platzhalter, `{n}` größer als die Anzahl der Regex-Gruppen, `{n}` ohne `name_regex`
- ungültiges `chrono`-Format (`StrftimeItems` enthält `Item::Error`)
- Ziel absolut, mit Laufwerksbuchstaben, UNC-Pfad oder einem Literal-Segment `.`/`..`
- leere Vorlage

### Plan (JSON, `version: 2`)
```text
Plan.kind: … | rules
Plan.params: { rules_file, rules: "fotos,rechnungen" (nur bei --rule) }
PlannedAction (move): target unter der Wurzel, rule: Some(name), reason: "rule:<name>"
Skipped.reason neu: no-exif-date, duplicate-at-target, invalid-target, path-too-long
```

### Journal (JSON Lines)
```text
{"t":"intent", …, "dest":"rules"}                         // Ziel unter der Wurzel
{"t":"created_dir","run":…,"action":7,"path":"C:\\…\\Fotos\\2026"}   // vor create_dir
```
- Journale aus Phase 2/3 bleiben lesbar (neue Felder bzw. Einträge sind optional). Eine **ältere** Programmversion kann ein Phase-4-Journal mit `created_dir` nicht lesen. Ein Downgrade ist kein Ziel.

### Index (SQLite, Schema v2)
```sql
CREATE TABLE exif_cache (
    path_key TEXT PRIMARY KEY,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    taken INTEGER              -- Aufnahmezeit als lokale Sekunden seit Epoche; NULL = kein EXIF
);
```
- Ein Treffer gilt nur, wenn `size` und `mtime` mit der Datei im Index übereinstimmen.
- Der Cache überdauert Re-Scans, weil er eine eigene Tabelle ist und nicht an `files.generation` hängt.
- `exif_prune` entfernt am Ende von `plan rules` Einträge unter der Wurzel, zu denen es keine Datei mehr gibt.

### Config
```toml
rules_file = 'D:\Regeln\rules.toml'   # optional; Default: rules.toml neben config.toml
```

## API / Interface Changes

### CLI
```text
ordner-cleanup plan rules <pfad> [--rules <datei>] [--rule <name>]… [--out <plan.json>]
```
- **Exit-Codes** wie bisher: 0 Plan geschrieben, 1 bei Fehlern in der Regeldatei, fehlender Datei oder unbekanntem `--rule`, ohne Plan-Datei.
- **Fehlende Default-Regeldatei:** Die Meldung nennt den erwarteten Pfad und zeigt ein Beispiel mit zwei Regeln.
- **Zusammenfassung:**
  - Gesamtzahl der Aktionen und Bytes
  - je Regel die Anzahl und bis zu 3 Beispiele „alt → neu“
  - Dateien ohne passende Regel
  - übersprungene Einträge nach Grund
  - Stabilitätshinweise
  - OneDrive-Hinweise wie bisher

### Bibliothek (Auszug)
```rust
// rules
pub struct RuleSet { pub source: PathBuf, pub rules: Vec<Rule> }
impl RuleSet {
    pub fn load(path: &Path) -> Result<Self, RulesError>;
    pub fn parse(text: &str, source: &Path) -> Result<Self, RulesError>;
    pub fn select(self, names: &[String]) -> Result<Self, RulesError>;   // --rule
}
pub struct Candidate<'a> { pub rel: &'a str /* mit '/' */, pub name: &'a str,
                           pub size: u64, pub mtime_ticks: i64 }
impl Rule {
    pub fn matches(&self, c: &Candidate, now_ticks: i64) -> Option<Captures>;
    pub fn needs_exif(&self) -> bool;                      // Ziel oder iso_date_prefix
    pub fn target(&self, c: &Candidate, caps: &Captures, exif: Option<NaiveDateTime>)
        -> Result<RelTarget, TargetError>;               // Segmente relativ zur Wurzel
}

// change::rules
pub trait ExifSource: Sync { fn taken(&self, file: &FileRow) -> ExifResult; }  // Date | None | Cloud
pub struct RulesPlan { pub plan: Plan, pub bytes: u64,
                       pub per_rule: Vec<RuleStats>,     // name, actions, bytes, examples
                       pub unmatched: usize, pub notes: Vec<String> }

// index
impl Index { pub fn exif_lookup(..); pub fn exif_store(..); pub fn exif_prune(..); }

// fsops
fn same_file(&self, a: &Path, b: &Path) -> io::Result<bool>;
```

### Ablauf im Planer
1. `files_under(root)`, ohne `_Archiv` und `.ordner-cleanup`, sortiert nach `path_key`.
2. Für jede Datei gewinnt die erste passende Regel, sonst zählt sie als „ohne Treffer“. Bei einem Treffer:
   - Geschützt, Link oder Cloud-only führt zu `skipped`.
3. Wer EXIF braucht, wird gesammelt. Dann:
   - erst im Cache nachsehen
   - fehlende Einträge parallel lesen (`rayon`)
   - den Cache in **einer** Transaktion schreiben
4. Das Ziel wird gebildet:
   - Vorlage einsetzen und Segmente prüfen (nicht leer, nicht `.`/`..`, `name_issue`)
   - Dateinamen normalisieren
   - zu einem Pfad unter der Wurzel zusammensetzen
   - Länge prüfen: über 260 Zeichen gibt `path-too-long`
5. `{exif.date}` ohne Datum: Bei `fallback = mtime` wird die mtime eingesetzt, bei `skip` gibt es `no-exif-date`.
6. Ziele werden in fester Reihenfolge vergeben (Quellen nach `path_key`):
   - Ist das Ziel die Quelle selbst, entsteht keine Aktion.
   - Unterscheidet es sich nur in der Groß-/Kleinschreibung, wird umbenannt.
   - Belegt sind alle Dateien und Ordner im Index sowie Ziele, die schon vergeben wurden.
   - Ein Ordner im Ziel ist im Index eine Datei: `invalid-target`.
   - Ziel belegt mit gleichem Inhalt: `duplicate-at-target`.
   - Ziel belegt mit anderem Inhalt: ` (2)`, ` (3)` … anhängen, bis der Name frei ist **oder der Quelle selbst entspricht**. Letzteres sorgt für Idempotenz nach dem Durchnummerieren.
7. Geschütztes Ziel, Ziel unter `_Archiv` oder unter `.ordner-cleanup`: `protected`.
8. Stabilitätsprüfung: Für jedes Ziel werden die Regeln erneut angewandt, mit neuem Pfad und derselben Größe, mtime und EXIF. Ergäbe das ein anderes Ziel, kommt eine Notiz zum Regelpaar.

## Implementation Sequence
0. **Spikes** (Wegwerf-Tests im Scratchpad bzw. als nicht committeter `#[ignore]`-Test, ohne Produktivcode):
   - (a) `move_no_replace("a.JPG", "a.jpg")` auf NTFS in einem Temp-Ordner. Klappt es in einem Schritt? Was liefert `file_identity` für beide Schreibweisen?
   - (b) API von `kamadak-exif`: `Reader::read_from_container` mit `LimitedReader`, JPEG, PNG (`eXIf`), WebP, HEIC. Wie verhält es sich bei abgeschnittenen Daten?
   - (c) Liefert `toml` 0.9 `Spanned`-Offsets für Elemente von `[[rules]]`?
   - Das Ergebnis kommt als kurzer Absatz in diesen Plan, unter „Risks“.
   - Depends on: nothing
1. **Regeldatei** — `rules::{file, condition, template, normalize}` und `RuleSet::parse` streng nach TDD. Tests:
   - jeder Fehlerfall aus „Data Model Changes“ mit Zeile und Regelname
   - Glob mit `/` und `\`, ohne Rücksicht auf Groß-/Kleinschreibung, `**` über mehrere Ebenen
   - `ext` ohne Rücksicht auf Groß-/Kleinschreibung, Grenzen von Alter und Größe
   - Regex-Gruppen
   - Escape `{{`/`}}`
   - Ziel mit `./` und mit `/` am Ende
   - jede Normalisierung, auch Groß-/Kleinschreibung erhalten
   - **Idempotenz**: `n(n(x)) == n(x)` als `rstest`-Tabelle
   - Spec-Beispiel `Rechnung März - Kopie (2).PDF` wird zu `2026-09-30_Rechnung_Maerz.pdf`
   - `RuleSet::select` mit unbekanntem Namen
   - Files: `src/rules/*`, `src/lib.rs`, `src/config.rs` (`parse_bytes`)
   - Depends on: 0
2. **EXIF lesen und Cache** — `scan::exif::read_taken` mit `LimitedReader`, Live-Prüfung auf Cloud-only vor dem Öffnen, `DateTimeOriginal` vor `DateTimeDigitized`, kaputte Daten ergeben `Ok(None)`. Index-Schema v2 mit Migration, `exif_cache`-Funktionen. Tests:
   - JPEG mit und ohne Datum, abgeschnittene Datei, Nicht-Bild, Datei über der Obergrenze
   - simulierter Platzhalter (OFFLINE-Attribut) wird nicht geöffnet
   - v1-Index-Datei wird migriert und behält Dateien, Hashes und Läufe
   - Cache ungültig nach Änderung von Größe oder mtime
   - Prune
   - Files: `src/scan/exif.rs`, `src/scan/mod.rs`, `src/index/{mod.rs, schema.sql, exif_cache.rs}`, `Cargo.toml`
   - Depends on: 0
3. **Plan-Modell** — `PlanKind::Rules`, `rule`-Feld, Validierung von `move` je `kind`, Regel „Ziel ≠ fremde Quelle“, neue `SkipReason`s, `ActionCounts.sorted`. Tests:
   - Roundtrip, und ein Plan v2 aus Phase 3 lädt unverändert
   - Ziel unter der Wurzel ist für `rules` gültig, für `archive` ungültig
   - Ziel unter `_Archiv` oder `.ordner-cleanup` ist für `rules` ungültig
   - Ziel nur in anderer Groß-/Kleinschreibung ist gültig, identisches Ziel ungültig
   - Ziel ist Quelle einer anderen Aktion: ungültig
   - Files: `src/change/plan.rs`, `src/change/mod.rs`
   - Depends on: nothing (parallel zu 1–2)
4. **Planer `plan_rules`** — Ablauf wie oben, mit In-Memory-Index und `ExifSource`-Fake. Tests (je ein Akzeptanzkriterium der Spec):
   - Die erste passende Regel gewinnt.
   - `--rule` filtert.
   - Bedingungen sind UND-verknüpft.
   - Platzhalter einschließlich `{parent}` und Gruppen.
   - EXIF vor mtime, Fallback `mtime`/`skip`.
   - Datei schon am Ziel ergibt keine Aktion.
   - Groß-/Kleinschreibung wird erkannt.
   - Duplikat am Ziel.
   - Durchnummerieren gegen den Index und innerhalb des Plans, mit fester Reihenfolge (zweimal geplant ergibt einen identischen Plan).
   - Nach dem Durchnummerieren ist der nächste Lauf leer.
   - Ziel kollidiert mit einem Ordner bzw. ein Ordner im Ziel ist eine Datei.
   - Ungültige Zeichen, reservierte Namen, `..` aus einer Regex-Gruppe, Länge über 260.
   - Geschützte Quelle bzw. geschütztes Ziel, Ziel unter `_Archiv`.
   - Quelle Cloud-only oder Link.
   - `_Archiv` als Quelle ausgenommen.
   - Stabilitätshinweis bei `./{parent}_{name}` und bei einer Kette von Regel A zu Regel B.
   - EXIF-Cache: Ein zweiter Lauf ruft `ExifSource` nicht für unveränderte Dateien auf. Dafür wird über `ExifSource` gezählt; die echte Implementierung kapselt den Cache.

   Prüfen, ob `UnstableTarget` als `SkipReason` nötig ist. Voraussichtlich nicht: Es reicht eine Notiz, und die Aktion bleibt im Plan.
   - Files: `src/change/rules.rs`, `src/change/mod.rs`, `src/analysis/problems.rs`
   - Depends on: 1, 2, 3
5. **Apply** — `prepare_parent` mit `CreatedDir` (write-ahead), Ziele nach `Dest`, Umbenennen nur in der Schreibweise je nach Spike-Ergebnis, `PathTooLong`. Mit `FaultyFs` und handgebauten Plänen testen:
   - Datei wird verschoben, und der Zielordner wird angelegt und protokolliert.
   - Vorhandener Zielordner bekommt keinen `CreatedDir`-Eintrag.
   - Ordner auf dem Zielweg ist eine Junction: abgelehnt.
   - Ziel ist inzwischen belegt: `target-exists`.
   - Quelle geändert: stale.
   - Umbenennen nur in der Schreibweise klappt, ohne `target-exists`.
   - Gesperrte Datei: Fehler, der Lauf geht weiter.
   - Absturz nach `CreatedDir` vor `create_dir`, und Absturz nach dem Rename.
   - Archiv-Moves verhalten sich unverändert (die vorhandenen Tests bleiben grün).
   - Files: `src/change/apply.rs`, `src/change/journal.rs`, `src/change/fsops.rs`
   - Depends on: 3
6. **Undo und `runs`** — `Dest::Rules` mit Bereich Wurzel, Umbenennen nur in der Schreibweise zurück, Aufräumen der `CreatedDir`-Ordner (nur wenn leer, von unten nach oben, nie die Wurzel, nie Links, nie Ordner ohne `CreatedDir`), Zählung in `runs`. Tests:
   - Apply und Undo stellen Pfad, Name, Größe, mtime, Attribute und Inhalt wieder her.
   - Vorher vorhandener **leerer** Ordner bleibt nach Undo erhalten.
   - Angelegter Ordner, in den der Nutzer inzwischen etwas gelegt hat, bleibt erhalten.
   - Ursprungspfad belegt: Konflikt.
   - Zweites Undo ändert nichts.
   - Manipuliertes Journal mit `dest: rules` und Ziel bzw. `created_dir` außerhalb der Wurzel wird abgelehnt.
   - Absturz-Fälle.
   - `purge` lässt `rules`-Läufe undo-fähig.
   - Files: `src/change/undo.rs`, `src/change/journal.rs`, `src/app/mod.rs`
   - Depends on: 5
7. **CLI, Config, Ausgabe** — `PlanRulesArgs`, `rules_file`, `plan_rules_command` mit Zusammenfassung je Regel, Vorschau, Stabilitäts- und OneDrive-Hinweisen; echte `ExifSource` aus Index-Cache und `scan::exif`. Fehlermeldungen für fehlende bzw. kaputte Regeldatei. Tests in `app` für Pfadauflösung (`--rules` vor Config vor Default) und Formatierung der Vorschau.
   - Files: `src/cli.rs`, `src/config.rs`, `src/app/plan.rs`, `src/app/mod.rs`
   - Depends on: 4, 6
8. **Integrationstests** `tests/cli_rules.rs` (echte Temp-Ordner, `ORDNER_CLEANUP_HOME` isoliert):
   - `scan` → `plan rules`: Der Baum bleibt byteidentisch.
   - `apply --yes` ergibt den erwarteten Baum.
   - Idempotenz: `scan` → `plan rules` ergibt 0 Aktionen.
   - `undo --yes` stellt den Ausgangszustand wieder her, einschließlich mtime und Attributen; neu angelegte Ordner sind weg.
   - JPEG mit EXIF-Datum aus dem Testhelfer, ohne Binär-Fixture.
   - Regeldatei mit Fehler: Exit 1 mit Zeilenangabe, kein Plan.
   - `--rule` unbekannt.
   - Umbenennen nur in der Schreibweise Ende-zu-Ende.
   - Obergrenze über `OneDrive`-Umgebungsvariable.
   - Manipulierter Plan mit Ziel außerhalb der Wurzel.
   - Files: `tests/cli_rules.rs`
   - Depends on: 7
9. **Performance und Doku** — `tests/perf_plans.rs` um `plan rules` erweitern:
   - 100.000 Dateien, 10 Regeln, ohne EXIF: unter 10 s
   - 10.000 JPEGs aus dem Testhelfer: erster Lauf messen und dokumentieren, zweiter Lauf aus dem Cache unter 2 s

   Dazu README und Roadmap, `cargo clippy -- -D warnings`, `cargo fmt`.
   - Files: `tests/perf_plans.rs`, `README.md`, `docs/roadmap.md`
   - Depends on: 8
10. **Manueller Test** — Ergebnis in der Spec unter „Manueller Test“:
    - OneDrive-Testordner und ein echter Alltagsordner (Downloads), jeweils mit 5 bis 10 Regeln
    - Ablauf: `scan` → `plan rules` → Vorschau prüfen → `apply` → `scan` → `plan rules` (leer) → `undo`
    - Vorher/Nachher-Vergleich (Pfad, Größe, mtime, Attribute, SHA256) wie in Phase 3
    - Echte Fotos mit EXIF (JPEG und HEIC vom Handy)
    - Umbenennen nur in der Schreibweise unter OneDrive: Kommt der Sync korrekt an?
    - Depends on: 9

## Test Strategy
- **Unit:**
  - Regeldatei: jeder Fehler mit Zeile, alle Bedingungen, Vorlage, Normalisierung einschließlich Idempotenz
  - `scan::exif` mit generierten JPEGs und kaputten Daten
  - Index-Migration v1 → v2, `exif_cache`
  - Plan-Validierung je `kind`
  - Planer mit In-Memory-Index und `ExifSource`-Fake (alle Akzeptanzkriterien der Spec zum Planen, feste Reihenfolge, Stabilitätshinweise)
- **Apply/Undo mit `FaultyFs`:** `CreatedDir` write-ahead, Absturz zwischen Journal und `create_dir` bzw. Rename, Umbenennen nur in der Schreibweise hin und zurück, vorher vorhandene leere Ordner bleiben stehen, Konflikte, manipulierte Journale.
- **Integration (`tests/cli_rules.rs`):** Plan → Apply → Re-Scan → leerer Plan → Undo, byteidentisch; Fehlerpfade der CLI.
- **Rückwärtskompatibilität:** Alle Tests aus Phase 1–3 bleiben unverändert grün. Plan-Fixtures v1/v2 und Journale aus Phase 2/3 werden gelesen. Eine v1-Index-Datei wird migriert.
- **Performance:** `tests/perf_plans.rs` (wie Phase 3, `#[ignore]` + manueller Lauf).
- **E2E/Manuell:** Schritt 10. Zusätzlich der seit Phase 2 offene Test mit echtem Cloud-only-Platzhalter, falls der OneDrive-Client läuft: Er darf kein EXIF-Lesen und keinen Download auslösen.
- Vorgehen nach `test-driven-development`, vor `status: implemented` `verification-before-completion`.

## Risks & Open Questions
- **Umbenennen nur in der Groß-/Kleinschreibung:** Unklar ist, ob `MoveFileExW` ohne `REPLACE_EXISTING` das in einem Schritt erlaubt (Spike 0a). Sonst braucht es zwei Schritte: zuerst `Intent` mit `via: <temporärer Name>`, dann Rename zum temporären und danach zum endgültigen Namen. Undo und Absturz-Recovery müssen dann die drei möglichen Orte prüfen (Quelle, temporärer Name, Ziel). Mehr Aufwand, aber eingegrenzt. Unter OneDrive kann ein solches Umbenennen außerdem schlecht synchronisiert werden; das wird im manuellen Test geprüft und bei Bedarf in der README vermerkt.
- **Aufräumen nach Undo:** `cleanup_empty_parents` darf bei `rules` nicht genutzt werden, weil es vorher vorhandene leere Ordner und im Extremfall die Wurzel entfernen würde. Abhilfe: Nur Ordner mit `CreatedDir` werden entfernt. Ein eigener Test deckt einen vorher vorhandenen leeren Ordner ab.
- **Idempotenz:** Das Durchnummerieren, `iso_date_prefix` und `strip_copy_suffix` können zusammen hin und her wechseln. Zum Beispiel würde `x (2).pdf` erst abgeschnitten und dann wieder nummeriert. Abhilfe: Die Nummernsuche akzeptiert die eigene Quelle als „frei“. Dazu kommen Tabellentests für `n(n(x)) == n(x)`, der Integrationstest „zweiter Plan leer“ und die Stabilitätsprüfung im Planer, die nicht stabile Regeln vor dem `apply` meldet.
- **Index-Schema v2:** Eine ältere Programmversion verlangt danach `--reset-index`. Akzeptiert, da es keine verteilten Installationen gibt; vermerkt in der README.
- **Inhaltsvergleich am Ziel:** Bei gleicher Größe wird gehasht, was bei großen Videos teuer ist. Abhilfe: Gültige Hashes aus dem Index werden genutzt; sonst gibt es einen Fortschrittshinweis. Cloud-only-Ziele werden nie gelesen.
- **EXIF und Zeitzone:** EXIF-Zeiten haben keine Zeitzone, die mtime wird lokal umgerechnet. Damit können Datei- und Aufnahmedatum um einen Tag abweichen. Das ist dokumentiertes Verhalten; `OffsetTimeOriginal` bleibt in Phase 4 unberücksichtigt.
- **EXIF-Parser:** Fremde Bilddaten sind nicht vertrauenswürdig. `kamadak-exif` ist reines Rust, gelesen wird mit Obergrenze, und Fehler gelten als „kein EXIF“. Zusätzlich wird `catch_unwind` um den Parser gelegt, damit ein Panic nie den Lauf beendet.
- **Zeilennummern bei semantischen Fehlern:** Hängt von `Spanned` in `toml` 0.9 ab (Spike 0c). Rückfall: Angegeben werden Regelname und Position der Regel (n-te `[[rules]]`), ohne Zeile.
- [ ] Ergebnis der Spikes 0a bis 0c hier eintragen, bevor Schritt 5 beginnt.
