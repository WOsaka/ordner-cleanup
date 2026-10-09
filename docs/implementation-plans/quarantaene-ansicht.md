---
title: "Implementation Plan: Quarantäne-Ansicht (Inhalt, Größe, Ablauf, einzelne Dateien zurückholen)"
feature_spec: docs/features/quarantaene-ansicht.md
status: pending-approval   # pending-approval | approved | implemented
created: 2026-10-09
updated: 2026-10-09
---

# Implementation Plan: Quarantäne-Ansicht

## Summary

Die Liste kommt aus den Journalen, nicht aus der Quarantäne selbst. Ein neues Modul `change/restore.rs` nutzt die Journal-Auswertung aus `change/undo.rs` (`load`, `collect_ops`, `Op`), die dafür `pub(super)` wird. Es liefert drei Dinge:
- alle Quarantäne-Moves, die ausgeführt und weder zurückgedreht noch gelöscht sind (`list_quarantine`);
- eine Vorschau für eine Auswahl (`preview_restore`: Ziel, ggf. umbenannt, Hash-Prüfung);
- das Zurückholen einzelner Aktionen eines Laufs (`restore_items`).

Das Zurückholen schreibt pro Eintrag `UndoDone` ins Journal des Laufs, aber kein `UndoStart`/`UndoEnd`. Dadurch bleibt der Laufstatus unverändert, nur die Größe in der Quarantäne sinkt. Ein späteres Undo des ganzen Laufs überspringt den Eintrag, weil `collect_ops` ihn schon als `undone` kennt. Bei belegtem Ursprungspfad bekommt die Datei einen Namen über `quarantine::unique_target`, und `UndoDone` trägt das neue optionale Feld `to`.

Die Pfadprüfungen aus `Restore::move_back` werden in eine gemeinsame Funktion herausgelöst, damit Undo und Zurückholen dieselben Regeln haben. Die `ops`-Schicht (`ops/quarantine.rs`) fasst alle Wurzeln zusammen und gruppiert eine Auswahl nach Wurzel und Lauf. Die GUI bekommt die Seite `Page::Quarantine` mit einer virtualisierten Tabelle (`egui_extras::TableBuilder`, wie in der Review). Filtern und Sortieren laufen als reine, getestete Funktion. Dazu kommen ein Lese-Task für die Vorschau, ein Schreib-Task fürs Zurückholen und in der Verlaufsansicht ein Knopf „Inhalt ansehen“.

## Files to Create

| File | Purpose |
|------|---------|
| `src/change/restore.rs` | `QuarantineItem`, `list_quarantine`, `RestorePreview`/`HashCheck`, `preview_restore`, `ItemOutcome`/`ItemResult`, `restore_items`; Unit-Tests mit `RealFs` und dem Fake aus `fsops` (Cloud-only) |
| `src/ops/quarantine.rs` | `quarantine_list(root)`, `restore_check(selection)`, `restore_execute(selection, ctx)` über alle bekannten Wurzeln; Fortschritt und Abbruch über `OpCtx` |
| `src/gui/views/quarantine.rs` | `QuarantineView`: Laden, Wurzel-Auswahl, Suche, Sortierung, Summenzeile, Tabelle mit Auswahl, „Zurückholen …“, Dialoge, Ergebnis-Meldung; reine Funktion `visible_rows` mit Unit-Tests |
| `tests/gui_quarantine.rs` | Rundreise ops ↔ CLI im Stil von `tests/gui_roundtrip.rs`: `plan junk` + `apply` per CLI, Liste, einzelnes Zurückholen, Kollision, danach `undo` und `purge` per CLI |
| `tests/perf_quarantine.rs` | Perf-Test (ignoriert wie `perf_plans.rs`): 100 000 Quarantäne-Einträge laden, filtern, sortieren |

## Files to Modify

