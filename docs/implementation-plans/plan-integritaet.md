---
title: "Implementation Plan: Plan-Integrität (Hash der Plan-Datei, vor dem Apply geprüft)"
feature_spec: docs/features/plan-integritaet.md
status: pending-approval   # pending-approval | approved | implemented
created: 2026-10-09
updated: 2026-10-09
---

# Implementation Plan: Plan-Integrität

## Summary
Die Prüfsumme lebt nur in der Datei, nicht im Datenmodell. `Plan` bekommt kein neues Feld, damit die sieben Planer, `subset` und alle Test-Konstruktoren unverändert bleiben und ein Plan im Speicher nie einen veralteten Hash tragen kann. Kanonischer Inhalt ist `serde_json::to_string(&plan)` (kompakt, Feldreihenfolge der Struktur, `BTreeMap` sortiert). Davon wird SHA-256 berechnet (`Plan::integrity()`, Format `sha256:<64 hex>`). Beim Schreiben (`to_json`/`save`) wird der Plan immer als Format v3 ausgegeben, mit `integrity` als erstem Feld, über eine Hilfsstruktur mit `#[serde(flatten)]`. Beim Lesen entfernt ein neuer Kern `Plan::parse` das Feld aus dem JSON-Wert, deserialisiert, berechnet den Hash neu und liefert den Zustand der Versiegelung (`Seal`) mit. `Plan::load`/`from_json` lehnen eine Abweichung mit dem neuen `PlanError::Tampered` ab, das `main.rs` auf Exit-Code `3` abbildet. v1/v2-Pläne laufen weiter, `apply_check` meldet sie als „ungeschützt“, und die GUI zeigt den Hinweis in der Review-Kopfzeile. Für bewusste Handänderungen gibt es `plan seal <datei> [--yes]`, das `Plan::parse` ohne Ablehnung nutzt und die Datei atomar ersetzt. Die GUI vergleicht vor dem Anwenden den Hash der Datei auf der Platte mit dem des geladenen Plans und bietet bei Abweichung „Plan neu laden“ an.

## Files to Create
| File | Purpose |
|------|---------|
| `tests/cli_plan_integrity.rs` | Ende-zu-Ende über die CLI: Plan erzeugen, Datei ändern (Wert, Aktion entfernen, nur Formatierung/CRLF/Schlüsselreihenfolge), `apply` → Exit-Code `3` bzw. Erfolg; `plan seal` mit und ohne `--yes`; v1-Plan mit Warnung; kein Eintrag in `runs` nach Abbruch |

