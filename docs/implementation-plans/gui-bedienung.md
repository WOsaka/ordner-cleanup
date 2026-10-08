---
title: "Implementation Plan: GUI-Bedienung: schneller und klarer"
feature_spec: docs/features/gui-bedienung.md
status: pending-approval   # pending-approval | approved | implemented
created: 2026-10-08
updated: 2026-10-08
---

# Implementation Plan: GUI-Bedienung: schneller und klarer

## Summary

Die Änderung betrifft nur `src/gui/` und kleine, rein lesende Hilfen in der Bibliothek. Plan-, Journal- und Config-Formate bleiben unberührt. Die Umsetzung hat **vier Stufen**, jede mit eigenem Commit-Block, grünem `cargo test`/`clippy`/`fmt` und eigenem Merge. Die Reihenfolge ist so gewählt, dass spätere Stufen auf früheren aufbauen:

0. **Vorarbeit (klein, vor Stufe 1):** `gui/theme.rs` mit den Farb-Tokens und `gui/format.rs` mit den reinen Formatfunktionen. Beides wird von Stufe 1 und 2 gebraucht (Status-Farben, Kopfleisten-Text).
1. **Kopfleiste:** neues `gui/header.rs`. `ScanPanel` löst sich auf, seine Optionen wandern in einen Dialog. Die Scan- und Classify-Tasks gehören nicht mehr zu `Route::Overview`, sondern zu einer neuen `Route::Header`. `Persisted` bekommt die zuletzt benutzten Ordner.
2. **Rohausgaben:** `format.rs` wird in Übersicht, Verlauf, Scan-Ergebnis und Tabellen eingesetzt.
3. **Aufräumen:** `CleanupView::form_ui` wird Karten-Ansicht. Die Zahlen kommen aus `Report` (Duplikate) und `HistorySection.metrics` (Müll, leere Ordner), also ohne neue Analyse.
4. **Review, Tastatur, Theme:** `ReviewState` bekommt feste Kopfzeile, Auswahl-Menü und Tastatur-Logik (als fensterfreie Funktionen in `gui/review.rs`). Alle `Color32`-Stellen ziehen auf `theme.rs` um.

Die Codeanalyse hat drei Dinge ergeben, die den Entwurf konkretisieren:

- Der Report enthält **Duplikate** direkt (`DuplicatesReport { group_count, total_wasted }`). **Müll** und **leere Ordner** stehen in `HistorySection.metrics` (`MetricRow` mit `key`, z. B. `empty_dirs`) und für leere Ordner zusätzlich in `structure` mit `issue == "empty"`. `HistorySection` ist aber `Option`, weil der Aufrufer sie setzt. Ohne Verlauf entfallen diese zwei Karten-Zahlen, die Karte zeigt dann nur die Beschreibung.
- Es gibt keinen Windows-Dunkelmodus-Zugriff im Code. egui 0.36 kennt `ThemePreference::System`, das den Systemmodus selbst verfolgt (offen: siehe Risiken).
- `resolve_rules_path` und `RuleSet::load` existieren. Für die Häkchenliste reicht ein Parsen im Worker (`TaskKind::Read`), es braucht keine neue Fachlogik.

## Files to Create
| File | Purpose |
|------|---------|
| `src/gui/theme.rs` | Farb-Tokens (Erfolg, Warnung, Fehler, gedämpft, Akzent, Gefahr) für hell/dunkel, `apply(ctx, preference)`, Abstands- und Schriftskala, `ThemeChoice` (System/Hell/Dunkel) |
| `src/gui/format.rs` | Reine Funktionen: `run_status`, `scan_status`, `relative_time`, `short_path`, `score_label`, `duration_to_arg`, `stale_note`, Zahlen-/Größenformat (zieht `texts::bytes/grouped` mit) |
| `src/gui/header.rs` | `Header`-Zustand und Zeichnung der Kopfleiste, Ordner-Dropdown mit Zuletzt-Liste, Scan-Optionen-Dialog, Fortschrittsanzeige, Scan-/Classify-Ergebnis als Kacheln |
| `src/gui/keys.rs` | Tastatur-Abbildung: `fn action(input: &KeyInput, ctx: KeyContext) -> Option<KeyAction>` als reine Logik, die egui-Eingaben in `KeyAction` übersetzt (testbar ohne Fenster) |
| `src/gui/views/cleanup_cards.rs` | Karten für die sechs Plan-Arten, Zahlen-Ermittlung (`CardFacts::from_report`), Dauer-Felder, Regel-Häkchen |
| `tests/gui_header.rs` | Rauchtest mit `egui_kittest`: Kopfleiste, Dropdown, Leerzustand (Stufe 1) |
| `tests/gui_cleanup_cards.rs` | Rauchtest: Karten mit/ohne Zahlen, Dauer-Eingabe (Stufe 3) |

