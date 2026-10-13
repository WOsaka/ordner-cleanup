---
title: "Fix-Plan: Verlauf-Tabelle (Spalten, schmale Ansicht) und Umbruch der Detailzeilen"
status: fixed
created: 2026-10-13
updated: 2026-10-14
analysis: history-quarantine-layout-analysis.md
---

# Fix-Plan: Verlauf-Tabelle und Detailzeilen

Betrifft die Punkte 2 und 3 der Analyse. Punkt 1 (Summenzeile der Quarantäne) entfällt: Der Nutzer hat bestätigt, dass die Zeile da ist.

## Gewählter Fix

### A. Lauf-Tabelle im Verlauf auf `TableBuilder` umstellen (Hypothesen V1, V2)
Die Tabelle der Läufe (`history.rs:178-221`) wird vom `egui::Grid` auf `egui_extras::TableBuilder` umgestellt, wie die Quarantäne-Tabelle (`quarantine.rs:722-760`), mit zwei Unterschieden:
- Die Tabelle liegt weiter in der vertikalen `ScrollArea` der Seite, also `TableBuilder::vscroll(false)`. Die Zeilen scrollen mit der Seite, nicht in einem eigenen Rahmen.
- Um die Tabelle liegt eine `ScrollArea::horizontal()` (eigene `id_salt("runs-h")`). Ist das Fenster schmaler als die Summe der Spalten, lässt sich nach rechts scrollen; keine Spalte fällt weg. Der Detailblock, „Quarantäne leeren“ und die Wurzelliste liegen **außerhalb** dieser horizontalen Scroll-Fläche und behalten die Fensterbreite.

Spalten (feste Breiten in Pixeln, der Rest fließt in „Ordner“):

| Spalte | Breite | Zelle |
|---|---|---|
| Wann | exakt 130 | `ui.label` mit Tooltip (wie bisher) |
| Ordner | `flex` = `max(avail − Summe der festen Spalten − Abstände, 220)` | `short_path_cell` mit `max_chars = flex / 7.5` (mindestens 20), Tooltip mit dem vollen Pfad (bleibt) |
| Was | exakt 140 | `ui.label` |
| Ergebnis | exakt 150 | `status_cell` |
| Größe | exakt 90 | `ui.label` |
| Läuft ab | exakt 100 | `ui.label` |
| Knöpfe | exakt 330 | wie bisher: Details, Inhalt ansehen, Rückgängig … |

Die Rechnung steckt in einer reinen Funktion `fn run_columns(avail: f32) -> RunColumns` (Struktur mit `flex`, `root_chars`, Summe), damit sie ohne Fenster testbar ist.

`ROOT_CHARS` in `format.rs:316` (32) und das feste `40` in `history.rs:191` entfallen als Vorkürzung zugunsten von `root_chars`. `RunRow::root` bleibt, solange die Übersicht es nutzt (Prüfen mit `grep`; sonst in der Aufräumrunde entfernen).

### B. Detailzeilen umbrechen (D1, D2)
Die Zeilen im Block „Lauf …“ (`history.rs:224-233`) werden nicht mehr mit `ui.label(RichText::monospace)` gezeichnet, sondern mit einem `egui::text::LayoutJob` mit `wrap.max_width = ui.available_width()` und `wrap.break_anywhere = true`. Damit brechen auch Pfade ohne Leerzeichen am Fensterrand um. Der Text bleibt monospace und markierbar (kopierbar ohne Sonderzeichen). Eine kleine Hilfsfunktion `wrapped_mono(ui, text)` in `widgets/table.rs` (oder in `history.rs`, wenn sie nur dort gebraucht wird).

Der Detailblock liegt außerhalb der horizontalen Scroll-Fläche aus A, damit `available_width()` die Fensterbreite ist (D2 entfällt dadurch).