| File | Change Description |
|------|--------------------|
| `src/change/journal.rs` | `UndoDone` bekommt `#[serde(default, skip_serializing_if = "Option::is_none")] to: Option<String>`; Test: altes `undo_done` ohne Feld wird gelesen, neues mit Feld rundreist |
| `src/change/undo.rs` | `load`, `collect_ops`, `Op`, `OpKind` werden `pub(super)`; Pfadprüfung aus `move_back` als `pub(super) fn move_is_sane(root_key, stop, from, to) -> bool`; alle `UndoDone { … }` mit `to: None` |
| `src/change/mod.rs` | `pub mod restore;` |
| `src/ops/mod.rs` | `pub mod quarantine;` |
| `src/gui/shell.rs` | `Route::Quarantine`, `Page::Quarantine` (nach `History`), `Page::ALL` mit 6 Einträgen, Symbol und Beschriftung; `Goto` bekommt `quarantine_run: Option<(PathBuf, RunId)>` |
| `src/gui/app.rs` | `QuarantineView` als Feld, Routing von `on_finished`/`on_answer`, Seite rendern, `Goto` auswerten |
| `src/gui/views/mod.rs` | `pub mod quarantine;` |
| `src/gui/views/history.rs` | Knopf „Inhalt ansehen“ je Lauf mit `bytes > 0` und Status ≠ `Purged` → `shell.goto` auf die Quarantäne-Seite, gefiltert auf den Lauf; `detail_rows`: `UndoDone` mit `to` ergibt „zurückgeholt als …“ |
| `src/gui/keys.rs` | `Key::Num6` → `page(5)`; Hilfeliste „Strg+1 … Strg+6“; Test `Num6 → None` wird `Num7 → None` |
| `src/gui/texts.rs` | `NAV_QUARANTINE = "Quarantäne"` und die Texte der Seite (leerer Zustand, Summenzeile, Dialog, Ergebnis) |
| `src/gui/help.rs` | `static QUARANTINE: HelpEntry` und `Topic::Page(Page::Quarantine)`; der bestehende Test „jede Seite hat einen eigenen Text“ deckt die Seite automatisch ab |
| `tests/gui_help.rs`, `tests/gui_smoke.rs` | Neue Seite, falls die Tests Seiten aufzählen |
| `docs/dokumentation.md` | §3 „Läufe und Journal“: Zurückholen einzelner Einträge; §6.3 undo: Verhalten nach einzelnem Zurückholen; §10 Journal-Eintrag `undo_done.to`; GUI-Abschnitt bzw. README: neue Seite |
| `README.md` | GUI-Seiten: „Quarantäne“ ergänzen |
| `docs/manual-tests.md` | Neuer Abschnitt „1c. Quarantäne-Ansicht (Phase 7.4)“ |
| `docs/roadmap.md` | 7.4 abhaken, Feature-Status |

## Data Model Changes

- **Journal:** `Entry::UndoDone { run, action, to: Option<String> }`. `to` gibt es nur, wenn die Datei unter einem anderen Pfad als `from` zurückkam (Kollision). Alte Journale ohne Feld bleiben lesbar (`serde(default)`). `Entry` hat kein `deny_unknown_fields`, also lesen auch ältere Programmversionen neue Journale.
- **Neue Typen in `change/restore.rs`:**
  ```rust
  pub struct QuarantineItem {
      pub run: RunId,
      pub action: u32,
      pub origin: String,          // `from` aus dem Intent
      pub stored: String,          // `to` aus dem Intent (in der Quarantäne)
      pub size: u64,
      pub hash: Option<String>,    // nur Dateien aus `dedupe` (xxh3-128); bei Ordnern ein Fingerprint, wird nicht geprüft
      pub is_dir: bool,
      pub file_count: Option<u64>, // nur bei Ordnern, per Verzeichnis-Durchlauf ohne Dateien zu öffnen
      pub started: Option<String>,
      pub expires: Option<DateTime<Utc>>,
      pub present: bool,           // liegt noch in der Quarantäne
      pub cloud_only: bool,
  }

  pub enum HashCheck { NotApplicable, Match, Mismatch, CloudOnly }

  pub struct RestorePreview {
      pub run: RunId,
      pub action: u32,
      pub target: PathBuf,         // ursprünglicher Pfad oder `name (2).ext`
      pub renamed: bool,
      pub hash: HashCheck,
  }

  pub enum ItemOutcome {
      Restored { to: String, renamed: bool },
      AlreadyRestored,             // inzwischen per Undo oder einzeln zurück
      Missing,                     // nicht mehr in der Quarantäne
      Failed(String),
  }
  pub struct ItemResult { pub action: u32, pub origin: String, pub outcome: ItemOutcome }
  ```
  `RestoreStatus` und `UndoOutcome` bleiben unverändert. So ändert sich an den Aufrufern von `undo_run` (CLI-Ausgabe, `undo_flow`) nichts.