## Files to Modify
| File | Change Description |
|------|--------------------|
| `Cargo.toml` | `sha2 = "0.10"` (steht als 0.10.9 schon transitiv im `Cargo.lock`, also kein neuer Download). CI-Sperre in `ci.yml` betrifft nur HTTP-/TLS-Crates |
| `src/change/plan.rs` | `PLAN_VERSION = 3`; `PlanError::Tampered`; `Seal`; `Plan::parse`, `Plan::integrity`, `Plan::read_unverified`, `Plan::save_replacing`; `to_json` schreibt versiegelt; `from_json`/`load` prüfen; v1/v2 mit `integrity` und v3 ohne sind ungültig; Doc-Kommentar von `subset` anpassen (beim Speichern wird jeder Plan v3) |
| `src/main.rs` | Fehler per `downcast_ref::<PlanError>()` prüfen: `Tampered` → Exit-Code `3`, sonst `1` |
| `src/ops/apply.rs` | `check_with`: Warnung „Ungeschützter Plan (Format vN) …“ für `plan.version < 3` in `notes` |
| `src/ops/plan.rs` | `PlanOutcome` bekommt `integrity: Option<String>` (gesetzt, wenn gespeichert); neue Funktionen `seal_preview(path)` und `seal_write(path)` für `plan seal` |
| `src/cli.rs` | `PlanCommand::Seal(PlanSealArgs { plan: PathBuf, yes: bool })` mit Hilfetext |
| `src/app/mod.rs` | Dispatch `PlanCommand::Seal` |
| `src/app/plan.rs` | Ausgabe „Prüfsumme: sha256:abcdef123456…“ nach „Plan: …“; `plan_seal_command` (Vorschau, Rückfrage über das bestehende `confirm`, Schreiben) |
| `src/gui/views/apply.rs` | Vor `apply_execute` im Task: ist `original_path` gesetzt, Datei mit `Plan::load` lesen und `integrity()` mit dem geladenen Plan vergleichen; Abweichung → eigener Fehler `PlanChanged`, `on_applied` zeigt Dialog mit „Plan neu laden“ |
| `src/gui/views/cleanup.rs` | Antwort auf den Dialog „Plan neu laden“ → `open_plan(path)` |
| `src/gui/views/review.rs` | Hinweis in der Kopfzeile bei `plan.version < 3`: „Ungeschützter Plan (Format vN): Änderungen an der Datei werden nicht erkannt.“ |
| `src/gui/help.rs` | Kurztext „Plan prüfen“: ein Satz zur Prüfsumme |
| `tests/cli_apply_undo.rs` | Test für unbekannte Version: `3` → `4` |
| `tests/cli_cleanup_actions.rs`, `tests/cli_dedupe_dirs.rs`, `tests/cli_rules.rs` | `assert_eq!(json["version"], 2)` → `3`, zusätzlich `integrity` vorhanden |
| `tests/cli_rules.rs` (Zeile ~432), `tests/protected_paths.rs` (Zeile ~157) | Von Hand geänderte v3-Pläne vor `apply` mit `plan seal --yes` versiegeln (genau der vorgesehene Ablauf) |
| `tests/gui_help.rs` | Prüft den neuen Satz, falls der Test Texte vergleicht |
| `tests/perf_plans.rs` | Laden großer Pläne inkl. Hash-Prüfung messen |
| `docs/dokumentation.md` | §6 (Plan, Review, Apply): Prüfsumme, Warnung bei alten Plänen; `plan seal` in der Befehlsübersicht; §11 Exit-Code `3`; §12 FAQ „Plan wurde nach dem Erstellen verändert“ inkl. bekannter Lücke (Version zurücksetzen) |
| `README.md` | `plan seal` in der Befehlsliste, falls dort aufgeführt |
| `docs/manual-tests.md` | Neuer Abschnitt „Plan-Integrität“ (Editor speichern, CRLF, GUI: Datei während Review ändern) |
| `docs/roadmap.md` | Punkt in 7.0 abhaken, Zeile in „Feature-Status“ |

## Data Model Changes
- **Plan-Datei, Format v3:** neues Pflichtfeld `integrity: "sha256:<64 Hex-Zeichen, klein>"`, als erstes Feld geschrieben. Sonst identisch mit v2.
- **`Plan` (Rust):** unverändert. Das Feld wird beim Lesen aus dem `serde_json::Value` entfernt, bevor `deny_unknown_fields` greift, und beim Schreiben von einer Hilfsstruktur ergänzt:
  ```rust
  #[derive(Serialize)]
  struct Sealed<'a> { integrity: String, #[serde(flatten)] plan: &'a Plan }
  ```
  `to_json` schreibt immer `version: PLAN_VERSION`. Der Hash wird über den Plan mit dieser Version berechnet.
- **Kanonische Form:** `serde_json::to_string(&plan)` des deserialisierten Plans (ohne `integrity`). Damit sind Einrückung, Zeilenenden, Schlüsselreihenfolge und explizite `null` statt fehlender optionaler Felder egal. Der Plan enthält keine Gleitkommazahlen, also keine Rundungsfragen.
- **Neuer Typ:**
  ```rust
  pub enum Seal {
      Valid,                                     // v3, Hash stimmt
      Unprotected { version: u32 },              // v1/v2
      Missing,                                   // v3 ohne Feld
      Mismatch { stored: String, actual: String },
  }
  ```
- **`PlanError::Tampered { stored: String, actual: String }`:** Meldung „Plan wurde nach dem Erstellen verändert (Prüfsumme erwartet abcdef123456…, gefunden 123456abcdef…). Bewusst geändert? Mit `ordner-cleanup plan seal <datei>` neu versiegeln.“ Keine Pfade aus dem Plan.