## Files to Modify
| File | Change Description |
|------|--------------------|
| `src/gui/mod.rs` | Neue Module (`theme`, `format`, `header`, `keys`) deklarieren; Theme beim Start anwenden |
| `src/gui/shell.rs` | `Route::Header` ergänzen; `Choice`-Anzeige (`label`, `short_label`); `recent_targets: Vec<Choice>` mit `remember(choice)` (Begrenzung auf 8, Duplikate entfernen); `Page::icon()`; `Page::ALL`-Reihenfolge unverändert |
| `src/gui/app.rs` | Kopfleiste über der CentralPanel (außer `Page::Settings`); `Persisted` um `recent: Vec<Choice>` und `theme: ThemeChoice` erweitern (beides `#[serde(default)]`, alte gespeicherte Zustände bleiben lesbar); Tasks von `Route::Header` verteilen; globale Tastenkürzel verteilen; Sidebar mit Icons und Hinweis |
| `src/gui/texts.rs` | Neue Texte für Kopfleiste, Karten, Status, Hinweise, Tastenliste. `bytes`/`grouped` bleiben (delegieren an `format`) |
| `src/gui/views/overview.rs` | `ScanPanel` entfernen; Leerzustand mit großem Knopf; Wurzeln-Tabelle mit Statustext, relativer Zeit, gekürzten Pfaden; Profil-Zeile „Öffnen“ + Badge „Zum Prüfen“; letzte Läufe als `TableBuilder` mit „Rückgängig“-Aktion; Quarantäne-Zeile mit Knopf |
| `src/gui/views/scan.rs` | Wird zu den Scan-/Classify-Optionen (Datenstruktur und `start_scan`/`start_classify`) und zieht nach `header.rs` um; `scan_text` wird zu Kachel-Daten (`ScanSummary`), Tests ziehen mit |
| `src/gui/views/analysis.rs` | Feste Farben auf Theme-Tokens; Kopfzeile „Ziel“ entfällt (steht im Kopf) |
| `src/gui/views/analysis_tabs.rs` | Pfad-Kürzung, Zeitformat, Score-Einordnung aus `format` |
| `src/gui/views/cleanup.rs` | `form_ui` ersetzt durch Karten-Ansicht (`cleanup_cards`); `Zuletzt erzeugte Pläne` lesbar (Art, Zeit, Größe, Anzahl); Ziel-Label entfällt; `generate` übernimmt Dauer-Felder über `duration_to_arg`; Regel-Häkchen füllen `only` |
| `src/gui/views/review.rs` | Feste Kopfzeile (`egui::Panel::top` im Review-Bereich), Quarantäne-/Archiv-Hinweis aus `quarantine_days`, Menü „Auswahl ▾“, Zähler „Übersprungene (N)“, Tastatur über `keys.rs`, Zeilenfokus, rechtsbündige Zahlenspalte |
| `src/gui/review.rs` | `ReviewModel::move_focus`, `toggle_and_advance`-Logik (rein), `skipped_count()` |
| `src/gui/views/history.rs` | Lauf-Tabelle mit `format`; Details/IDs in Tooltips |
| `src/gui/views/apply.rs` | `target_text` und Statusfarben auf Theme-Tokens; Status in der Ergebnisansicht über `format::run_status` |
| `src/gui/widgets/table.rs` | `short_path_cell`: gekürzter Pfad mit Tooltip und Kontextmenü (baut auf `path_cell`/`path_menu` auf) |
| `src/gui/widgets/dialogs.rs` | Gefahr-Farbe aus Theme (`from_rgb(160,40,40)`), Esc schließt Dialog |
| `src/gui/widgets/form.rs`, `toml_editor.rs`, `settings/content.rs`, `settings/doc_editor.rs`, `settings/profiles.rs`, `settings/rules.rs` | Feste Farben → Theme-Tokens (nur Farben, keine Struktur) |
| `src/gui/views/settings/general.rs` bzw. `settings/mod.rs` | Auswahl „Darstellung: System / Hell / Dunkel“ (wirkt auf `theme`, gespeichert im eframe-Storage) |
| `src/ops/mod.rs` | Falls nötig: `status_label` öffentlich lassen, nichts ändern. `format::run_status` baut darauf auf, statt zu duplizieren |
| `docs/manual-tests.md` | Neuer Abschnitt: Kopfleiste, Darstellung bei 100/150/200 % DPI, hell/dunkel, Tastatur im Review |
| `docs/features/gui-bedienung.md` | `status: implemented` nach Abschluss (Schritt 20) |

