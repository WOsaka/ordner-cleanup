---
title: "Implementation Plan: Duplikate ganzer Ordner (plan dedupe-dirs)"
feature_spec: docs/features/ordner-duplikate.md
status: pending-approval   # pending-approval | approved | implemented
created: 2026-10-09
updated: 2026-10-09
---

# Implementation Plan: Duplikate ganzer Ordner (`plan dedupe-dirs`)

## Summary
Die Erkennung wird ein reiner Analyse-Baustein `analysis::folder_dups`. Er arbeitet nur auf dem Index: auf dem bestehenden `change::tree::Tree` (Ordner, Dateien, Vollständigkeit, Links, Cloud) und einer neuen Abfrage der vollen Hashes. Ein Merkle-Fingerabdruck wird von unten nach oben berechnet, aus (Name, Größe, Hash) bzw. den Fingerabdrücken der Unterordner. Ignorierte Junk-Dateien und leere Unterordner zählen nicht mit. Ordner mit gleichem Fingerabdruck bilden Gruppen. Bericht und Planer nutzen denselben Baustein. Der neue Planer `change::dedupe_dirs` wählt je Gruppe den behaltenen Ordner (Namens-Heuristik, dann `--keep`) und arbeitet die Gruppen von oben nach unten ab, damit Unterordner bereits entfernter Ordner entfallen und nichts innerhalb eines behaltenen Ordners entfernt wird. Er erzeugt `quarantine`-Aktionen mit `is_dir: true`. Apply bekommt einen Ordner-Zweig in der Quarantäne-Verarbeitung: Prüfung der Quelle wie bei `archive` (Dateianzahl, Summe, jüngste mtime per Metadaten-Walk), Prüfung des behaltenen Ordners gegen einen Metadaten-Fingerabdruck (`keep_fingerprint`), dann ein einziges Rename in die Quarantäne. Undo und Purge funktionieren unverändert, weil sie Ordner-Moves schon kennen (Archiv). Danach folgen CLI, `ops`, Profile, Bericht (HTML/JSON/CSV/Terminal), GUI-Karte und Hilfe, Doku und die Erweiterung des Rundreise-Tests.

## Files to Create
| File | Purpose |
|------|---------|
| `src/analysis/folder_dups.rs` | Reine Erkennung: Ignorier-Regel, Merkle-Fingerabdruck je Ordner, exakte Gruppen, teilweise gleiche Paare, Namens-Heuristik (`copy_name_score`). Keine Abhängigkeit zu `change::protect` |
| `src/change/dedupe_dirs.rs` | Planer `plan_dedupe_dirs`: Schutzregeln, Wahl des behaltenen Ordners, Abarbeitung von oben nach unten, Längenprüfung, Platzgewinn, Plan-Aufbau. Unit-Tests mit In-Memory-Index wie in `archive.rs` |
| `tests/cli_dedupe_dirs.rs` | Ende-zu-Ende über die CLI: scan → plan → apply → undo, Akzeptanzkriterien der Spec |