- **GUI:** `Page::Quarantine`, `Route::Quarantine`. `Page` wird mit serde gespeichert, der neue Wert ist nur additiv. `Goto.quarantine_run`.

## API / Interface Changes

- `change::restore::list_quarantine(root, quarantine_days, fs) -> Result<QuarantineListing, UndoError>`
  - `QuarantineListing { items, unreadable: usize }`.
  - Liest alle Journale wie `list_runs`. Je Lauf: `collect_ops`, dann Filter `done && !failed && !undone`, `Move` mit `dest: Quarantine`, Lauf nicht `Purged`.
  - `present`, `cloud_only` und `file_count` kommen aus `fs.metadata` bzw. `read_dir`. Der Inhalt von Dateien wird nie gelesen.
  - Journal-Einträge, die `move_is_sane` nicht bestehen, fallen weg.
- `change::restore::preview_restore(root, selection: &[(RunId, u32)], fs) -> Result<Vec<RestorePreview>, UndoError>`
  - Rechnet das Ziel aus: `from`, wenn frei, sonst `unique_target(from)`.
  - Hash-Prüfung nur, wenn `hash` vorhanden ist, `!is_dir` gilt und die Datei nicht cloud-only ist (`fs.full_hash`).
  - Ändert nichts.
- `change::restore::restore_items(root, run, actions: &[u32], env: &UndoEnv) -> Result<Vec<ItemResult>, UndoError>`
  - `load` prüft wie beim Undo, dass das Journal zu Wurzel und Lauf gehört. `ensure_plain_dirs` lehnt Links im Tool-Ordner ab.
  - Je Aktion:
    1. Zustand prüfen: Move in die Quarantäne, ausgeführt, nicht `undone`, Lauf nicht `purged`. Sonst `AlreadyRestored` oder `Missing`.
    2. `move_is_sane` prüfen.
    3. Ziel bestimmen, Elternordner anlegen. Ist der Elternordner danach kein normaler Ordner (Datei oder Link), wird der Eintrag `Failed("Ziel ist kein Ordner")`.
    4. `fs.rename` aufrufen, das nie überschreibt. Bei `AlreadyExists` einmal neu `unique_target` versuchen, sonst `Failed`.
    5. `UndoDone { to: renamed.then(...) }` ins Journal schreiben.
    6. `cleanup_empty_parents` im Lauf-Ordner der Quarantäne aufräumen.
  - Zwischen den Einträgen wird `env.cancel` geprüft.
  - Kein `UndoStart`/`UndoEnd`.
- `ops::quarantine`:
  - `quarantine_list(root: Option<&Path>) -> Result<QuarantineList>` mit Einträgen je Wurzel, Summe der unlesbaren Journale und nicht erreichbaren Wurzeln.
  - `restore_check(selection: &[Selected]) -> Result<Vec<(PathBuf, RestorePreview)>>`.
  - `restore_execute(selection, ctx) -> Result<Vec<(PathBuf, RunId, ItemResult)>>`: gruppiert nach Wurzel und Lauf, setzt `ctx.progress`, ein Abbruch beendet nach dem laufenden Eintrag.
  - `Selected { root: PathBuf, run: RunId, action: u32 }`.