## Data Model Changes
Keine Änderungen an Plan, Journal, Index, History oder `config.toml`.

- **`Persisted`** (eframe-Storage, nur Komfort): `recent: Vec<Choice>` und `theme: ThemeChoice`, beide mit `#[serde(default)]`. Ein älterer Zustand ohne die Felder lädt unverändert.
- **`Shell`**: `recent_targets` (Länge ≤ 8), gefüllt bei erfolgreicher Auswahl bzw. erfolgreichem Scan.
- **`Route`**: neues `Header`. `Overview` behält nur das Laden der Übersichtsdaten.
- **`format`-Typen:** `StatusStyle { text, symbol, tone }` mit `Tone::{Ok, Warn, Error, Muted}`. Die Zuordnung zu Farben liegt ausschließlich in `theme.rs`.
- **`CardFacts`:** `{ dedupe: Option<(groups, bytes)>, junk: Option<(files, bytes)>, empty_dirs: Option<u64>, scanned_at: Option<DateTime> }`, abgeleitet aus `Report` (aus `view.model.duplicates`, `history.metrics`, `structure`).

## API / Interface Changes
Nur GUI-intern; die öffentliche Bibliotheks-API wächst um die neuen Module unter `gui::`.

```rust
// gui/format.rs
pub fn relative_time(now: DateTime<Local>, t: DateTime<Local>) -> (String /*kurz*/, String /*tooltip*/);
pub fn short_path(path: &str, max_chars: usize) -> String;      // kürzt in der Mitte, trennt an '\'
pub fn run_status(s: RunStatus) -> StatusStyle;
pub fn score_label(score: u8) -> &'static str;                  // Schwellen aus health/score.rs
pub fn duration_to_arg(amount: &str, unit: DurationUnit) -> Result<Option<String>, String>; // "2","Jahre" -> "2y"

// gui/keys.rs
pub enum KeyAction { Next, Prev, Toggle, SelectFiltered, DeselectFiltered, FocusSearch,
                     Apply, OpenPlan, Reload, Page(Page), CloseDialog }
pub fn action(input: &InputSnapshot, ctx: KeyContext) -> Option<KeyAction>;

// gui/review.rs (Ergänzung)
impl ReviewModel {
    pub fn toggle_and_advance(&mut self, focus: usize) -> usize;
    pub fn skipped_count(&self) -> usize;
}
```

`KeyAction::Apply` löst **nur** den bestehenden Weg „Anwenden …“ aus (Prüfung → Bestätigungsdialog). Es gibt kein Kürzel für Rückgängig, Löschen oder Quarantäne leeren.