## API / Interface Changes
- `Plan::integrity(&self) -> String`: SHA-256 der kanonischen Form, ohne IO.
- `Plan::parse(text: &str) -> Result<(Plan, Seal), PlanError>`: Versionsprüfung (wie bisher zuerst), `integrity` entfernen und Format prüfen (`Invalid` bei falschem Format oder bei v1/v2 mit Feld), deserialisieren (`Corrupt`), Hash vergleichen und `Seal` bestimmen, dann `validate` (`Invalid`).
- `Plan::from_json(text)`: `parse`, danach `Mismatch` → `Tampered`, `Missing` → `Invalid("Plan im Format 3 braucht das Feld „integrity“")`. Signatur unverändert, deshalb ändern sich die Aufrufer nicht.
- `Plan::read_unverified(path) -> Result<(Plan, Seal), PlanError>`: nur für `plan seal`.
- `Plan::to_json(&self) -> String`: versiegeltes, eingerücktes JSON.
- `Plan::save(&self, path)`: unverändert `create_new`, schreibt `to_json()`.
- `Plan::save_replacing(&self, path)`: in `<datei>.tmp` schreiben, `sync_all`, dann `std::fs::rename` über das Original (atomar auf demselben Volume). Nur für `plan seal`.
- **CLI:** `ordner-cleanup plan seal <datei> [--yes]`
  - `Valid`: „Plan ist bereits versiegelt (sha256:…).“, Exit-Code `0`, Datei unverändert.
  - Sonst: Ausgabe von Art, Wurzel, Aktionen und Übersprungenen, alter Zustand (Prüfsumme gekürzt / „keine, Format vN“ / „fehlt“) und neue Prüfsumme gekürzt. Bei v1/v2 zusätzlich „wird auf Format 3 gehoben“. Rückfrage „Plan mit diesem Inhalt versiegeln? [j/N]“. „N“ → Exit-Code `1`, Datei unverändert. Ohne Terminal und ohne `--yes` → Abbruch wie bei `apply`.
  - Strukturfehler (`validate`) → wird nicht versiegelt, Exit-Code `1`.
- **Exit-Code `3`:** nur `PlanError::Tampered`. Gilt für `apply`. `plan seal` liefert ihn nie.
- **ops:** `apply_check` bekommt die Warnung für ungeschützte Pläne. `PlanOutcome.integrity` für die Ausgabe nach `plan …`.

## Implementation Sequence
Jeder Schritt nach `test-driven-development`: zuerst der rote Test, dann der Code. Nach jedem Schritt `cargo test`, `cargo clippy -- -D warnings` und `cargo fmt`, dann ein Commit.

1. **Abhängigkeit** — `sha2 = "0.10"` eintragen. Prüfen, dass `cargo tree -e normal` keine gesperrte Crate zeigt (Muster aus `ci.yml`).
   - Files: `Cargo.toml`, `Cargo.lock`
   - Depends on: nichts
2. **Kern: Hash, Versiegeln beim Schreiben, Prüfen beim Lesen** — `integrity`, `Sealed`, `Seal`, `parse`, `from_json`, `to_json`, `PlanError::Tampered`, `PLAN_VERSION = 3`. Unit-Tests in `plan.rs`:
   - Rundreise `to_json` → `from_json` bleibt gleich, und die Datei beginnt mit `integrity`.
   - Formatierung, CRLF, umgestellte Schlüssel und explizite `null` ergeben denselben Hash.
   - Geänderter Wert, entfernte oder zusätzliche Aktion ergeben `Tampered`.
   - v3 ohne Feld ist `Invalid`. Falsches Format (Präfix, Länge, Großbuchstaben, kein Hex) ist `Invalid`.
   - v2 mit Feld ist `Invalid`. v1 (`V1_JSON`) und v2 ohne Feld werden gelesen, `Seal::Unprotected`.
   - Version `4` ergibt `UnsupportedVersion`.

   Bestehende Tests, die versiegeltes JSON verändern, um `validate` zu prüfen (u. a. Zeilen ~666, ~911, ~1272), bekommen den Test-Helfer `unsealed(value)`. Er setzt `version` auf 2 und entfernt `integrity`, damit der Test weiter die Strukturprüfung trifft und nicht die Prüfsumme.
   - Files: `src/change/plan.rs`
   - Depends on: Schritt 1
