---
title: "Bug: Aufräumen-Seite der GUI: Rand rechts, Karten bei schmalem Fenster, kein Scrollen, Rückweg nach Anwenden"
status: analysed
created: 2026-10-08
updated: 2026-10-08
---

# Bug: Aufräumen-Seite der GUI (Layout, Scrollen, Rückweg zu den Karten)

Gefunden beim Handtest GUI-Bedienung Stufe 3 (`docs/manual-tests.md`, Abschnitt 5c), Debug-Build von `dev` (56fab63).

## Observed Behavior
1. Die Karten und der Inhalt der Seite „Aufräumen“ liegen rechts direkt am Fensterrand, links ist ein Rand da.
2. Ist das Fenster nur auf der linken oder rechten Bildschirmhälfte geöffnet, sind die Karten schlecht angeordnet.
3. Nach „Anwenden“ eines Versionen-Plans kommt der Nutzer nicht mehr zu den Karten zurück.

Kein Fehler: Die Kartenzahlen (Duplikate 6, Müll 3, Leer 1) stimmen mit den Testdaten überein. Die drei Dateien `Thumbs.db`, `~$lock.docx`, `x.tmp` hatten identischen Inhalt und bildeten eine sechste Duplikatgruppe. „Leer“ zählt nur Ordner ohne direkten Eintrag (`src/health/mod.rs:242`), `leer1` mit dem Unterordner `leer2` zählt nicht. Das bleibt unverändert.

## Expected Behavior
1. Rechts derselbe Rand wie links.
2. Die Karten ordnen sich nach der verfügbaren Breite an und laufen nicht über den Rand.
3. Nach dem Ergebnis führt „Weiter“ immer zurück zu den Karten, und die ganze Seite ist bei niedriger oder schmaler Fensterform erreichbar.

## Reproduction Steps
1. GUI starten (`cargo run --bin ordner-cleanup-gui`), Testordner scannen, Seite „Aufräumen“.
2. Fenster auf eine Bildschirmhälfte ziehen (Windows+Pfeil links/rechts), Karten ansehen.
3. Fenster niedrig ziehen: Optionen, „Plan erzeugen“ und „Zuletzt erzeugte Pläne“ sind nicht erreichbar.
4. Karte „Versionen“ wählen, Plan erzeugen, anwenden, im Ergebnis weiter: Punkt 3 prüfen.

Punkt 3 ist bisher nur vom Nutzer beobachtet, nicht reproduziert; was genau auf dem Bildschirm steht, ist offen.

## Environment
- Branch `dev` (56fab63), Windows 11, Debug-Build (`cargo run`)

## Root Cause Hypotheses
| # | Hypothesis | Probability | Evidence |
|---|-----------|-------------|----------|
| 1 | Die Seite „Aufräumen“ hat keine `ScrollArea`. Andere Seiten (`overview.rs:129`, `history.rs:165`, `analysis.rs:160`, Einstellungen) packen den Inhalt in `ScrollArea::vertical()`, `cleanup.rs::ui` nicht. Bei niedrigem Fenster wird alles unterhalb des Fensterrandes abgeschnitten (Optionen, „Plan erzeugen“, Pläne). Das kann auch den Rückweg (3) verdecken. | High für 2 (Teil „unten abgeschnitten“), Medium für 3 | `cleanup.rs:302-346` (`ui` ruft `form_ui` bzw. Review ohne Scroll auf), `cleanup.rs:585-649` (`form_ui` hat keine ScrollArea), `cleanup.rs:566` nur die Regelliste scrollt |
| 2 | Kein Rand rechts: Die Spaltenzahl `(available_width / (CARD_WIDTH + 24.0)) as usize` rechnet die Breite pro Karte knapp. Eine Karte braucht `230 + 2 × 6 (Frame-Innenrand) + 2 × 1 (Strich) + 8 (Grid-Abstand) = 252`, der Divisor ist 254. Es passen so viele Karten in die Zeile, dass sie bis zum rechten Rand reichen. Dazu fehlt die ScrollArea, die sonst Platz für die Scrollleiste reserviert. | High | `cleanup.rs:40, 400-401, 409-416` |
| 3 | Bei schmalem Fenster ist `columns` oft 1 oder 2. Die feste Kartenbreite (230) nutzt die Breite schlecht, bei Zwischenbreiten bleibt rechts ein großer Rest und die Karten wirken schief. Die Optionszeilen (`options_ui`, `duration_row`, Ordnerzeilen) sind `ui.horizontal` ohne Umbruch und laufen bei schmalem Fenster über den Rand. | Medium | `cleanup.rs:401, 449-467, 479-558` (`ui.horizontal` statt `horizontal_wrapped`). Das genaue Bild bei halber Breite ist nicht geprüft. |
| 4 | Rückweg nach „Anwenden“ (3): Die Logik ist unauffällig. `on_applied` setzt `self.result`, „Weiter“ setzt `result`/`review` zurück und zeigt wieder `form_ui` (`apply.rs:222-282`, `cleanup.rs:307-318`). Auch „← Anderen Plan wählen“ führt zurück (`cleanup.rs:325-344`). Eine Ursache im Code habe ich nicht gefunden. Möglich wären Hypothese 1 (Knopf „Weiter“ nicht sichtbar) oder ein Zustand, den ich nicht kenne (z. B. fehlgeschlagenes `downcast`, das lautlos nichts setzt: `apply.rs:225`). | Low, unklar | siehe Code; benötigt Reproduktion mit Beschreibung des Bildschirms |

## Affected Code
| File | Line | Description |
|------|------|-------------|
| `src/gui/views/cleanup.rs` | 302-346 | `ui`: keine `ScrollArea`, weder für Formular noch Review/Ergebnis |
| `src/gui/views/cleanup.rs` | 383-441 | `cards_ui`: Spaltenrechnung und feste Kartenbreite, kein Rand rechts |
| `src/gui/views/cleanup.rs` | 443-583 | Optionszeilen mit `ui.horizontal` (kein Umbruch) |
| `src/gui/views/cleanup.rs` | 40-41 | `CARD_WIDTH`, `CARD_HEIGHT` |
| `src/gui/views/apply.rs` | 222-282 | Ergebnisansicht, „Weiter“ |
| `src/gui/app.rs` | 324 | `CentralPanel::default_margins()` |

## Notes
Lösungsrichtung für `/debug:fix-plan`:
- `form_ui` (und die Ergebnisansicht) in `ScrollArea::vertical().auto_shrink([false, false])` setzen; so verschwindet das Abschneiden und die Scrollleiste reserviert den Platz rechts.
- Spaltenzahl aus der echten Kartenbreite rechnen (inkl. Innenrand, Strich, Abstand) und die Karten auf die Zeilenbreite aufteilen (`CARD_MIN_WIDTH` bis `CARD_MAX_WIDTH`), statt feste 230.
- Optionszeilen auf `horizontal_wrapped` umstellen.
- Vor dem Fix Punkt 3 reproduzieren; ist er nach der ScrollArea weg, war es Hypothese 1. Bleibt er, braucht es neue Daten (Bildschirmbeschreibung, Fenstergröße, ob „Weiter“ sichtbar ist).
- Regressionstest: reine Funktion für die Spaltenzahl und die Kartenbreite (Eingabe: verfügbare Breite) mit Tests für schmale, mittlere und breite Fenster; die Zeichnung selbst ist nur per Handtest prüfbar (`manual-tests.md` 5c, letzter Punkt „Fenster schmal ziehen“).