Für die Score-Einordnung: `health/score.rs` wird **nicht** verändert. `score_label` nutzt feste Bänder in `format.rs` (≥ 80 gut, 60–79 mittel, < 60 schwach) und dokumentiert, dass sie reine Darstellung sind. Damit entfällt die offene Frage aus der Spec zur Sichtbarkeit der Schwellen, die Teilwert-Schwellen in `score.rs` sind eine andere Größe.

## Implementation Sequence
Jeder Schritt ist ein Commit (kurze Message, kein Footer, laut CLAUDE.md) und endet grün. Tests werden nach `superpowers:test-driven-development` zuerst geschrieben.

**Vorarbeit**
1. **`format.rs` (rein, testgetrieben)** — `relative_time`, `short_path`, `run_status`, `score_label`, `duration_to_arg`, `stale_note`; `trend`, `review_text` und Größenformat ziehen aus `overview.rs`/`texts.rs` um, ihre Tests mit.
   - Files: `src/gui/format.rs`, `src/gui/mod.rs`, `src/gui/texts.rs`, `src/gui/views/overview.rs`
   - Depends on: nichts
2. **`theme.rs`** — Tokens hell/dunkel, `Tone → Color32`, `apply(ctx, ThemeChoice)`, Skala (Überschrift, Zeilenhöhe, Innenabstand). Noch nichts umgestellt, nur eingehängt (Start ruft `apply`).
   - Files: `src/gui/theme.rs`, `src/gui/mod.rs`, `src/gui/app.rs`
   - Depends on: Schritt 1 (`Tone`)

**Stufe 1: Kopfleiste**
3. **`Route::Header` und `recent_targets`** — Shell-Erweiterung mit Tests für `remember` (Duplikate, Grenze 8, ungültige Einträge nicht gespeichert).
   - Files: `src/gui/shell.rs`
   - Depends on: Schritt 1
4. **Scan-Optionen auslagern** — Optionen-Struktur samt `lines`/`opt`/Starts aus `scan.rs` in `header.rs`; zwei getrennte Abschnitte im Dialog „Scan-Optionen“. Verhalten gleich, Tests ziehen mit.
   - Files: `src/gui/header.rs`, `src/gui/views/scan.rs` (entfällt), `src/gui/views/mod.rs`
   - Depends on: Schritt 3
5. **Kopfleiste zeichnen** — Dropdown (Zuletzt · Profile · „Anderen Ordner wählen …“), Anzeige der tatsächlichen Auswahl, Scannen/Klassifizieren/Optionen, Fortschritt mit Abbrechen, Warnsymbol bei fehlendem Ordner, Scan-Ergebnis als Kacheln. Profile kommen aus einem eigenen Lade-Task (`ops::admin::profiles`), Config-Fehler erscheinen in der Profil-Gruppe.
   - Files: `src/gui/header.rs`, `src/gui/app.rs`
   - Depends on: Schritt 4, Schritt 2
6. **Übersicht entschlacken** — `ScanPanel`-Aufrufe raus, Leerzustand mit großem Knopf „Ordner wählen und scannen“, Neuladen bei `shell.generation`. `Persisted` um `recent`.
   - Files: `src/gui/views/overview.rs`, `src/gui/app.rs`
   - Depends on: Schritt 5
7. **Rauchtest und Handtest Stufe 1** — `tests/gui_header.rs`, Eintrag in `docs/manual-tests.md`. Danach Merge-Reife prüfen.
   - Files: `tests/gui_header.rs`, `docs/manual-tests.md`
   - Depends on: Schritt 6

**Stufe 2: Rohausgaben**
8. **Status, Zeiten, Pfade in Übersicht und Verlauf** — Lauf-Tabelle (`TableBuilder`: Wann · Ordner · Art · Ergebnis · Größe · Aktion), „Rückgängig“ über den bestehenden `UndoFlow` nur dort, wo das Journal es erlaubt. Wurzel-Tabelle mit Status ohne `{:?}`.
   - Files: `src/gui/views/overview.rs`, `src/gui/views/history.rs`, `src/gui/widgets/table.rs`
   - Depends on: Schritt 1, 7