- **GUI-intern:** `quarantine::visible_rows(items, filter: &str, root: Option<&Path>, run: Option<&RunId>, sort: Sort) -> Vec<usize>`. Die Funktion ist rein. Die Suche ignoriert Groß- und Kleinschreibung (Kleinbuchstaben einmal beim Laden vorberechnet). Sortiert wird nach Name, Größe, Datum oder Ablauf, auf- oder absteigend. Standard ist „Datum absteigend“.
- **CLI:** keine Änderung.

## Implementation Sequence

Jeder Schritt läuft nach `test-driven-development`: zuerst der rote Test, dann der Code. Nach jedem Schritt laufen `cargo test`, `cargo clippy -- -D warnings` und `cargo fmt`, dann folgt ein Commit auf `dev`.

1. **Journal-Feld `to`.** `UndoDone.to` anlegen, alle Konstruktoren mit `to: None`. Tests: altes JSON ohne Feld wird gelesen, `to: None` wird nicht serialisiert, `to: Some` rundreist.
   - Files: `src/change/journal.rs`, `src/change/undo.rs`, `src/gui/views/history.rs` (Test-Konstruktor)
   - Depends on: nichts
2. **Undo-Bausteine freilegen.** Die Prüfung in `move_back` wird zu `move_is_sane` (gleiche Logik, `move_back` ruft sie auf). Sichtbarkeit `pub(super)` für `load`, `collect_ops`, `Op`, `OpKind`. Bestehende Undo-Tests müssen unverändert grün bleiben. Neue Unit-Tests für `move_is_sane`: außerhalb der Wurzel, `..`, Ziel gleich Stop-Ordner, gültiger Fall.
   - Files: `src/change/undo.rs`
   - Depends on: nichts
3. **`list_quarantine`.** Tests mit echtem Tempdir und `apply` aus den bestehenden Test-Helfern (`change/test_support.rs`):
   - Nur Quarantäne-Moves erscheinen, keine Archiv-, Regel- oder Leerordner-Aktionen.
   - Vollständig zurückgedrehte und per `purge` geleerte Läufe erscheinen nicht.
   - Teilweise zurückgedrehte Läufe zeigen nur den Rest.
   - Fehlschläge und `skip` erscheinen nicht.
   - Eine von Hand gelöschte Datei hat `present: false`.
   - Ein Ordner-Eintrag hat die richtige `file_count`.
   - Ein Cloud-only-Eintrag (Fake-FS) wird nie gelesen.
   - Ein unlesbares Journal zählt in `unreadable`.
   - Files: `src/change/restore.rs`, `src/change/mod.rs`
   - Depends on: Schritt 2
4. **`preview_restore`.** Tests:
   - freies Ziel ergibt den Ursprungspfad;
   - belegtes Ziel ergibt `name (2).ext`;
   - Ziel und ` (2)` belegt ergibt ` (3)`;
   - bei Hash `Match` und `Mismatch` (Datei in der Quarantäne verändert);
   - ohne Hash `NotApplicable`;
   - Ordner `NotApplicable`;
   - Cloud-only `CloudOnly` ohne Lesezugriff.
   - Files: `src/change/restore.rs`
   - Depends on: Schritt 3
5. **`restore_items`.** Tests:
   - Datei kommt an den Ursprung, Elternordner werden neu angelegt, leere Ordner im Lauf-Ordner der Quarantäne verschwinden.
   - Kollision ergibt einen umbenannten Zielnamen, das Original bleibt byte-gleich, `UndoDone.to` ist gesetzt.
   - Ein ganzer Ordner (`dedupe-dirs`) kommt als Ganzes zurück.
   - Ist der Elternordner eine Datei, wird der Eintrag `Failed`, die übrigen laufen weiter.
   - Ein zweiter Aufruf liefert `AlreadyRestored`.
   - Fehlende Quarantäne-Datei ergibt `Missing`.
   - Ein Journal-Eintrag außerhalb der Wurzel wird `Failed`, ohne Dateioperation.
   - Ist der Tool-Ordner eine Junction, wird abgelehnt (Test überspringt, wenn `mklink` fehlt, wie in `quarantine.rs`).
   - Abbruch vor dem zweiten Eintrag: der erste bleibt zurück und ist journaliert.
   - Files: `src/change/restore.rs`
   - Depends on: Schritt 4