## Files to Modify
| File | Change Description |
|------|--------------------|
| `src/analysis/mod.rs` | `pub mod folder_dups;` |
| `src/index/dups.rs` | Neue Abfrage `file_hashes_under(dir_key) -> HashMap<path_key, FileHash>` (voller Hash, Identität, nlinks; nur `hash_status = 'ok'`, lokal, kein Link) |
| `src/change/mod.rs` | `pub mod dedupe_dirs;`; `ActionCounts` um `dirs_quarantined` erweitern (Texte „N Ordner in die Quarantäne verschieben/verschoben“, Kurzform „N Ordner (Quarantäne)“); `count()` bekommt `is_dir`; neue `SkipReason::KeepChanged` („behaltener Ordner seit dem Plan geändert“) und `SkipReason::Unverifiable` („Inhalt nicht prüfbar (Cloud-only oder ohne Hash)“) |
| `src/change/plan.rs` | `PlanKind::DedupeDirs` (`dedupe-dirs`); optionales Feld `keep_fingerprint` in `PlannedAction`; Validierung: `quarantine` + `is_dir` nur in `dedupe-dirs`, dann sind `files`, `hash`, `keep`, `keep_hash`, `keep_fingerprint` Pflicht; `keep_fingerprint` sonst verboten; Aktionspfad darf nicht in/über einem `keep` liegen; `dependents()` deckt Ordner-Quarantäne bereits über `is_dir` ab (Test ergänzen) |
| `src/change/apply.rs` | `process_quarantine` verzweigt bei `is_dir`: `verify_quarantine_dir` (Schutz, Existenz, Link/Cloud, `summarize_dir` gegen `files`/`size`/`mtime_ticks`, behaltener Ordner per `keep_dir_fingerprint` gegen `keep_fingerprint`, Ergebnis je Keep gecacht), Ziel per `quarantine_target`, Intent mit `is_dir: true`, ein Rename |
| `src/change/quarantine.rs` | `fits_dir(root, dir, longest_relative_len)`: auch die tiefste Datei im Ordner muss unter `MAX_TARGET_LEN` passen |
| `src/change/test_support.rs` | `dir_dedupe_plan(&[(dup, keep)])` für Apply-/Undo-Unit-Tests |
| `src/change/undo.rs` | Keine Logik-Änderung erwartet; Tests: Undo und Purge eines Ordner-Quarantäne-Laufs |
| `src/config.rs` | `dedupe_dirs_copy_patterns: Vec<String>` (Zusatzmuster), `dedupe_dirs_partial_threshold: f64` (Default 0.8, gültig 0.5 bis 1.0); `PROFILE_PLAN_KINDS` um `dedupe-dirs` (7 Einträge); Validierung |
| `src/settings/fields.rs` | Beide Config-Felder in den GUI-Einstellungen |
| `src/cli.rs` | `PlanCommand::DedupeDirs(PlanDedupeDirsArgs { path, profile, keep, min_size, out })`; `--min-size` per `bytesize::ByteSize::from_str` |
| `src/app/mod.rs`, `src/app/plan.rs` | Dispatch und `plan_dedupe_dirs_command` wie `plan_dedupe_command` |
| `src/app/run.rs` | Profil-Lauf kennt `"dedupe-dirs"` |
| `src/ops/plan.rs` | `PlanKindRequest::DedupeDirs { keep, min_size }`; Kopfzeile „N doppelte Ordner, X freiwerdend, M übersprungen (Strategie: …)“; Hinweis-Note „Erst doppelte Ordner bereinigen, dann einzelne Duplikate (`plan dedupe`)“ |
| `src/report/mod.rs`, `src/report/build.rs` | Neuer Abschnitt `folder_duplicates: FolderDuplicatesReport { groups, total_reclaimable, partial, partial_threshold }`; Aufbau über `analysis::folder_dups` (ohne Schutzregeln, wie Datei-Duplikate) |
| `src/report/csv.rs`, `src/report/json.rs`, `src/report/terminal.rs`, `src/report/html.rs`, `templates/report.html` | `folder_duplicates.csv` und `folder_partial.csv`; JSON über serde; eine Terminal-Zeile; HTML-Abschnitt „Ordner-Duplikate“ (Gruppen nach Platzgewinn, „teilweise gleich“ Top 50, Empfehlungssatz) |
| `src/gui/views/cleanup_cards.rs` | 7. Karte `dedupe-dirs` „Doppelte Ordner“ **vor** „Duplikate“; `CardFacts.dedupe_dirs: Option<(u64, u64)>` aus dem Report |
| `src/gui/views/cleanup.rs` | Zuordnung `PlanKind::DedupeDirs` ↔ `"dedupe-dirs"` ↔ `PlanKindRequest::DedupeDirs` (Strategie-Auswahl wie bei Duplikate) |
| `src/gui/views/apply.rs` | Text für `PlanKind::DedupeDirs` („die Quarantäne“) |
| `src/gui/review.rs` | Ordner-Einträge: Dateianzahl und behaltener Ordner in der Zeile/Tooltip |
| `src/gui/views/analysis_tabs.rs` | Reiter „Duplikate“: Liste „Doppelte Ordner“ und „Teilweise gleich“ aus dem Report, Knopf „Plan für doppelte Ordner“ |
| `src/gui/help.rs`, `src/gui/texts.rs` | Hilfetext der Karte und Review-Hinweis |
| `src/notify.rs` | Nur falls Plan-Arten aufgezählt werden: `dedupe-dirs` ergänzen |
| `tests/roundtrip_random.rs` | `dedupe-dirs` in `ACTIONS`; Zufallsbaum legt Ordnerkopien an (auch mit zusätzlichem `Thumbs.db`/`desktop.ini`, verschachtelt) |
| `tests/perf_plans.rs` | Messung `plan dedupe-dirs` im bestehenden Muster |
| `tests/gui_cleanup_cards.rs` | Sieben Karten, neue Reihenfolge |
| `README.md`, `docs/dokumentation.md`, `docs/manual-tests.md`, `docs/roadmap.md` | Befehl, Optionen, Beispiele, Handtest, Status |