9. **Kennzahl-Kacheln für Scan-/Classify-Ergebnis** — `ScanSummary`/`ClassifySummary` aus `ScanReport`/`ClassifyOutcome`, Kachelreihe plus Hinweisliste, Fehler-Klick öffnet Details. Score mit `score_label`.
   - Files: `src/gui/header.rs`, `src/gui/format.rs`
   - Depends on: Schritt 5, 8
10. **Quarantäne-Zeile, Profil-Zeile, Badge** — „Quarantäne leeren …“ springt nach Verlauf; „Öffnen“ setzt `shell.target`; Badge „Zum Prüfen“ löst `Goto { Analysis }` aus (Tab „Inhalte“ vorauswählen: `AnalysisView::preselect_tab`).
    - Files: `src/gui/views/overview.rs`, `src/gui/views/analysis.rs`, `src/gui/shell.rs`
    - Depends on: Schritt 8

**Stufe 3: Aufräumen mit Zahlen**
11. **`CardFacts` aus `Report`** — rein, mit Tests (mit/ohne `history`, veralteter Scan, leere Ordner über `structure`). `AnalysisView` stellt `ReportView` bereit, `CleanupView` lädt ihn selbst über `report_model` (`TaskKind::Read`), wenn keiner vorliegt.
    - Files: `src/gui/views/cleanup_cards.rs`, `src/gui/views/cleanup.rs`
    - Depends on: Schritt 1
12. **Karten-Ansicht** — sechs Karten mit Titel, Satz, Zahl; Stand-Hinweis nach 7 Tagen; Klick öffnet die Optionen; „Plan erzeugen“ als Hauptknopf. `preselect(kind)` bleibt für den Sprung aus der Analyse.
    - Files: `src/gui/views/cleanup_cards.rs`, `src/gui/views/cleanup.rs`, `src/gui/texts.rs`
    - Depends on: Schritt 11, 2
13. **Dauer-Felder** — Zahl + Einheit (`DurationUnit::{Days, Months, Years}`), Platzhalter aus der Config (`archive_older_than`/`versions_min_age`), Validierung über `parse_old_after`; leer = Config.
    - Files: `src/gui/views/cleanup_cards.rs`, `src/gui/format.rs`
    - Depends on: Schritt 12
14. **Duplikat-Strategie und Regeldatei** — Dropdown mit Erklärung, Ordnerdialog für „unterhalb von“. Regeldatei: Worker-Task `TASK_RULE_NAMES` lädt `RuleSet::load` (mit `KnownCategories` aus der Config) und liefert Namen oder `RulesError` (Datei, Zeile); `only` entsteht aus den Häkchen.
    - Files: `src/gui/views/cleanup_cards.rs`, `src/gui/views/cleanup.rs`
    - Depends on: Schritt 13
15. **Pläne-Liste und Rauchtest** — lesbare Zeilen mit Art/Zeit/Größe (Plan-Kopf lesen, wie `recent_plans` es schon tut, nur ergänzt um `kind` und `total_bytes`). `tests/gui_cleanup_cards.rs`.
    - Files: `src/gui/views/cleanup.rs`, `tests/gui_cleanup_cards.rs`
    - Depends on: Schritt 14

**Stufe 4: Review, Tastatur, Theme**
16. **Theme umziehen** — alle 18 `Color32`-Stellen auf `theme::tone(…)`; Gefahr-Knopf; danach `grep Color32::` außerhalb von `theme.rs` leer (als Test: Quelltext-Scan in `tests/`, siehe Teststrategie). Auswahl „Darstellung“ in den Einstellungen, in `Persisted` gespeichert.
    - Files: siehe „Files to Modify“ (Farbstellen), `src/gui/theme.rs`, `src/gui/views/settings/general.rs`, `src/gui/app.rs`
    - Depends on: Schritt 2
