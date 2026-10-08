---
title: "Fix-Plan: Layout der Aufräumen-Seite, Review-Tabelle und Regel-Formular"
status: pending-approval
created: 2026-10-08
updated: 2026-10-08
analysis: cleanup-page-layout-analysis.md
---

# Fix-Plan: Scrollen, Kartenbreite, Spaltenabstand, Feldausrichtung

## Gewählter Fix
Vier kleine, voneinander unabhängige Änderungen, jede für sich rückholbar. Es ändert sich nur die Darstellung, keine Logik.

1. **Aufräumen-Seite scrollt.** In `cleanup.rs::ui` den Formularzweig (`form_ui`) und den Ergebniszweig (`apply.result_ui`) in `egui::ScrollArea::vertical().auto_shrink([false, false])` setzen. Die Review-Ansicht bleibt **ohne** äußere ScrollArea: ihre Tabelle rechnet ihre Höhe aus `ui.available_height()` (`review.rs:255`) und scrollt selbst; in einer ScrollArea wäre die verfügbare Höhe unbegrenzt und die Tabelle würde riesig. Die ScrollArea reserviert den Platz für die Scrollleiste rechts und beseitigt damit das „Rand rechts fehlt“ (Hypothese 2) und das Abschneiden unten (Hypothese 1).
2. **Kartenbreite aus dem Platz berechnen.** Neue reine Funktion `card_layout(avail_width, chrome, spacing) -> (columns, card_width)` in `cleanup_cards.rs`:
   - `chrome` ist die Summe aus Innenrand und Strichbreite der Karte (aus `frame.total_margin()` ermittelt, nicht als Zahl im Code geraten),
   - `columns = ((avail + spacing) / (CARD_MIN_WIDTH + chrome + spacing)).floor().max(1)`,
   - `card_width = ((avail + spacing) / columns - spacing - chrome).clamp(CARD_MIN_WIDTH, CARD_MAX_WIDTH)`.
   - `CARD_MIN_WIDTH = 200`, `CARD_MAX_WIDTH = 300` ersetzen das feste `CARD_WIDTH = 230`. `cards_ui` nutzt die Funktion statt `/ (CARD_WIDTH + 24.0)`. So füllen die Karten die Zeile gleichmäßig und laufen nie über den Rand.
3. **Optionszeilen umbrechen.** In `options_ui` und `duration_row` (`cleanup.rs:449-558`) `ui.horizontal` durch `ui.horizontal_wrapped` ersetzen.
4. **Review-Tabelle: Abstand und Trennlinien.** In `review.rs::table`:
   - Hilfsfunktion `cell_padding(ui)`, die links `ui.add_space(6.0)` einfügt, und `cell_rule(ui)`, die am rechten Zellrand mit `ui.painter().vline(x, rect.y_range(), ui.visuals().widgets.noninteractive.bg_stroke)` eine dünne Linie zeichnet (Farbe aus dem Theme, damit sie in hell und dunkel passt).
   - Für alle Datenspalten und die Kopfzellen aufrufen. Die rechtsbündige „Größe“-Zelle bekommt zusätzlich rechts `add_space(8.0)`.
   - Spalte „Größe“: `TCol::initial(90.0).at_least(70.0).resizable(true).clip(true)`. „Alter“: `at_least(60.0)`.
5. **Regel-Formular: gleiche Ausrichtung und Breite.**
   - In `widgets/form.rs` die Konstanten `LABEL_COL_WIDTH` und eine neue `FIELD_WIDTH = 420.0` auf `pub(crate)` setzen und die zwei festen `420.0` in `text_field` (Z. 191, 216) durch `FIELD_WIDTH` ersetzen.
   - In `settings/rules.rs` (Z. 106-195): Die Beschriftungszelle von „Name“ und „Ziel“ bekommt `ui.set_min_width(form::LABEL_COL_WIDTH)` (wie `form::field`), die Eingabefelder `desired_width(form::FIELD_WIDTH)` statt 320/420. Die zwei Mehrzeilenfelder („Felder“, „Felder per Regex“, Z. 172-195) ebenfalls `FIELD_WIDTH`.
   - Ergebnis: Name, Ziel und die Felder aus `form::show` beginnen in derselben Spalte und enden bündig.

**Verworfen:**
- *Eine ScrollArea um die ganze Seite inklusive Review:* bricht die Höhenrechnung der Tabelle (siehe oben).
- *Spaltenzahl fest nach Fensterbreitenstufen (1/2/3):* funktioniert bei Zwischenbreiten schlecht; die Formel passt sich an jede Breite an.
- *Eigene Tabellenbibliothek oder `egui::Grid` statt `TableBuilder`:* viel zu groß für ein Darstellungsproblem; die Trennlinien lassen sich in den Zellen zeichnen.