## Data Model Changes

**Plan-Format (bleibt v2):**
- `PlanKind::DedupeDirs` → JSON `"kind": "dedupe-dirs"`.
- `PlannedAction.keep_fingerprint: Option<String>` (32 Hex, xxh3-128), `skip_serializing_if = "Option::is_none"`. Da `PlannedAction` `deny_unknown_fields` hat, lehnen ältere Programmversionen solche Pläne als „beschädigt“ ab, statt sie falsch auszuführen (gewollt laut Spec).
- Ordner-Aktion: `type: quarantine`, `is_dir: true`, `path` = Ordner, `size` = Summe aller Dateien (inkl. ignorierter), `mtime_ticks` = jüngste mtime aller Dateien, `files` = Anzahl aller Dateien, `hash` = `keep_hash` = Inhalts-Fingerabdruck der Gruppe, `keep` = behaltener Ordner, `keep_fingerprint` = Metadaten-Fingerabdruck des behaltenen Ordners, `reason` = `exact-duplicate-dir` bzw. `exact-duplicate-dir:copy-name` (Heuristik hat entschieden).
- `Plan.keep_strategy` wie bei `dedupe`; `params`: `min_size` (falls gesetzt).

**Fingerabdrücke (in `analysis::folder_dups`):**
- *Inhalt* (Merkle, unabhängig vom Ordnernamen): je Ordner xxh3-128 über die nach Schlüssel sortierte Liste der Kinder: Datei → `F|<name kleingeschrieben>|<size>|<voller Hash oder "0" bei size 0>`, Unterordner → `D|<name kleingeschrieben>|<Inhalts-Fingerabdruck>`. Ignoriert werden Junk-Dateien (siehe unten) und Unterordner mit leerem effektivem Inhalt. Ein Ordner ist **nicht vergleichbar** (`None`), wenn der Baum ihn nicht vollständig kennt (`Tree::complete`), er Links oder Cloud-only-Dateien enthält oder eine nicht ignorierte Datei mit Größe > 0 keinen vollen Hash hat. Nicht vergleichbar vererbt sich nach oben.
- *Metadaten des behaltenen Ordners* (für Apply): xxh3-128 über sortierte `relativer Pfad kleingeschrieben|size|mtime_ticks` aller nicht ignorierten Dateien. Dieselbe Funktion rechnet beim Planen (aus dem Index) und beim Apply (Metadaten-Walk über `FsOps`, ohne Dateien zu öffnen). Sie liegt in `analysis::folder_dups` und bekommt die Einträge übergeben.

**Ignorier-Regel beim Vergleich:** `junk::is_builtin_junk_name(name)` (Thumbs.db, ehthumbs.db, .DS_Store, `*.tmp`, `~$*`, `*.crdownload`, `*.part`, `*.partial`) plus `desktop.ini`. Installer und eigene `junk_rules` zählen bewusst nicht dazu, damit das Ergebnis nicht von der Config abhängt.

**Index:** keine Schemaänderung. Neue Abfrage `file_hashes_under` liest vorhandene Spalten (`full_hash`, `hash_status`, `volume_serial`, `file_index`, `nlinks`).

**Report:**
```rust
pub struct FolderDuplicatesReport {
    pub groups: Vec<FolderDupGroup>,   // sortiert nach reclaimable absteigend
    pub total_reclaimable: u64,
    pub partial: Vec<PartialPair>,     // vollständig; HTML zeigt Top 50
    pub partial_threshold: f64,
}
pub struct FolderDupGroup { pub dirs: Vec<String>, pub files: u64, pub bytes: u64, pub reclaimable: u64 }
pub struct PartialPair { pub a: String, pub b: String, pub shared_bytes: u64, pub ratio: f64 }
```
`reclaimable` im Bericht = `bytes × (Anzahl − 1)` ohne Schutzregeln (wie bei Datei-Duplikaten); der Plan nennt den echten Wert nach Schutz und Hardlinks.