3. **Exit-Code 3** — `main.rs` bildet `PlanError::Tampered` auf `3` ab. Integrationstest „geänderter Plan → Exit-Code 3, `runs` leer, Datei-Baum unverändert“ in `tests/cli_plan_integrity.rs`.
   - Files: `src/main.rs`, `tests/cli_plan_integrity.rs`
   - Depends on: Schritt 2
4. **Bestehende Integrationstests anpassen** — Versionsnummern (`2` → `3`, `3` → `4`). Die Tests, die einen v3-Plan von Hand ändern, bleiben rot, bis Schritt 6 `plan seal` liefert. Dort sind sie bis dahin mit `#[ignore = "bis plan seal"]` markiert, oder Schritt 4 läuft zusammen mit Schritt 6 (Entscheidung beim Umsetzen, kein roter Commit).
   - Files: `tests/cli_apply_undo.rs`, `tests/cli_cleanup_actions.rs`, `tests/cli_dedupe_dirs.rs`, `tests/cli_rules.rs`
   - Depends on: Schritt 2
5. **Warnung für ungeschützte Pläne und Ausgabe der Prüfsumme** — Warnung in `check_with` mit Unit-Test in `ops/apply.rs` (v2-Plan erzeugt die Warnung, v3 nicht). `PlanOutcome.integrity` und die Ausgabezeile in `app/plan.rs`. Integrationstests: v1-Plan mit `apply --yes` zeigt die Warnung auf stderr und läuft durch; `plan junk` gibt „Prüfsumme: sha256:“ aus.
   - Files: `src/ops/apply.rs`, `src/ops/plan.rs`, `src/app/plan.rs`, `tests/cli_plan_integrity.rs`
   - Depends on: Schritt 2
6. **`plan seal`** — `read_unverified`, `save_replacing` (Unit-Test: ersetzt den Inhalt, keine `.tmp`-Datei bleibt zurück), `seal_preview`/`seal_write` in `ops/plan.rs`, `PlanSealArgs` und Dispatch, `plan_seal_command`. Integrationstests:
   - geänderter Plan + `seal --yes` + `apply` läuft durch;
   - `seal` ohne `--yes` und ohne Terminal bricht ab, Datei unverändert;
   - bereits versiegelt → Exit-Code `0`, Datei byte-gleich;
   - v1-Plan wird nach `seal --yes` v3;
   - strukturell ungültiger Plan wird nicht versiegelt.

   Danach `tests/cli_rules.rs` (~432) und `tests/protected_paths.rs` (~157) auf `plan seal --yes` vor dem `apply` umstellen.
   - Files: `src/change/plan.rs`, `src/ops/plan.rs`, `src/cli.rs`, `src/app/mod.rs`, `src/app/plan.rs`, `tests/cli_plan_integrity.rs`, `tests/cli_rules.rs`, `tests/protected_paths.rs`
   - Depends on: Schritte 3, 4
7. **GUI: Datei vor dem Anwenden prüfen** — In `ApplyFlow::on_answer` vor `save_subset` und `apply_execute`: Bei gesetztem `original_path` wird `Plan::load(path)` mit `p.original.integrity()` verglichen. Bei Abweichung oder `Tampered` gibt es den Fehler `PlanChanged { path }`. `on_applied` zeigt dann den Dialog „Plan wurde seit dem Laden verändert“ mit „Plan neu laden“, und `cleanup.rs` öffnet den Plan per `open_plan` neu. Nichts wird ausgeführt. Ohne `original_path` (Plan nur im Speicher) entfällt die Prüfung. Test im Stil von `tests/gui_roundtrip.rs`: Plan laden, Datei ändern, Anwenden ergibt keinen Lauf in `runs`. Teilplan anwenden ergibt eine `-auswahl-`-Datei mit gültigem v3-Hash.
   - Files: `src/gui/views/apply.rs`, `src/gui/views/cleanup.rs`, `tests/gui_roundtrip.rs`
   - Depends on: Schritt 2
8. **GUI: Hinweis und Hilfe** — Hinweis in der Review-Kopfzeile bei `plan.version < 3` und Satz in `help.rs` („Plan prüfen“). Rauchtest bzw. Text-Test anpassen.
   - Files: `src/gui/views/review.rs`, `src/gui/help.rs`, `tests/gui_help.rs`, ggf. `tests/gui_smoke.rs`
   - Depends on: Schritt 2