**Rückweg nach „Anwenden“ (Punkt 3 der Analyse):** kein eigener Code-Fix, weil keine Ursache gefunden wurde. Nach Änderung 1 prüfen, ob es noch auftritt. Wenn ja, neue Analyse mit Bildschirmbeschreibung.

## Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/gui/views/cleanup.rs` | `ui` (Z. 302-346) | Formular- und Ergebniszweig in `ScrollArea::vertical().auto_shrink([false, false])` |
| `src/gui/views/cleanup.rs` | `cards_ui` (Z. 383-441), Konstanten (Z. 40-41) | `card_layout` benutzen; `CARD_WIDTH` entfällt, `CARD_MIN_WIDTH`/`CARD_MAX_WIDTH` neu |
| `src/gui/views/cleanup.rs` | `duration_row`, `options_ui` (Z. 443-583) | `horizontal` → `horizontal_wrapped` |
| `src/gui/views/cleanup_cards.rs` | neu | `pub fn card_layout(avail, chrome, spacing) -> (usize, f32)` plus Tests |
| `src/gui/views/review.rs` | `table` (Z. 250-348) | Zellabstand, Trennlinien, Spaltenbreiten |
| `src/gui/widgets/form.rs` | Z. 36, 191, 216 | `LABEL_COL_WIDTH` und `FIELD_WIDTH` als `pub(crate)` |
| `src/gui/views/settings/rules.rs` | Z. 106-195 | Name/Ziel/Felder auf `LABEL_COL_WIDTH` und `FIELD_WIDTH` |

## Tests
Zeichnen lässt sich nur von Hand prüfen. Automatisch geprüft wird die Rechnung:
- `card_layout` (vor dem Fix gibt es sie nicht, die Tests werden zuerst geschrieben):
  - sehr schmal (z. B. 150 px) → 1 Spalte, Breite mindestens `CARD_MIN_WIDTH`,
  - mittel (z. B. 560 px) → 2 Spalten, `columns * (card_width + chrome) + (columns - 1) * spacing <= avail`,
  - breit (z. B. 1400 px) → mehrere Spalten, Kartenbreite höchstens `CARD_MAX_WIDTH`,
  - Eigenschaft: für alle Breiten von 100 bis 2000 in Schritten von 7 passt die Zeile in `avail` (außer bei 1 Spalte unter `CARD_MIN_WIDTH`).
- Bestehende Tests (1664 Tests, `cargo test`), `cargo clippy -- -D warnings`, `cargo fmt --check` bleiben grün. Der Rauchtest `tests/gui_cleanup_cards.rs` läuft weiter.
- Handtest in `docs/manual-tests.md` ergänzen (Abschnitt 5c/5d):
  - Fenster auf linke und rechte Bildschirmhälfte: Karten füllen die Breite, rechts ist derselbe Rand wie links.
  - Fenster niedrig ziehen: die Seite scrollt, alle Optionen und „Plan erzeugen“ sind erreichbar.
  - Review-Tabelle: zwischen Größe, Grund / Regel und Alter sind Linien und Abstand sichtbar, in hell und dunkel.
  - Einstellungen → Regeln: Name, Ziel und Felder beginnen und enden bündig.
  - Versionen-Plan anwenden, „Weiter“ → Karten sind wieder da (Punkt 3 aus der Analyse).

## Risiken
- **Review:** Die Linien werden in jeder Zelle gezeichnet; bei 110.000 Zeilen nur für sichtbare Zeilen (`body.rows` zeichnet nur diese). Kein messbarer Mehraufwand erwartet, trotzdem beim Ruckel-Test (manual-tests Abschnitt 4) gegenprüfen.
- **Kartenhöhe:** Schmalere Karten umbrechen den Text häufiger; `CARD_HEIGHT` (110) bleibt als Mindesthöhe, der Text kann bei 200 px etwas höher werden. Bei Abschneiden `set_min_height` anheben.
- **`auto_shrink([false, false])`:** die ScrollArea nimmt immer den ganzen Platz; es entsteht kein sichtbarer Unterschied, solange der Inhalt passt.
- **Regel-Formular:** `form::show` und die Regelmaske teilen sich jetzt die Konstanten; eine spätere Änderung wirkt auf beide.

## Rollback
Jede der fünf Änderungen steht in einem eigenen Commit, `git revert <commit>` macht sie einzeln rückgängig.

## Reihenfolge für `/debug:fix`
1. `card_layout` mit Tests (rot → grün), dann `cards_ui` umstellen.
2. ScrollArea um Formular und Ergebnis.
3. `horizontal_wrapped`.
4. Review-Tabelle.
5. Regel-Formular.
6. `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`, Handtests, `status: fixed` in Analyse und Fix-Plan (nach `verification-before-completion`).