**Config:** `dedupe_dirs_copy_patterns = []` (zusätzliche Wörter/Muster, case-insensitiv), `dedupe_dirs_partial_threshold = 0.8`.

## API / Interface Changes
- **CLI:** `ordner-cleanup plan dedupe-dirs [<wurzel>] [--profile <name>] [--keep oldest|newest|path:<prefix>] [--min-size <größe>] [--out <datei>]`. Ausgabe wie die übrigen Planer plus Empfehlungssatz. Exit-Codes unverändert.
- **Profile:** `plans = ["dedupe-dirs", …]` erlaubt; geplante Läufe erzeugen auch diesen Plan (kein Auto-Apply).
- **Bibliothek:**
  - `analysis::folder_dups::analyze(tree: &Tree, hashes: &HashMap<String, FileHash>, opts: &FolderDupOptions) -> FolderDupAnalysis` mit `groups: Vec<ExactGroup { members: Vec<DirKey>, fingerprint: u128, files, bytes }>` und `partial: Vec<PartialPair>`
  - `analysis::folder_dups::copy_name_score(name: &str, extra: &[String]) -> u8` (0 = normaler Name; höher = eher Kopie)
  - `analysis::folder_dups::meta_fingerprint(entries: impl Iterator<Item = (rel, size, mtime)>) -> u128`
  - `change::dedupe_dirs::plan_dedupe_dirs(index, root, strategy, protector, created, &DedupeDirsOptions { min_size, copy_patterns }) -> Result<DedupePlan, IndexError>` (Rückgabetyp von `dedupe` wiederverwenden: Plan + `freed_bytes`)
  - `ops::plan::PlanKindRequest::DedupeDirs { keep: KeepStrategy, min_size: Option<u64> }`
- **Apply/Journal:** keine neuen Journal-Einträge; `Intent { dest: Quarantine, is_dir: true }` gibt es bisher nur für Archiv-Moves und funktioniert in Undo/Purge bereits.

## Fachliche Algorithmen (Kurzform)

**Gruppen und Auswahl (Planer):**
1. `Tree` und `file_hashes_under` laden; `analyze` liefert exakte Gruppen (≥ 2 Mitglieder, ≥ 1 effektive Datei mit Größe > 0). Wurzel und `.ordner-cleanup` sind nie Mitglied.
2. Gruppen nach Tiefe des flachsten Mitglieds aufsteigend sortieren (oben zuerst), bei Gleichstand nach Bytes absteigend, dann Pfad.
3. Je Gruppe die Mitglieder filtern: entfällt, wenn es in einem bereits **entfernten** Ordner liegt (still, ohne Skip-Eintrag); `protected`, wenn es Geschütztes enthält (`contains_protected` aus `archive.rs` wiederverwenden, dazu in eine gemeinsame Hilfsfunktion in `tree.rs` heben); `too-long`, wenn `fits_dir` scheitert (nur für zu entfernende). Mitglieder **in einem behaltenen Ordner** dürfen nur behalten, nie entfernt werden.
4. Behalten: unter den zulässigen Mitgliedern minimal nach (`path:`-Präfix-Treffer zuerst, falls Strategie `path:`), `copy_name_score` aufsteigend, dann `--keep` (jüngste Datei-mtime im Ordner), kürzester Pfad, alphabetisch. Bei `path:` ohne Treffer → alle `group-incomplete`.
5. Zu entfernen: alle übrigen zulässigen Mitglieder außerhalb behaltener Ordner. Sind sie vollständig Hardlinks auf den behaltenen Ordner (jede Datei teilt die Identität mit der Datei am gleichen relativen Pfad), dann `hardlink`. Bleibt nichts übrig, entfällt die Gruppe.
6. `--min-size`: Gruppen mit effektiven Bytes darunter entfallen ohne Skip-Eintrag.
7. Entfernte Ordner und behaltene Ordner merken (für Schritt 3 der folgenden Gruppen). Platzgewinn wie in `dedupe.rs` über Datei-Identitäten.
8. Nicht vergleichbare Ordner, die sonst gleich wären, lassen sich nicht erkennen (kein Fingerabdruck). Der Bericht nennt deshalb die Zahl nicht vergleichbarer Ordner mit Grund (`unverifiable`) nur als Zusammenfassung, nicht je Ordner.