6. **Zusammenspiel mit Undo, Purge und `runs`.** Tests in `restore.rs`:
   - Nach dem einzelnen Zurückholen liefert `undo_run` für den Eintrag `NothingToDo`, Exit-Code 0, die restlichen Einträge kommen zurück.
   - Nach dem Zurückholen mit Kollision lässt `undo_run` die umbenannte Datei in Ruhe.
   - `purge_run` lässt zurückgeholte Dateien unberührt.
   - `list_runs` zeigt verringerte `bytes` und unveränderten Status `Complete`. Sind alle Einträge zurück, ist `bytes == 0` und der Status bleibt `Complete`.
   - Files: `src/change/restore.rs`
   - Depends on: Schritt 5
7. **ops-Schicht.** `ops/quarantine.rs` mit Unit-Tests: Gruppierung der Auswahl nach Wurzel und Lauf, Abbruch vor dem ersten Eintrag liefert ein leeres Ergebnis, nicht erreichbare Wurzel als Hinweis statt Fehler.
   - Files: `src/ops/quarantine.rs`, `src/ops/mod.rs`
   - Depends on: Schritt 5
8. **Seite, Navigation, Kürzel, Hilfe.**
   - `Page::Quarantine`/`Route::Quarantine` mit Symbol aus Segoe UI Symbol (z. B. `\u{25A3}`). Prüfen, dass `gui_theme_static`/`fonts` es abdeckt.
   - Leere `QuarantineView` mit Überschrift und `heading_with_help` einhängen.
   - `Strg+6` einführen, `HELP`-Text anpassen.
   - `QUARANTINE`-Hilfetext: Was ist die Quarantäne, wann läuft sie ab, wie holt man zurück, was passiert bei Namensgleichheit.
   - Tests: `keys`-Tests, Hilfe-Test über `Page::ALL`, `gui_smoke`.
   - Files: `src/gui/shell.rs`, `src/gui/app.rs`, `src/gui/keys.rs`, `src/gui/texts.rs`, `src/gui/help.rs`, `src/gui/views/mod.rs`, `src/gui/views/quarantine.rs`, `tests/gui_help.rs`, `tests/gui_smoke.rs`
   - Depends on: nichts (parallel zu 1–7 möglich)
9. **Liste, Filter, Sortierung.**
   - `visible_rows` mit Unit-Tests: Suche auf Name und Pfad ohne Groß-/Kleinschreibung, Filter nach Wurzel und Lauf, Sortierung je Spalte stabil, Standardsortierung.
   - Laden als Lese-Task beim Öffnen und nach jedem `shell.generation`-Wechsel.
   - Kopfzeile mit Wurzel-Auswahl, Suchfeld und Summenzeile („N Einträge, X GB, nächster Ablauf in N Tagen“). Dazu Hinweise auf nicht lesbare Läufe und nicht erreichbare Wurzeln.
   - `TableBuilder` mit `body.rows` (virtualisiert). Spalten: Auswahl, Name, Ursprünglicher Ort (`short_path_cell`, Kontextmenü zeigt den Quarantäne-Pfad im Explorer), Größe, Lauf/Datum (`time_cell`), Läuft ab, Status (`status_cell`: „in Quarantäne“ oder „fehlt“, „nur online“ als Hinweis).
   - Leerer Zustand „Die Quarantäne ist leer.“
   - Die gefilterte Indexliste wird nur bei geänderter Eingabe neu berechnet.
   - Files: `src/gui/views/quarantine.rs`, `src/gui/texts.rs`
   - Depends on: Schritte 7, 8