## Alternativen, verworfen
- **Grid behalten und `ScrollArea::both()` um die ganze Seite**: Das Grid hätte weiter keine definierte Breite für „Ordner“, und alle Abschnitte würden breiter als das Fenster. 
- **Spalten verstecken, wenn das Fenster schmal ist** (responsive): Bei vier bis sieben Spalten schwer lesbar und für den Nutzer nicht auffindbar. Horizontales Scrollen verliert keine Information.
- **Zero-Width-Space nach jedem `\`** für den Umbruch: verändert den kopierten Text (Pfade mit unsichtbaren Zeichen). `break_anywhere` tut es nicht.
- **Detailzeilen in einer eigenen horizontalen Scroll-Fläche**: Lange Pfade lesen sich schlechter als umbrochen.

## Änderungen im Einzelnen
| Datei | Zeilen | Änderung |
|---|---|---|
| `src/gui/views/history.rs` | 176-221 | `Grid` durch `ScrollArea::horizontal()` + `TableBuilder` ersetzen, Kopfzeile mit `ui.strong`, Zellen wie bisher, Knöpfe unverändert |
| `src/gui/views/history.rs` | 224-233 | Detailzeilen mit `wrapped_mono` |
| `src/gui/views/history.rs` | neu | `RunColumns` und `run_columns(avail)` mit Unit-Tests |
| `src/gui/widgets/table.rs` | neu | `wrapped_mono(ui, &str)` (LayoutJob mit `break_anywhere`) |
| `src/gui/format.rs` | 316, 329 | `ROOT_CHARS` und `RunRow::root` nur noch, wenn die Übersicht sie braucht |

Nicht Teil dieses Fixes: Die Wurzelliste „Gescannte Ordner“ in der Übersicht (`overview.rs:152`) und „Gescannte Wurzeln im Index“ (`history.rs:277`) haben dieselbe Grid-Form; sie wurden im Handtest nicht beanstandet. Beim Handtest-Durchgang in Block E (schmale Fenster) mitprüfen, bei Befund eigener Fix.

## Tests
1. **Unit** (`history.rs`, `mod tests`): `run_columns(1400.0)`: `flex` ≥ 220 und `root_chars` ≥ 30; `run_columns(400.0)`: `flex == 220`, `root_chars == 29`, `total` > 400 (Scrollen nötig); `run_columns(0.0)` und Negativwerte ergeben dieselben Mindestwerte, kein Absturz.
2. **Unit** (`widgets/table.rs`): `wrapped_mono` bricht eine 200 Zeichen lange Zeichenkette ohne Leerzeichen bei `max_width = 300` in mehrere Zeilen um (Galley `rows.len() > 1`, jede Zeile ≤ 300 px).
3. **kittest** (`tests/gui_smoke.rs`): Neuer Test `verlauf_zeichnet_bei_schmaler_und_breiter_flaeche`: `Harness` mit `with_size` 500×700 und 1600×700, `HistoryView` mit zwei Läufen und aufgeklapptem Detail (über `set_data`/Testzugriff, falls nicht vorhanden: `pub(crate) fn test_with(...)` hinter `#[cfg(test)]`-freiem, aber `#[doc(hidden)]` Zugriff wie bei `QuarantineView::set_list`). Erwartet: zeichnet ohne Panic, „Rückgängig …“ ist im Accesskit-Baum vorhanden (nicht abgeschnitten, nur scrollbar), „Details“ ebenso.
4. **Regression**: `cargo test` komplett, `cargo clippy -- -D warnings`, `cargo fmt --check`. Der bestehende Test `alle_seiten_lassen_sich_ohne_daten_zeichnen` bleibt unverändert grün.
5. **Handtest** (Nachweis für `manual-tests.md` 1c und 5b): Verlauf bei 100 %, 150 % Skalierung; Fenster schmal (rund 600 px) und breit (volle Breite): alle Spalten erreichbar bzw. sichtbar, „Ordner“ zeigt einen sinnvoll gekürzten Pfad; „Details“ bei einem Lauf mit langem Pfad: Zeilen brechen um, kein Seitwärtsscrollen im Detailblock.

## Regressionsrisiken
- **Zeilenhöhe/Optik**: `TableBuilder` hat eine feste Zeilenhöhe (`ROW_H`, hier 24 px). Die Knopfreihe passt hinein (Standardknopf ~22 px). Bei 200 % Skalierung prüfen.
- **Scrollen in zwei Richtungen**: Eine horizontale `ScrollArea` in einer vertikalen kann beim Mausrad stören (Mausrad scrollt vertikal weiter, Umschalt+Mausrad horizontal). Verhalten ist üblich und in Ordnung; in Block E mit Touchpad prüfen.
- **Striping/Auswahl**: Die Läufe haben keine Auswahl, kein Zustand geht verloren. Der Zustand `detail`, `filter_root` und Knopf-Absichten bleiben in `HistoryView`.
- **`short_path_cell` mit `max_chars`**: Bei sehr kleinem `flex` zeigt `short_path` nur den Kopf `D:\…`. Der Tooltip bleibt vollständig.

## Rollback
Reine GUI-Änderung in `history.rs` und `widgets/table.rs`, kein Datenformat. Bei Regression `git revert` des Fix-Commits; die Dateien sind vorher mit `cargo test` grün (Stand `6f238da`).

## Reihenfolge der Umsetzung
1. Test 3 (kittest-Rahmen) schreiben, lässt sich vor dem Fix ausführen und ist grün (Absturzschutz).
2. `run_columns` mit Test 1.
3. Tabelle umstellen (A).
4. `wrapped_mono` mit Test 2, Detailzeilen umstellen (B).
5. `cargo test`, `clippy`, `fmt`, Handtest 5.
6. Analyse und Fix-Plan auf `status: fixed`; `manual-tests.md` ergänzen.

## Umsetzungsstand (2026-10-13)
Code umgesetzt auf `dev`: `run_columns` und `TableBuilder` mit horizontaler `ScrollArea` in `history.rs`, `wrapped_mono` in `widgets/table.rs`, `HistoryView::set_runs` als Testzugang.
- Abweichung zu Test 2: Der Umbruch von `wrapped_mono` ist als kittest (`tests/gui_smoke.rs`, `wrapped_mono_bricht_pfade_ohne_leerzeichen_am_rand_um`) statt als Unit-Test geschrieben, weil er das Layout der Schrift braucht.
- Unit-Tests `run_columns` (`history.rs`), kittest `verlauf_zeichnet_bei_schmaler_und_breiter_flaeche_alle_knoepfe` (500 und 1600 px).
- `cargo test` (1913 Tests grün), `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` sauber.
- **Handtest (2026-10-14) bestanden:** Verlauf in schmalem und breitem Fenster sowie Details mit langem Pfad vom Nutzer geprüft und für gut befunden. Status `fixed`.