**Teilweise gleich (nur Bericht):**
- Über die Datei-Duplikatgruppen (aus `file_hashes_under`): je Hash-Gruppe mit höchstens `MAX_PARTIAL_GROUP` Mitgliedern (Startwert 32; größere Gruppen wie 5000 gleiche Lizenzdateien werden für die Paarbildung übersprungen) für jedes Dateipaar die Ordner **gleicher relativer Tiefe** bis unter den gemeinsamen Vorfahren paaren. Je Ordnerpaar zählt `min(Anzahl in A, Anzahl in B) × Größe` je Hash.
- `ratio = shared / min(effektive Bytes A, B)`; Hinweis ab Schwelle, exakte Gruppen-Paare ausgenommen, nur oberste Paare (Paar entfällt, wenn das Elternpaar ebenfalls über der Schwelle liegt), Paare unter 1 MiB gemeinsamer Bytes entfallen im HTML (in CSV/JSON vollständig).
- Speicherschutz: Zahl der Ordnerpaare wird auf 200 000 begrenzt; bei Überschreitung bricht die Paarbildung ab und der Bericht vermerkt „unvollständig“.

**Apply eines Ordners:**
1. `env.protector.check(src)`/`check_inside(src)` → `protected`; fehlt der Ordner → `already-done`; Cloud/Link/kein Ordner → wie `verify_move`.
2. `summarize_dir` (bestehend) gegen `files`/`size`/`mtime_ticks` → sonst `stale`.
3. Behaltener Ordner: Metadaten-Walk ohne Öffnen der Dateien; fehlt oder ist Link/Cloud → `keep-missing`; `meta_fingerprint` ≠ `keep_fingerprint` → `keep-changed`. Ergebnis je Keep-Pfad cachen (mehrere Kopien eines Ordners).
4. Ziel per `quarantine_target` (Volume-Prüfung, freier Name), Intent schreiben, ein `rename` des Ordners. Scheitert es (gesperrte Datei), bleibt der Ordner vollständig; Ergebnis `failed` mit Grund.

## Implementation Sequence
1. **Analyse-Kern** — Ignorier-Regel, Merkle-Fingerabdruck, Vergleichbarkeit, exakte Gruppen, `copy_name_score`, `meta_fingerprint`; Unit-Tests auf handgebauten `Tree`-Daten (gleich trotz anderem Ordnernamen, Thumbs.db/desktop.ini ignoriert, Groß-/Kleinschreibung, ein Byte Unterschied, leere Unterordner, Cloud/Link/ohne Hash → nicht vergleichbar, nur Junk/nur leere Dateien → kein Kandidat).
   - Files: `src/analysis/folder_dups.rs`, `src/analysis/mod.rs`, `src/index/dups.rs`
   - Depends on: nothing
2. **Plan-Format** — `PlanKind::DedupeDirs`, `keep_fingerprint`, Validierungsregeln, neue `SkipReason`s, `ActionCounts.dirs_quarantined`; Tests in `plan.rs`/`mod.rs` (Rundreise JSON, Pflichtfelder, verbotene Kombinationen, Aktion in/über `keep`, alter Plan ohne Feld bleibt gültig, Version-1-Pläne unverändert).
   - Files: `src/change/plan.rs`, `src/change/mod.rs`
   - Depends on: nothing (parallel zu 1)
3. **Planer** — `plan_dedupe_dirs` mit Auswahl, Abarbeitung von oben nach unten, Schutz, Hardlinks, Längen, `--min-size`, Platzgewinn; `contains_protected` nach `tree.rs` verschieben und in `archive.rs` weiterverwenden; `quarantine::fits_dir`. Unit-Tests mit In-Memory-Index für alle Planer-Kriterien der Spec (Heuristik vor Alter, drei Kopien, nur oberste Ebene, Mitglied im behaltenen Ordner wird nie entfernt, geschützt, `path:` ohne Treffer, Hardlinks, zu lang).
   - Files: `src/change/dedupe_dirs.rs`, `src/change/tree.rs`, `src/change/archive.rs`, `src/change/quarantine.rs`
   - Depends on: Steps 1, 2