10. **Zurückholen in der GUI.**
    - Auswahl über Kontrollkästchen. Dazu „Alle gefilterten wählen“ und „Auswahl aufheben“.
    - „Zurückholen …“ ist deaktiviert ohne Auswahl und während eines Schreib-Tasks (`shell.is_running` bzw. `Rejected`).
    - Klick startet `restore_check` als Lese-Task.
    - Danach öffnet sich `Dialog::Confirm`: Anzahl, Größe, die ersten 15 Ziele, umbenannte Ziele mit „→ neuer Name“, Warnung bei `Mismatch`, Hinweis bei `CloudOnly`.
    - OK startet `restore_execute` als Schreib-Task.
    - Danach folgt eine Meldung („N zurückgeholt, davon M umbenannt, K fehlen, F Fehler“ plus die ersten Fehler) und `shell.generation += 1`.
    - Test in `tests/gui_quarantine.rs` (Schritt 12) deckt die ops-Kette ab, die Oberfläche selbst prüft der Handtest.
    - Files: `src/gui/views/quarantine.rs`, `src/gui/texts.rs`
    - Depends on: Schritt 9
11. **Verlauf.**
    - „Inhalt ansehen“ je Lauf mit Quarantäne-Inhalt setzt `shell.goto = Goto { page: Quarantine, quarantine_run: Some((root, run)), .. }`. Die Quarantäne-Seite übernimmt den Filter und zeigt ihn als entfernbares Etikett „Nur Lauf …“.
    - `detail_rows`: `UndoDone` mit `to` ergibt „zurückgeholt als <to>“. Unit-Test dazu.
    - Files: `src/gui/views/history.rs`, `src/gui/shell.rs`, `src/gui/app.rs`, `src/gui/views/quarantine.rs`
    - Depends on: Schritte 1, 9
12. **Integrationstest.** Ablauf in `tests/gui_quarantine.rs`:
    1. Baum anlegen, `scan`, `plan junk` und `apply --yes` per CLI.
    2. `quarantine_list` zeigt die Einträge.
    3. Einen Eintrag mit `restore_execute` zurückholen.
    4. Prüfen: Datei am Ursprung, `runs` per CLI zeigt die geringere Größe.
    5. `undo` per CLI liefert Exit-Code 0, alles zurück.

    Zweiter Fall: Ursprungspfad vorher neu belegt. Das Zurückholen ergibt `(2)`, und `undo` meldet keinen Konflikt für diesen Eintrag. Dritter Fall: `purge --older-than 0d` nach dem Zurückholen. Die zurückgeholte Datei bleibt, der Rest ist weg.
    - Files: `tests/gui_quarantine.rs`
    - Depends on: Schritt 7
13. **Performance.** `tests/perf_quarantine.rs` (`#[ignore]`): 100 Journale mit je 1 000 Quarantäne-Moves und echten kleinen Dateien.
    - `quarantine_list` soll unter 10 s laufen (Grenze wie `perf_plans.rs`).
    - `visible_rows` mit Suchbegriff und Sortierung soll auf 100 000 Einträgen unter 100 ms bleiben.
    - Messwerte kommen in `dokumentation.md` §13.
    - Files: `tests/perf_quarantine.rs`
    - Depends on: Schritte 3, 9
14. **Doku und Abschluss.**
    - `dokumentation.md`, `README.md`, `manual-tests.md` (Abschnitt 1c: Datei zurückholen, Kollision, Ordner aus `dedupe-dirs`, „Inhalt ansehen“ aus dem Verlauf, Cloud-only-Datei in der Quarantäne, Strg+6, F1), `roadmap.md`.
    - `verification-before-completion`: voller `cargo test`, `clippy` und `fmt`; `roundtrip_random.rs` grün.
    - Danach Spec und Plan auf `implemented`.
    - Files: siehe oben
    - Depends on: Schritte 1–13

## Test Strategy

- **Unit (`change/restore.rs`):**
  - Liste: Filterregeln, `present`, `file_count`, Cloud-only ohne Lesezugriff, unlesbare Journale.
  - Vorschau: Zielnamen und Hash-Fälle.
  - Zurückholen: Erfolg, Kollision, Ordner, Fehlerfälle, Abbruch, Links.
  - Zusammenspiel mit `undo_run`, `purge_run` und `list_runs`.
