---
title: "Implementation Plan: GUI-Hilfe (kontextbezogene Erklärungen)"
feature_spec: docs/features/gui-hilfe.md
status: implemented   # pending-approval | approved | implemented
created: 2026-10-08
updated: 2026-10-08
---

# Implementation Plan: GUI-Hilfe

## Summary
Ein neues, fensterfreies Modul `src/gui/help.rs` hält alle Kurztexte als statische Daten (Thema → Titel, „Was es tut“, „Was sich ändert“, „Rückgängig“, Optionen, Verweis auf einen Abschnitt der mitgelieferten Doku). Das bestehende Kürzel-Fenster (`help_window` in `app.rs`) wird zu einem Hilfefenster mit zwei Reitern („Erklärung“, „Tastenkürzel“) ausgebaut und von einem kleinen „?“-Knopf (neues Widget) sowie `F1` geöffnet. Der Knopf sitzt neben der Überschrift jeder Seite und im Kopf jeder der sechs Aufräum-Karten. „Ausführliche Doku öffnen“ öffnet `docs\dokumentation.md` bzw. `README.md` neben der `exe` über `platform::shell::open`; fehlt die Datei oder gibt es kein Programm für `.md`, erscheint eine Meldung mit dem Pfad (bei fehlender Zuordnung mit Rückfall auf Notepad). Die Doku wird im CI-Artefakt mitgeliefert. Tests prüfen, dass jedes Thema einen Text hat, dass die Texte frei von CLI-Begriffen sind und dass jeder Doku-Verweis auf eine existierende Überschrift zeigt.

**Abweichung von der Spec:** Die Spec nennt „an der passenden Stelle“. Ein Standardprogramm für `.md` springt nicht zu Überschriften. Der Link öffnet deshalb die Datei am Anfang, und der Kurztext nennt den Abschnitt namentlich („Abschnitt 7: Die Aufräumaktionen im Einzelnen“). Siehe Open Questions.

## Files to Create
| File | Purpose |
|------|---------|
| `src/gui/help.rs` | Reine Daten und Logik: `Topic` (Seite, Karte, Kürzel), `HelpEntry`, `DocRef`, `entry(topic)`, `topic_for_page(page)`, `doc_path(DocFile)`, Pfadsuche. Keine egui-Abhängigkeit außer in Tests nicht nötig |
| `src/gui/widgets/help_button.rs` | `help_button(ui, topic) -> bool`: kleiner „?“-Knopf mit Tooltip „Hilfe zu …  (F1)“ und Barrierefreiheits-Label |
| `src/gui/views/help_window.rs` | Hilfefenster (Reiter Erklärung/Tastenkürzel, Doku-Link, Meldung bei fehlender Datei); nimmt den Platz von `help_window` in `app.rs` ein |
| `tests/gui_help.rs` | Rauchtests mit `egui_kittest`: „?“ auf der Karte öffnet das richtige Thema, ändert die Kartenauswahl nicht; `F1`; Esc-Reihenfolge mit Dialog |