4. **Apply** — Ordner-Zweig in `process_quarantine`, Keep-Prüfung per Metadaten-Fingerabdruck mit Cache; Tests mit `Fx`/`FaultyFs`: Happy Path inkl. Journal-Intent `is_dir`, Quelle geändert → `stale`, behaltener Ordner geändert/fehlt → `keep-changed`/`keep-missing`, gesperrte Datei → `failed` ohne Teilverschiebung, Junction im Ordner → `link`, zweiter Apply → `already-done`.
   - Files: `src/change/apply.rs`, `src/change/test_support.rs`
   - Depends on: Step 2 (Fingerabdruck aus Step 1)
5. **Undo/Purge absichern** — Tests: Undo stellt Ordner byteidentisch her (inkl. mtime), Kollision bei neu angelegtem gleichnamigem Ordner, Purge löscht den Ordner aus der Quarantäne, `runs`-Zeile nennt „N Ordner (Quarantäne)“.
   - Files: `src/change/undo.rs` (nur Tests), `src/change/mod.rs`
   - Depends on: Step 4
6. **Config, CLI, ops, Profile** — Config-Felder und Validierung, `PROFILE_PLAN_KINDS`, `PlanDedupeDirsArgs`, `PlanKindRequest::DedupeDirs`, Dispatch in `app`, Profil-Lauf; `tests/cli_dedupe_dirs.rs` mit den Ende-zu-Ende-Akzeptanzkriterien (Plan ändert nichts, Apply, Undo byteidentisch, Junk ignoriert, unterschiedliche Datei → kein Treffer, drei Kopien, nur oberste Ebene, geschützt, stale, keep geändert, OneDrive-Obergrenze, Heuristik, Idempotenz).
   - Files: `src/config.rs`, `src/settings/fields.rs`, `src/cli.rs`, `src/app/mod.rs`, `src/app/plan.rs`, `src/app/run.rs`, `src/ops/plan.rs`, `tests/cli_dedupe_dirs.rs`
   - Depends on: Steps 3, 4
7. **Bericht** — `FolderDuplicatesReport` inkl. teilweise gleicher Paare; HTML-Abschnitt, CSV, JSON, Terminal-Zeile, Empfehlungssatz; Tests in `report/tests*.rs` (Abschnitt vorhanden, 85 % erscheint, 75 % nicht, exakte Gruppe nicht doppelt als „teilweise“, Top-50-Grenze im HTML, CSV vollständig).
   - Files: `src/analysis/folder_dups.rs` (Partial-Teil), `src/report/*`, `templates/report.html`
   - Depends on: Step 1 (unabhängig von 2 bis 6)
8. **GUI** — Karte „Doppelte Ordner“ vor „Duplikate“ mit Zahl aus dem Report, Strategie-Auswahl, Review-Zeilen für Ordner, Apply-Text, Analyse-Reiter, Hilfe; Tests `gui_cleanup_cards.rs` (7 Karten, Reihenfolge, Zahlentext), `gui_roundtrip.rs` (GUI-Teilplan → CLI-apply → CLI-undo für `dedupe-dirs`), `gui_help.rs` (Hilfetext zur Karte).
   - Files: `src/gui/views/cleanup_cards.rs`, `src/gui/views/cleanup.rs`, `src/gui/views/apply.rs`, `src/gui/review.rs`, `src/gui/views/analysis_tabs.rs`, `src/gui/help.rs`, `src/gui/texts.rs`, `tests/gui_*.rs`
   - Depends on: Steps 6, 7
9. **Rundreise und Performance** — `dedupe-dirs` in `tests/roundtrip_random.rs` (Zufallsbaum mit Ordnerkopien, Junk-Zusätzen, verschachtelten Kopien), 8 Seeds grün plus Lauf mit `--ignored`; Messung in `tests/perf_plans.rs` nach bestehendem Muster, Ziel aus der Spec (1 Mio. Dateien < 30 s; falls der Test kleiner skaliert, linear hochrechnen und Messwert in der Spec festhalten).
   - Files: `tests/roundtrip_random.rs`, `tests/perf_plans.rs`
   - Depends on: Step 6