- **Unit (`undo.rs`, `journal.rs`):** `move_is_sane`, Rundreise und Abwärtskompatibilität von `UndoDone.to`. Alle bestehenden Undo-Tests bleiben unverändert grün.
- **Unit (`ops/quarantine.rs`):** Gruppierung, Abbruch, nicht erreichbare Wurzeln.
- **Unit (GUI-Logik):** `visible_rows`, `keys` (Strg+6), Hilfetext je Seite, `detail_rows` mit `to`.
- **Integration (`tests/gui_quarantine.rs`):** ops ↔ CLI mit `apply`, Zurückholen, `runs`, `undo` und `purge`, inklusive Kollision.
- **Rundreise:** `tests/roundtrip_random.rs` bleibt unverändert grün. Ein zusätzlicher Planer-Schritt „zufällig einzelne Einträge zurückholen“ ist nicht nötig, denn Schritt 6 deckt das Zusammenspiel gezielt ab.
- **Performance:** `tests/perf_quarantine.rs` (ignoriert, Release).
- **Manuell (`manual-tests.md` 1c):** Optik und Bedienung der Seite, Dialogtexte, Explorer-Kontextmenü, Kollision, Ordner-Eintrag, echter Cloud-only-Platzhalter in der Quarantäne (Zurückholen ohne Download), Sprung aus dem Verlauf, F1 und Strg+6.

## Risks & Open Questions

- **Strg+5 ändert sich.** Die neue Seite steht laut Spec neben „Verlauf“, damit rutscht „Einstellungen“ von Strg+5 auf Strg+6. Gegenmaßnahme: Die Hilfeliste und die Kürzelübersicht (`?`) werden angepasst, und das Release-Notes-Stichwort in der Doku nennt es. Alternative wäre die Seite ans Ende zu stellen. Das wäre gegen die Spec, deshalb nicht gewählt.
- **Gleichzeitiges Undo aus der CLI und Zurückholen in der GUI.** Beide hängen an dasselbe Journal an. Die GUI serialisiert nur ihre eigenen Schreib-Tasks. Das Risiko besteht schon heute (CLI-`undo` parallel zur GUI) und ist durch „nie überschreiben“ bei `rename` begrenzt: Im schlimmsten Fall meldet eine Seite `Missing` oder `NothingToDo`. Keine eigene Sperre in diesem Feature.
- **`exists` je Eintrag bei 100 000 Einträgen.** Das sind 100 000 Metadaten-Abfragen beim Laden. Bei SSD sind wenige Sekunden zu erwarten, das läuft im Hintergrund. Gegenmaßnahme: Der Perf-Test misst es. Wird es zu langsam, liest der Lauf-Ordner per `read_dir` in eine Menge, statt einzeln zu fragen.
- **`file_count` für Ordner-Einträge.** Ein Verzeichnis-Durchlauf je Ordner. Ordner-Einträge kommen nur aus `dedupe-dirs` und sind selten. Bei Cloud-only-Ordnern zählt der Durchlauf nur Verzeichniseinträge, er öffnet keine Dateien, also gibt es keinen Download.
- **Hash bei Ordnern.** Das Feld `hash` eines `dedupe-dirs`-Intents ist ein Fingerprint und kein Datei-Hash. Die Prüfung wird für `is_dir` deshalb ausdrücklich übersprungen (`NotApplicable`).
- **Elternordner als Junction weiter oben im Pfad.** `restore_items` prüft wie `move_back` nur den direkten Elternordner. Links weiter oben im Ursprungspfad bleiben wie beim heutigen Undo ungeprüft. Keine Verschärfung in diesem Feature, aber im Risiko-Abschnitt der Doku erwähnt.
- [x] Eigenes Modul `restore.rs` statt Erweiterung von `undo.rs`: `undo.rs` ist schon über 1 500 Zeilen lang. Die gemeinsamen Teile werden `pub(super)`.
- [x] Eigener Ergebnis-Typ `ItemOutcome` statt neuer Variante in `RestoreStatus`: So bleiben `UndoOutcome`, die CLI-Ausgabe und `undo_flow` unberührt.