## Files to Modify
| File | Change Description |
|------|--------------------|
| `src/gui/mod.rs` | `pub mod help;` |
| `src/gui/widgets/mod.rs` | `pub mod help_button;` |
| `src/gui/views/mod.rs` | `pub mod help_window;` |
| `src/gui/app.rs` | `help_open: bool` durch `help: HelpState` ersetzen; `help_window` ausbauen bzw. an `views::help_window` übergeben; `F1` in `handle_keys` (nicht an `text_focus` gebunden, aber an `dialog_open`); `?` und der Knopf „? Kürzel“ öffnen den Reiter Tastenkürzel; Knopf „? Hilfe“ in der Seitenleiste öffnet das Thema der aktuellen Seite; Esc schließt zuerst die Hilfe und wird verbraucht (`consume_key`), damit der Dialog darunter ihn nicht auch sieht |
| `src/gui/keys.rs` | `HELP`-Liste um `F1` („Hilfe zur Seite“) ergänzen (Array-Größe 11 → 12); `action()` bleibt unverändert, weil `F1` außerhalb davon behandelt wird |
| `src/gui/views/overview.rs` (Z. 130), `analysis.rs` (Z. 112), `cleanup.rs` (Z. 303), `history.rs` (Z. 135) | Überschrift in `ui.horizontal` mit `help_button(ui, Topic::Page(..))` |
| `src/gui/app.rs` (Einstellungen, `appearance_ui`) | „?“ am Einstellungs-Kopf (die Seite hat keine eigene `heading`) |
| `src/gui/views/cleanup.rs` (`cards_ui`, ca. Z. 383-441) | „?“ im Kartenkopf; Kartenklick darf dadurch nicht ausgelöst werden (siehe Schritt 6); Rückgabe der Hilfeanfrage an `GuiApp` über `Shell` (`shell.help_request: Option<Topic>`) |
| `src/gui/shell.rs` | Feld `help_request: Option<Topic>`; `Shell::open_help(topic)` |
| `src/platform/shell.rs` | `open_text(path)`: `open`, bei „keine Zuordnung“ Rückfall auf `notepad.exe <Datei>` (ohne die Datei anzulegen, anders als `edit`) |
| `.github/workflows/ci.yml` | Artefakt zusammenstellen: Schritt `Stage` kopiert die drei `exe` nach `dist\`, `docs\dokumentation.md` nach `dist\docs\`, `README.md` nach `dist\README.md` (die Relativlinks `../README.md` bleiben gültig); `upload-artifact` nimmt `dist\` |
| `README.md` (Abschnitt „Grafische Oberfläche“, Z. 296 ff.) | „?“ und `F1`, Hinweis auf den Ordner `docs\` neben den `exe` |
| `docs/dokumentation.md` | Abschnitt 1 „Einrichtung“ bzw. 10 „Dateien und Speicherorte“: Ordner `docs\` neben der `exe`; ggf. kurzer Abschnitt „Grafische Oberfläche“, falls die Kurztexte darauf verweisen (siehe Schritt 3) |
| `docs/manual-tests.md` | Neuer Abschnitt 5e „GUI-Hilfe“ mit Handtests |
| `docs/roadmap.md` | Zeile „Phase 6d: GUI-Hilfe“ |
| `docs/features/gui-hilfe.md` | `status: approved` (laut Nutzer freigegeben), nach Umsetzung `implemented` |

## Data Model Changes
Keine persistenten. Neue Typen in `help.rs`:

```rust
pub enum Topic { Page(Page), Card(&'static str /* CARDS[..].key */), Keys }
pub enum DocFile { Dokumentation, Readme }
pub struct DocRef { pub file: DocFile, pub heading: &'static str /* exakter Überschriftentext */ }
pub struct HelpEntry {
    pub title: &'static str,
    pub what: &'static str,     // Was die Funktion tut
    pub changes: &'static str,  // Was sich dabei ändert (oder „nichts“)
    pub undo: &'static str,     // Wie man es rückgängig macht
    pub options: &'static [(&'static str, &'static str)], // Option → Erklärung
    pub doc: Option<DocRef>,
}
```

`HelpState { open: bool, topic: Topic, tab: HelpTab, notice: Option<String> }` in `app.rs`. Einbindung der Texte als Rust-Konstanten (`static ENTRIES`), nicht per `include_str!`: Das ist typsicher, der Test über Themen ist erschöpfend (ein `match` ohne `_`-Zweig erzwingt einen Eintrag je neuer Seite), und es gibt keine Datei, die fehlen könnte. Karten werden über ihren `key` gefunden; ein Test sichert, dass jede `CARDS`-Karte einen Eintrag hat.

## API / Interface Changes
- `help::entry(Topic) -> &'static HelpEntry` (für `Topic::Card` mit unbekanntem Schlüssel: Rückfall auf den Eintrag der Seite „Aufräumen“, kein Panic).
- `help::topic_for_page(Page) -> Topic`.
- `help::doc_path(DocFile) -> Option<PathBuf>`: sucht `docs\dokumentation.md` bzw. `README.md` in dieser Reihenfolge: (1) neben der `exe` (`exe_dir\docs\…`, `exe_dir\README.md`), (2) für Entwicklerläufe `target\{debug,release}` → Repo-Wurzel (zwei Ebenen höher). Die Namen kommen ausschließlich aus dem Enum, nie aus Nutzereingaben.
- `platform::shell::open_text(&Path) -> Result<()>`.
- `widgets::help_button::help_button(ui, topic) -> bool`.
- Tastenkürzel: `F1` öffnet die Hilfe zur aktuellen Seite (auch bei Fokus in einem Textfeld), nicht bei offenem Dialog; `?` bleibt für die Kürzel-Liste.

## Implementation Sequence
1. **Themen und Texte als Daten (TDD)** — `help.rs` mit `Topic`, `HelpEntry`, `entry`, den Texten für 5 Seiten + 6 Karten + Kürzel; zuerst die Tests (Schritt „Test Strategy“), dann die Texte. Kurztexte in einfachem Deutsch, jeweils höchstens ca. 600 Zeichen je Feld, ohne Befehle wie `plan junk`/`--allow-large`.
   - Files: `src/gui/help.rs`, `src/gui/mod.rs`
   - Depends on: nichts
2. **Doku-Pfad und Öffnen** — `help::doc_path`, `platform::shell::open_text` (Rückfall auf Notepad bei `SE_ERR_NOASSOC`); Meldungstext mit erwartetem Pfad. Tests mit Wegwerf-Ordner (Pfadsuche) und für die Meldung.
   - Files: `src/gui/help.rs`, `src/platform/shell.rs`
   - Depends on: Schritt 1
3. **Doku-Verweise prüfen und ergänzen** — Pro Eintrag `DocRef` auf eine real existierende Überschrift setzen (Seiten: Übersicht → `README.md` „Grafische Oberfläche (Phase 6b)“, Analyse → Abschnitt 5, Aufräumen → Abschnitt 6, Karten → Abschnitt 7, Verlauf → Abschnitt 6, Einstellungen → Abschnitt 9). Fehlt für „Laufender Betrieb“ (Profile, Zeitpläne) ein Abschnitt in `dokumentation.md`, verweist der Eintrag auf `README.md` („Laufender Betrieb …“). Test liest beide Dateien und prüft die Überschriften (zeilengenau).
   - Files: `src/gui/help.rs`, ggf. `docs/dokumentation.md`
   - Depends on: Schritt 1
4. **Widget `help_button`** — kleiner Knopf „?“, `on_hover_text`, `widget_info` mit Label „Hilfe zu <Titel>“; Farben nur über Theme-Tokens (`tests/gui_theme_static.rs` bleibt grün).
   - Files: `src/gui/widgets/help_button.rs`, `src/gui/widgets/mod.rs`
   - Depends on: Schritt 1
5. **Hilfefenster** — `views/help_window.rs`: Reiter „Erklärung“ (Titel, Abschnitte „Was es tut“, „Was sich ändert“, „Rückgängig“, Optionsliste, Knopf „Ausführliche Doku öffnen“ mit Abschnittsname, Meldung bei Fehler) und „Tastenkürzel“ (bisherige Liste). Inhalt in `ScrollArea`, Fenster verschiebbar und in der Breite begrenzt, damit es bei schmalem Fenster umbricht. `app.rs` ersetzt `help_open` durch `HelpState`, `?` und „? Kürzel“ öffnen den Reiter Kürzel, Seitenleistenknopf „? Hilfe“ und `F1` den Reiter Erklärung.
   - Files: `src/gui/views/help_window.rs`, `src/gui/views/mod.rs`, `src/gui/app.rs`, `src/gui/keys.rs`
   - Depends on: Schritte 1, 2, 4
6. **„?“ auf den Seiten und in den Karten** — Überschriften von Übersicht, Analyse, Aufräumen, Verlauf, Einstellungen. In `cards_ui` wird der Kartenklick bisher über `frame.show(..).response.interact(Sense::click())` erkannt; ein „?“ innerhalb würde vom späteren, darüberliegenden Klick-Widget verdeckt. Vorgehen: Kartenfläche zuerst per `ui.interact(rect, id, Sense::click())` anlegen, danach das „?“ mit `ui.put(rect_oben_rechts, help_button)` darüber zeichnen; ein Klick auf „?“ setzt `shell.help_request` und wählt die Karte nicht.
   - Files: `src/gui/views/overview.rs`, `analysis.rs`, `cleanup.rs`, `history.rs`, `src/gui/app.rs`, `src/gui/shell.rs`
   - Depends on: Schritte 4, 5
7. **Esc-Reihenfolge** — `help_window` verbraucht `Esc` (`ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))`), wenn es offen ist; `dialogs::show` läuft danach und sieht das Ereignis nicht mehr (`key_pressed` zählt Ereignisse). Test in `tests/gui_help.rs`: Dialog und Hilfe offen, `Esc` → Hilfe zu, Dialog offen; zweites `Esc` → Dialog (abgelehnt).
   - Files: `src/gui/app.rs`, `src/gui/views/help_window.rs`, `tests/gui_help.rs`
   - Depends on: Schritt 5
8. **Auslieferung der Doku** — CI-Schritt `Stage` und `upload-artifact` auf `dist\`; README/Dokumentation: Ordnerlayout beschreiben (`ordner-cleanup.exe`, `-bg.exe`, `-gui.exe`, `README.md`, `docs\dokumentation.md`). Lokal prüfen mit `cargo build --release` und Kopieren in einen Wegwerf-Ordner.
   - Files: `.github/workflows/ci.yml`, `README.md`, `docs/dokumentation.md`
   - Depends on: Schritt 3
9. **Abschluss** — Handtests (`manual-tests.md` 5e), Roadmap, README-Abschnitt, `verification-before-completion` (`cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`), Spec und Plan auf `status: implemented`.
   - Files: `docs/manual-tests.md`, `docs/roadmap.md`, Spec, Plan
   - Depends on: Schritte 1–8

## Test Strategy
- **Unit (`help.rs`, fensterfrei):**
  - Jede Seite (`Page::ALL`) und jede Karte (`CARDS`) hat einen `HelpEntry`; unbekannter Kartenschlüssel liefert den Rückfall ohne Panic.
  - Textlänge: jedes Feld nicht leer und höchstens 700 Zeichen (Lesbarkeit).
  - Keine CLI-Begriffe in den Texten: Verbotsliste (`--`, `cargo`, `plan `, `.exe`, `json` als Befehlsbestandteil). Die Liste ist bewusst klein; Ausnahmen müssen im Test begründet werden.
  - Jeder `DocRef` zeigt auf eine Datei, die im Repo existiert (`env!("CARGO_MANIFEST_DIR")`), und auf eine Überschrift, die dort zeilengenau steht.
  - `doc_path`: Datei neben der `exe` gefunden; Entwicklerpfad gefunden; nichts gefunden → `None`. Kein Pfad aus Nutzereingaben möglich (Funktion nimmt nur das Enum).
  - `keys::HELP` enthält `F1`.
- **Integration (`tests/gui_help.rs`, `egui_kittest`, ohne GPU):**
  - Klick auf „?“ einer Karte öffnet das Fenster mit dem Titel dieser Karte und ändert die gewählte Karte nicht.
  - Klick auf die Karte (nicht auf „?“) wählt sie weiterhin.
  - `F1` auf jeder Seite öffnet das Thema der Seite, auch mit Fokus in einem Textfeld; bei offenem Dialog nicht.
  - Hilfe offen, Task läuft: `shell.runner` bleibt unberührt (Hilfe blockiert nichts).
  - Esc-Reihenfolge mit Dialog (siehe Schritt 7).
  - Fehlende Doku-Datei: Klick auf „Ausführliche Doku öffnen“ zeigt die Meldung mit dem erwarteten Pfad (Öffnen über eine injizierte Funktion/Trait, damit der Test nichts startet).
- **Statisch:** `tests/gui_theme_static.rs` (keine festen Farben) und der CI-Schritt „Keine Netzwerk-Crates“ bleiben grün; keine neue Abhängigkeit.
- **E2E / von Hand (`manual-tests.md` 5e):**
  - „?“ auf jeder Seite und jeder Karte; Text verständlich ohne CLI-Kenntnis (mit einer Testperson aus dem Nutzertest gegenlesen).
  - „Ausführliche Doku öffnen“ mit und ohne Zuordnung für `.md`, und mit gelöschtem `docs\`-Ordner.
  - DPI 100/150/200 %, hell/dunkel, schmales Fenster: Fenster liest sich, scrollt, „Schließen“ erreichbar.
  - `netstat -ano` während Hilfe benutzt wird: keine Verbindung.

## Risks & Open Questions
- **Risiko: Kartenklick vs. „?“.** Überlappende Klick-Widgets in egui. Mitigation: Schritt 6 legt die Kartenfläche zuerst an und setzt das „?“ darüber; der Rauchtest sichert das Verhalten. Fallback, falls es hakt: „?“ in eine eigene Zeile über der Karte.
- **Risiko: Texte veralten.** Die Kurztexte sind von Hand geschrieben. Mitigation: Der Test erzwingt nur das Vorhandensein, nicht die Aktualität; in der Checkliste für neue Features (`CLAUDE.md`/Spec-Vorlage) „Hilfetext“ ergänzen, wenn gewünscht (nicht Teil dieses Plans).
- **Risiko: Esc-Verbrauch.** `consume_key` nimmt allen anderen Handlern in diesem Frame das Ereignis. Mitigation: nur bei offener Hilfe, Test in Schritt 7.
- **Risiko: `.md` hat kein Standardprogramm.** Rückfall auf Notepad (Schritt 2); eine `.html`-Auslieferung wäre die Alternative, erfordert aber einen Markdown-Renderer (neue Abhängigkeit) und ist hier verworfen.
- **Risiko: Doku fehlt neben der `exe`** (nur die `exe` kopiert). Mitigation: Meldung mit Pfad; die Kurztexte funktionieren immer.
- **Nicht enthalten, aber naheliegend:** „?“ in der Review-Ansicht (Sicherheitshinweis, „Anwenden“), an einzelnen Optionen und an den Analyse-Reitern. Die Spec verlangt es nicht (Open Question 1); das Datenmodell (`Topic`) lässt sich später erweitern.
- [ ] **Abschnitts-Sprung:** Reicht „Datei öffnen, Abschnitt namentlich nennen“? Springen an die Stelle wäre nur mit HTML-Auslieferung (Markdown-Renderer) möglich. Vorschlag: so lassen.
- [ ] **Reiter im Fenster:** Sind „Erklärung“ und „Tastenkürzel“ in einem Fenster wie geplant in Ordnung, oder soll die Kürzel-Liste ein eigenes Fenster bleiben? Vorschlag: ein Fenster.
- [ ] **Zusätzliche Themen:** Soll die Review-Ansicht (Anwenden/Quarantäne) schon in dieser Runde ein eigenes Thema bekommen, weil sie für Personen ohne CLI-Kenntnisse die wichtigste Stelle ist? Vorschlag: ja, als ein zusätzlicher `Topic::Review` mit F1 in der Review-Ansicht; Aufwand gering (ein Eintrag, ein „?“ in der Kopfzeile).

## Umsetzungsstand

Schritte 1 bis 9 sind im Code umgesetzt. `cargo test`, `cargo clippy --all-targets -- -D warnings` und `cargo fmt --check` sind grün. **Offen:** die Handtests in `docs/manual-tests.md`, Abschnitt 5e (Optik, `F1` im echten Fenster, Notepad-Rückfall, DPI, `netstat`).

Abweichungen vom Plan:

- **Review-Thema:** `Topic::Review` mit „?“ neben der Plan-Überschrift und `F1` in der Review-Ansicht ist enthalten (Open Question 3, Vorschlag „ja“).
- **„?“ in den Karten:** Statt `ui.put` (verschiebt den Zellzeiger des Grids) zeichnet `help_button_at` den Knopf über `ui.interact` und den Painter; er liegt über der Kartenfläche und belegt keinen Platz.
- **F1 und Dialog:** `F1` im Hauptfenster ist nicht automatisiert getestet (`GuiApp` braucht einen `CreationContext`); das Verhalten steht in den Handtests 5e. Getestet sind die Hilfe-Anfrage, „?“ an Karte und Seite, der Doku-Link und die Esc-Reihenfolge.
- **Nebenbefund behoben:** Der bestehende Test `duplikat_karte_zeigt_die_zahl_aus_dem_scan_ohne_neue_analyse` scheiterte schon vor dieser Arbeit: Der Karteninhalt erbte das Links-nach-rechts-Layout der Grid-Zelle, die Zahlzeile stand neben dem Text und bekam keine Breite (Karten bis 526 px hoch, zweite Reihe unten abgeschnitten). Der Inhalt liegt jetzt in `ui.vertical` (`cleanup.rs`).
- **Kartentext „Archivieren“:** Der Kartentext in `cleanup_cards.rs` sagt „als ZIP ablegen“, die Doku und die Hilfe sagen „nach `_Archiv` verschieben“ (so arbeitet `plan archive`). Der Kartentext wurde nicht geändert; bitte prüfen.