10. **Doku und Abschluss** — README, `docs/dokumentation.md` (Befehl, Gleichheitsregel, Heuristik, Reihenfolge-Empfehlung), Handtest in `docs/manual-tests.md` (echter Ordner mit „Kopie von …“, OneDrive-Ordner mit Cloud-only-Datei), Roadmap-Status; `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test`; Spec und Plan auf `implemented`.
   - Files: `README.md`, `docs/*.md`
   - Depends on: Steps 1 bis 9

## Test Strategy
- **Unit:** Analyse-Kern (`folder_dups`: Fingerabdruck, Ignorier-Regel, Vergleichbarkeit, Heuristik, Partial-Paare inkl. Kappung), Planer (`dedupe_dirs` mit In-Memory-Index wie `archive.rs`/`dedupe.rs`), Plan-Validierung, Apply mit `Fx`/`FaultyFs`, Undo/Purge, `ActionCounts`-Texte, Config-Validierung. TDD je Schritt (`test-driven-development`): zuerst der fehlschlagende Test aus dem Akzeptanzkriterium, dann der Code.
- **Integration:** `tests/cli_dedupe_dirs.rs` über die echte CLI mit isoliertem `ORDNER_CLEANUP_HOME`, Snapshot-Vergleich wie in `cli_cleanup_actions.rs`; Profil-Lauf mit `plans = ["dedupe-dirs"]`; Bericht-Tests für HTML/CSV/JSON.
- **E2E/Eigenschaften:** `tests/roundtrip_random.rs` mit `dedupe-dirs` in zufälliger Reihenfolge zusammen mit allen anderen Planern (Plan ändert nichts, Apply ohne Fehler, Idempotenz, Undo rückwärts byteidentisch); GUI-Rundreise in `tests/gui_roundtrip.rs`.
- **Performance:** `tests/perf_plans.rs`.
- **Manuell:** Handtest in `docs/manual-tests.md` auf einem echten Ordner mit Kopien und einem OneDrive-Ordner mit Cloud-only-Datei (nichts darf heruntergeladen werden).
- **Abschluss:** `verification-before-completion` vor `status: implemented`, `requesting-code-review` vor dem Merge `dev → main`.

## Risks & Open Questions
- **Teilweise gleich wird zu teuer** (viele Duplikate, tiefe Bäume): Kappung je Hash-Gruppe (`MAX_PARTIAL_GROUP = 32`) und Obergrenze von 200 000 Ordnerpaaren mit Vermerk „unvollständig“; Werte nach dem Perf-Test anpassen.
- **Apply-Prüfung des behaltenen Ordners nur über Metadaten:** Eine Inhaltsänderung bei gleicher Größe und mtime bliebe unentdeckt. Das ist dasselbe Restrisiko wie bei `archive`/`versions` und im Ergebnis harmlos: Der entfernte Ordner liegt in der Quarantäne und ist per Undo zurückholbar.
- **Lange Pfade in der Quarantäne:** Tiefe Dateien im Ordner könnten das Limit überschreiten; `fits_dir` prüft die längste relative Länge schon beim Planen.
- **Verschieben großer Ordner unter OneDrive:** Rename ist lokal billig, löst aber Sync aus; die bestehende Obergrenze zählt `files` bereits.
- **Heuristik-Fehltreffer** („Altbau“, „Copyright“): Muster mit Wortgrenzen (`\balt\b`, `\bcopy\b`, `\(\d+\)$`), Tests mit Gegenbeispielen; endgültige Liste nach Handtest (offene Frage der Spec).
- **Junk-Ignorieren und Inhaltsverlust:** Ignorierte Dateien wandern mit in die Quarantäne, auch wenn sie sich vom behaltenen Ordner unterscheiden. Das ist laut Spec gewollt und über Undo umkehrbar.
- [ ] Startwerte `MAX_PARTIAL_GROUP = 32` und 200 000 Paare nach dem Perf-Test bestätigen (offene Frage der Spec, hier mit Startwert beantwortet)
- [ ] Reihenfolge der Karten: Plan legt „Doppelte Ordner“ **vor** „Duplikate“ fest (Empfehlung des Berichts); bitte bei der Freigabe bestätigen oder ändern
- [ ] Reiter in der Analyse-Ansicht: Plan nimmt die Liste in den bestehenden Reiter „Duplikate“ auf (kein eigener Reiter)
