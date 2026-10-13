---
title: "Bug: Verlauf-Tabelle (Spaltenbreiten, schmale Ansicht), Detailtext ohne Umbruch, Summenzeile der Quarantäne nicht sichtbar"
status: fixed
created: 2026-10-13
updated: 2026-10-14
---

# Bug: Verlauf-Tabelle und Detailtext, Summenzeile der Quarantäne

Gefunden beim Handtest Quarantäne-Ansicht (`docs/manual-tests.md`, Abschnitt 1c), Release-Build von `dev` (6f238da), Testordner `D:\ordner-test\testordner`.

## Observed Behavior
1. Auf der Seite „Quarantäne“ ist keine Summenzeile zu sehen.
2. Seite „Verlauf“, Tabelle der Läufe: In schmalem Fenster werden nicht alle Spalten angezeigt. In breitem Fenster sind die Spalten zu schmal, vor allem die Spalte „Ordner“.
3. Verlauf → „Details“: Die Zeilen `Quelle → Ziel  [Status]` werden in schmalem Fenster nicht umgebrochen. Beispiel der Zeile (vom Nutzer kopiert):
   `D:\ordner-test\testordner\Downloads\Thumbs.db → D:\ordner-test\testordner\.ordner-cleanup\quarantine\20261013-224025-d391\Downloads\Thumbs.db  [zurückgeholt als D:\ordner-test\testordner\Downloads\Thumbs (2).db]`

## Expected Behavior
1. Über der Tabelle steht „7 Einträge, 665 KiB, nächster Ablauf in 30 Tagen“ (Spec `features/quarantaene-ansicht.md`, Punkt 4 der Seitenbeschreibung).
2. Die Lauf-Tabelle zeigt in jeder Fensterbreite alle Spalten (bei Platzmangel mit horizontalem Scrollen oder abgeschnittenen Texten und Tooltip). Die Spalte „Ordner“ hat genug Breite für einen gekürzten Pfad.
3. Die Detailzeilen brechen am Fensterrand um oder sind horizontal scrollbar.

## Reproduction Steps
1. `ORDNER_CLEANUP_HOME=D:\ordner-test-home`, GUI starten, Wurzel `D:\ordner-test\testordner` (3 offene Läufe in der Quarantäne).
2. Seite „Verlauf“: Fenster schmal ziehen (Spalten rechts fehlen), breit ziehen („Ordner“ klein). Bei einem Lauf „Details“.
3. Seite „Quarantäne“ (`Strg+6`): Summenzeile suchen.

Zu Punkt 1 liegt keine Beschreibung des Bildschirms vor (siehe Hypothese S1). Die Punkte 2 und 3 sind nur aus der Nutzerbeobachtung und aus dem Code abgeleitet, nicht per Bildschirmfoto belegt.

## Environment
- Branch `dev` (6f238da), Windows 11, Release-Build, `egui`/`eframe` 0.36.

## Root Cause Hypotheses

### Verlauf-Tabelle (Punkt 2)
| # | Hypothesis | Probability | Evidence |
|---|-----------|-------------|----------|
| V1 | Die Läufe stehen in einem `egui::Grid` in einer **nur vertikalen** `ScrollArea`. Ein Grid nimmt die Breite seiner Inhalte, es hat keine Spaltenlogik für wenig Platz. Wird das Fenster schmaler als die Summe der Spalten, ragt das Grid über den Rand und die rechten Spalten („Läuft ab“, Knöpfe „Details“, „Inhalt ansehen“, „Rückgängig …“) sind nicht erreichbar, weil es kein horizontales Scrollen gibt. | High | `history.rs:176-177` (`ScrollArea::vertical()`, `Grid::new("runs")`), 7 Spalten plus 3 Knöpfe in `ui.horizontal` (`history.rs:197-219`) |
| V2 | Die Spalte „Ordner“ ist eine `short_path_cell` mit `Label::truncate()`. In einem Grid hat ein abschneidendes Label **keine natürliche Breite**, die Spalte schrumpft auf die Mindestbreite und der Pfad wird zu wenigen Zeichen abgeschnitten, obwohl das Fenster breit ist. Der Text ist zusätzlich schon vorab auf 40 Zeichen gekürzt (`short_path(…, 40)`), das Label kürzt ein zweites Mal. | High | `history.rs:191` (`short_path_cell(ui, &row.root_full, 40, shell)`), `widgets/table.rs:62-70` (`.truncate()` plus `short_path`) |
| V3 | Die Übersicht (`overview.rs`, „Letzte Läufe“) nutzt dieselbe Tabellenform und `short_path_cell`. Das Problem dort ist wahrscheinlich dasselbe. | Medium | `overview.rs:166` |

Die Quarantäne-Tabelle (`quarantine.rs:722-760`) verwendet dagegen `TableBuilder` mit festen und `remainder`-Spalten und rechnet die Breite der Flex-Spalten selbst aus. Diese Form ist das Vorbild für die Verlauf-Tabelle.