17. **Review-Kopfzeile, Hinweis, Auswahl-Menü** — feste Kopfzeile mit „Anwenden (N Einträge, X GB)“, Quarantäne-/Archiv-Hinweis (`quarantine_days` aus `ops::load_config`, einmal beim Öffnen des Plans gelesen), Menü „Auswahl ▾“, Zähler bei „Übersprungene“. Der bestehende Test `review_mit_tausend_eintraegen…` wird angepasst (Label „Anwenden …“ → neuer Text).
    - Files: `src/gui/views/review.rs`, `src/gui/review.rs`, `src/gui/texts.rs`, `tests/gui_smoke.rs`
    - Depends on: Schritt 16
18. **Tastatur** — `keys.rs` (rein), Zeilenfokus im Review, `toggle_and_advance`, globale Kürzel (Seitenwechsel, Plan öffnen, Neu laden, Esc). Textfelder haben Vorrang (`ctx.wants_keyboard_input()`). Hilfeliste hinter „?“, Kürzel in Tooltips.
    - Files: `src/gui/keys.rs`, `src/gui/views/review.rs`, `src/gui/review.rs`, `src/gui/app.rs`, `src/gui/widgets/dialogs.rs`
    - Depends on: Schritt 17
19. **Skala, Icons, Tabellen** — einheitliche Abstände (`add_space`-Werte durch Skala ersetzen), Seitenleisten-Icons aus der geladenen Symbolschrift mit Textfallback, gestreifte Tabellen, rechtsbündige Zahlen.
    - Files: `src/gui/theme.rs`, `src/gui/app.rs`, `src/gui/views/*.rs`
    - Depends on: Schritt 16
20. **Abschluss** — Handtests (DPI 100/150/200 %, hell/dunkel, Tastatur) in `docs/manual-tests.md` ergänzen, `verification-before-completion`, `status: implemented` in der Spec. Gesamtlauf `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`.
    - Files: `docs/manual-tests.md`, `docs/features/gui-bedienung.md`
    - Depends on: Schritt 19

Stufen 1–4 lassen sich jeweils nach den Schritten 7, 10, 15 und 20 mergen.