9. **Performance** — `tests/perf_plans.rs`: Laden des größten Test-Plans inklusive Hash-Prüfung messen. Die Schwelle orientiert sich an den vorhandenen Messungen dort.
   - Files: `tests/perf_plans.rs`
   - Depends on: Schritt 2
10. **Doku und Abschluss** — `dokumentation.md`, `README.md`, `manual-tests.md`, `roadmap.md`. `verification-before-completion`: voller `cargo test`, `clippy`, `fmt`, Rundreise-Tests (`roundtrip_random.rs`) grün. Danach Spec und Plan auf `implemented`.
    - Files: siehe oben
    - Depends on: Schritte 1 bis 9

## Test Strategy
- **Unit (`plan.rs`):**
  - Hash-Stabilität gegen Formatierung, CRLF, Schlüsselreihenfolge und `null`.
  - Jede inhaltliche Änderung ergibt `Tampered`.
  - Feldformat und Versionsregeln (v1/v2 ohne, v3 mit Pflichtfeld, v4 unbekannt).
  - `save_replacing` atomar und ohne Restdatei.
  - Bestehende Validierungstests über `unsealed()`.
- **Unit (`ops`):** Warnung für ungeschützte Pläne in `check_with`, `seal_preview` für alle vier `Seal`-Zustände.
- **Integration (CLI, `assert_cmd` wie in den übrigen `tests/cli_*.rs`):** Exit-Code `3` ohne Dateioperation und ohne `runs`-Eintrag, v1-Warnung, Prüfsumme in der Ausgabe von `plan`, alle `plan seal`-Pfade, angepasste bestehende Tests.
- **Integration (GUI-Logik, `tests/gui_roundtrip.rs`):** geänderte Datei verhindert Apply, Teilplan ist versiegelt.
- **Rundreise:** `tests/roundtrip_random.rs` muss ohne Änderung grün bleiben, denn Pläne gehen dort durch `save`/`load`.
- **Manuell (`manual-tests.md`):**
  - Plan in Notepad öffnen und unverändert speichern ergibt keinen Fehlalarm.
  - Plan in Notepad ändern ergibt Exit-Code `3`.
  - `plan seal` danach lässt `apply` wieder durchlaufen.
  - GUI: Plan öffnen, Datei extern ändern, Anwenden führt zum Dialog „Plan neu laden“.
  - Alter Plan aus einem früheren Release zeigt die Warnung.

## Risks & Open Questions
- **Viele Tests bauen versiegeltes JSON um, um die Validierung zu prüfen.** Ohne Anpassung melden sie `Tampered` statt des erwarteten Fehlers. Gegenmaßnahme: Helfer `unsealed()` in Schritt 2. Dazu kommt die Reihenfolge in `parse`: Die Prüfsumme wird vor `validate` verglichen, damit ein veränderter Plan immer als „verändert“ (Exit-Code `3`) gemeldet wird.
- **Kanonische Form hängt an der Rust-Struktur.** Ändert ein späteres Release die Feldreihenfolge oder ein Default (`skip_serializing_if`), ändert sich der Hash alter v3-Pläne. Gegenmaßnahme: Ein Unit-Test mit einem fest eingecheckten v3-JSON samt Prüfsumme schlägt dann an. Jede solche Änderung braucht eine neue Formatversion.
- **`#[serde(flatten)]` und `deny_unknown_fields`:** `flatten` wird nur zum Serialisieren benutzt. Die Deserialisierung bleibt bei `Plan` mit `deny_unknown_fields`, also kein Konflikt.
- **Atomares Ersetzen mit `rename` unter Windows:** `std::fs::rename` ersetzt vorhandene Dateien (`MoveFileExW` mit `MOVEFILE_REPLACE_EXISTING`). In OneDrive-Ordnern kann ein Sync-Lock das Ersetzen kurz verhindern. Dann gibt es eine klare Fehlermeldung, kein Retry.
- **Bekannte Lücke laut Spec:** Version auf 2 zurücksetzen und Feld löschen ergibt nur eine Warnung. Sie wird dokumentiert und nicht behoben.
- [ ] Soll die GUI einen Knopf „Versiegeln“ bekommen? Mein Vorschlag: nein. Die Spec verlangt es nicht, und Handänderungen sind ein CLI-Thema.