### Detailtext (Punkt 3)
| # | Hypothesis | Probability | Evidence |
|---|-----------|-------------|----------|
| D1 | `ui.label(RichText::monospace())` bricht in egui nur an Wortgrenzen (Leerzeichen und Trennzeichen) um. Die Zeile besteht zu großen Teilen aus Pfaden ohne Leerzeichen. Ein Pfad wie `D:\…\.ordner-cleanup\quarantine\20261013-224025-d391\Downloads\Thumbs.db` ist ein einziges „Wort“ und bleibt auf einer Zeile, bis es nicht mehr hineinpasst. Es fehlt `Label::wrap()` (so wie an anderen Stellen, `cleanup.rs:439`, `help_window.rs:172`). | Medium | `history.rs:226-232` (nur `ui.label(...monospace())`); andere Stellen mit Umbruch rufen `.wrap()` explizit auf |
| D2 | Die breite Grid-Tabelle (V1) kann die Mindestbreite des umgebenden `ScrollArea`-Inhalts nach oben ziehen. Dann wäre die Breite für den Umbruch der Detailzeilen größer als das sichtbare Fenster. Dieselbe Ursache wie V1. | Medium | `history.rs:176-232`, Detailzeilen liegen im selben Scroll-Inhalt wie das Grid |

Ob D1 oder D2 greift, ist nur durch Ausprobieren zu klären (Fix mit `.wrap()` plus Test bei schmalem Fenster).

### Summenzeile (Punkt 1)
| # | Hypothesis | Probability | Evidence |
|---|-----------|-------------|----------|
| S1 | Die Zeile ist im Code vorhanden und wird bei jedem Aufbau der Seite gezeichnet: `ui.label(totals_text(...))` (`quarantine.rs:589`), Test `summentext_nennt_anzahl_groesse_und_frist` (`:1063`). Sie steht in normaler Textfarbe direkt unter der Filterzeile und über der Tabelle und ist leicht zu übersehen, wenn die Seite nicht wie erwartet aussieht (z. B. Filterzeile umbricht). Wahrscheinlich liegt hier ein Missverständnis über den Ort vor, nicht ein fehlender Code. | Medium | `quarantine.rs:154-187`, `:571-605` |
| S2 | Die Seite zeigt den Inhalt der Quarantäne nicht, etwa weil beim Aufruf „Wird geladen …“ stehen bleibt oder der Filter (Wurzel/Lauf) alles ausblendet. Dann stünde statt der Summe der Text „QUARANTINE_NO_MATCH“. Der Nutzer hat beim Test aber Einträge gesehen (er hat Einträge zurückgeholt). | Low | `quarantine.rs:577-604` |
| S3 | Der Nutzer meint die Summenzeile **im Verlauf** (Spec: „Summenzeile soll zu Verlauf passen“). Im Verlauf gibt es keine Summenzeile, nur die Tabelle und darunter den Block „Quarantäne leeren“. | Medium | `history.rs:259-276` |

Offene Frage an den Nutzer: Welche Seite und welcher Bildschirminhalt? Ein Bildschirmfoto der Seite „Quarantäne“ mit mindestens zwei Einträgen klärt S1 bis S3. Ohne diese Angabe wird Punkt 1 nicht angefasst.

## Affected Code
| File | Line | Description |
|------|------|-------------|
| `src/gui/views/history.rs` | 176-177, 180-221 | `Grid` in vertikaler `ScrollArea`, 7 Spalten und 3 Knöpfe, kein horizontales Scrollen |
| `src/gui/views/history.rs` | 191 | Spalte „Ordner“ über `short_path_cell` (zweifach gekürzt) |
| `src/gui/views/history.rs` | 224-233 | Detailzeilen: `ui.label` ohne `.wrap()` |
| `src/gui/widgets/table.rs` | 62-70 | `short_path_cell`: `.truncate()` in Grid-Zelle |
| `src/gui/views/overview.rs` | 166 | gleiche Tabellenform, vermutlich gleiches Problem |
| `src/gui/views/quarantine.rs` | 571-605 | Seitenaufbau, Summenzeile in Zeile 589 |

## Recommended Next Step
`/debug:fix-plan`. Vorschlag für den Plan:
1. Verlauf-Tabelle auf `TableBuilder` umstellen (wie `quarantine.rs:722`), mit `Column::remainder().clip(true)` für „Ordner“, Mindestbreiten, horizontalem Scrollen (`ScrollArea::horizontal` oder `TableBuilder::vscroll` plus Scroll auf der Seite) und Tooltip für abgeschnittene Texte. Dieselbe Tabelle in der Übersicht prüfen.
2. Detailzeilen mit `egui::Label::new(...).wrap()` ausgeben.
3. Punkt 1 erst nach Rückmeldung des Nutzers.