## Test Strategy
- **Unit (ohne Fenster), zuerst geschrieben:**
  - `format.rs`: relative Zeit (gestern, vor 2 Tagen, Zukunft → absolutes Datum, Wochengrenze), `short_path` (Umlaute, `\\?\`, > 260 Zeichen, kürzer als Limit), `run_status` für jeden `RunStatus`, `score_label` an den Bandgrenzen, `duration_to_arg` (Einheiten, 0, Text, Überlauf, leer = `None`).
  - `shell.rs`: `remember` begrenzt auf 8, ohne Duplikate, ohne leere Einträge.
  - `cleanup_cards.rs`: `CardFacts::from_report` mit/ohne Verlauf, mit leeren Ordnern, Scan älter/neuer als 7 Tage.
  - `keys.rs`: jede Kombination der Spec-Tabelle, Vorrang von Textfeldern, `Apply` nur mit Auswahl, kein Kürzel für schreibende Aktionen.
  - `review.rs`: `toggle_and_advance` am Listenende, bei leerer Liste und bei gefilterter Liste.
  - Bestehende Tests (`trend`, `scan_text`, `review_text`, `pending_quarantine`, `zeilen_und_optionen_werden_bereinigt`) bleiben grün und ziehen mit.
- **Integration (`egui_kittest`, AccessKit-Baum):** Kopfleiste mit Auswahl und Profil-Anzeige, Leerzustand der Übersicht, Karten mit und ohne Zahlen, Review mit fester Kopfzeile und Hinweis, Esc schließt einen Dialog. Der bestehende `gui_smoke.rs` wird nachgezogen, `gui_roundtrip.rs` muss unverändert grün bleiben (belegt, dass Plan-Format und Apply/Undo unberührt sind).
- **Statische Prüfung:** Ein Test liest `src/gui/**/*.rs` und schlägt fehl, wenn `Color32::` außerhalb von `theme.rs` und der Datei `widgets/toml_editor.rs` (Syntaxfarben, falls es dort nicht über Tokens läuft) vorkommt.
- **Manuell** (`docs/manual-tests.md`): Darstellung bei 100/150/200 % Skalierung, Windows hell/dunkel zur Laufzeit wechseln, Scan mit Abbruch aus der Kopfleiste, 100.000-Zeilen-Review mit Tastatur (Leistung), fehlende Schriften.
- **Regression:** `cargo test` komplett, damit CLI-Tests nachweisen, dass nichts außerhalb von `gui/` berührt wurde.

## Risks & Open Questions
- **Report-Daten für Müll/leere Ordner hängen an `HistorySection`.** Wenn der Verlauf fehlt (`None`), gibt es dort keine Zahl. Mitigation: Karte zeigt dann nur die Beschreibung; für leere Ordner zusätzlich Rückfall auf `structure` mit `issue == "empty"`. Wird in Schritt 11 per Test abgesichert. Eine eigene neue Zählung wäre eine neue Analysefunktion und ist ausgeschlossen.
- **Zusätzlicher Report-Aufbau beim Öffnen der Aufräumen-Seite** kostet bei großen Indizes Zeit. Mitigation: Hintergrund-Task, Ergebnis wird mit der `shell.generation` gecacht und nur nach Scan/Apply neu gebaut, die Karten erscheinen ohne Zahl, bis er fertig ist.
- **Kopfleiste und Task-Zuordnung:** Scan/Classify gehören bisher zu `Route::Overview`, wodurch `is_running(Route::Overview)` Knöpfe sperrt. Der Wechsel auf `Route::Header` muss `OverviewView::on_finished` und die Ladelogik (`shell.generation`) mitnehmen. Mitigation: Schritt 5 und 6 gemeinsam testen, der Rauchtest prüft „Scan fertig → Übersicht lädt neu“.
- **Tastatur und egui-Fokus:** Leertaste und Pfeile kollidieren mit fokussierten Widgets (Checkbox in der Zeile). Mitigation: Listen-Kürzel nur, wenn `!ctx.wants_keyboard_input()` und kein Dialog offen ist; die Zeilen-Checkbox bekommt keinen Tab-Fokus-Anspruch (`TableBuilder.sense(click)` bleibt).
- **Dunkelmodus folgt dem System:** egui 0.36 `ThemePreference::System` sollte den Windows-Modus verfolgen. Falls das zur Laufzeit nicht greift, ist der Rückfall ein Lesen der Registry `AppsUseLightTheme` über das vorhandene `windows-sys` (kein neues Crate), das beim Fensterfokus erneut geprüft wird. Wird in Schritt 2 mit einem Handtest geklärt.
- **Symbolschrift auf Windows 10:** Einige Symbole der Segoe-UI-Symbolschrift fehlen dort evtl. Mitigation: jede Icon-Zeichenfolge hat einen Textfallback ohne Symbol, getestet in `texts`.
- **Umfang:** Vier Stufen sind viel für einen Merge. Jede Stufe ist deshalb eigenständig lauffähig; bei Zeitdruck lässt sich nach Stufe 2 aufhören.
- [x] Score-Einordnung: drei feste Bänder in `format.rs` (≥ 80 gut, 60–79 mittel, < 60 schwach), reine Darstellung, Tooltip mit `top_deductions` (entschieden 2026-10-08)
- [x] Profil-Gruppe im Kopf-Dropdown: höhenbegrenzt mit Scrollleiste, kein Suchfeld (entschieden 2026-10-08)
- [x] Ordnerliste im Dropdown: zuletzt gewählte Ordner zuerst, danach alle gescannten Wurzeln aus dem Index, ohne Dubletten (entschieden 2026-10-08)
- [x] Anzahl gemerkter Ordner: 8 als Konstante (entschieden 2026-10-08)
- Technisch, im Schritt geklärt (keine Entscheidung nötig): Dunkelmodus zur Laufzeit (Schritt 2), Parsen der Regeldatei (Schritt 14, Worker-Task), Symbolschrift auf Windows 10 (Schritt 19, Textfallback)
