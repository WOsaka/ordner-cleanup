---
title: "GUI-Bedienung: schneller und klarer"
status: implemented          # draft | approved | implemented
created: 2026-10-08
updated: 2026-10-08
---

# Feature: GUI-Bedienung: schneller und klarer

## Problem Statement
Die egui-Oberfläche (Phase 6b, [`gui.md`](gui.md)) ist funktional vollständig, aber im Alltag umständlich und teils unleserlich:

- **Das Ziel ist versteckter Zustand.** `shell.target` gilt für alle Seiten, steht aber nur als kleines „Ziel: …“-Label da. Das Profil-Dropdown zeigt nach der Wahl weiter „Profil …“, und „Wählen“ bei einem Profil ändert sichtbar nichts.
- **Die Übersicht ist halb Formular, halb Dashboard.** Das Scan-Panel steht über drei Tabellen. „Erweitert“ mischt Scan- und Klassifizier-Optionen.
- **Rohe Entwicklerausgaben:** Status als `{:?}` („Complete, 0 Fehler“), unformatierte Zeitstempel, Scan- und Classify-Ergebnis als Monospace-Textblock, Lauf-IDs im Fließtext, Score ohne Einordnung.
- **Das Aufräumen-Formular spricht CLI:** „Älter als (leer = aus der Config, z. B. 2y)“, „Ordner (absolut)“, Regelnamen kommagetrennt. Die sechs Plan-Arten sind nackte Tabs. Die Analyse kennt Zahlen (Duplikate, Müll, leere Ordner), zeigt sie hier aber nicht.
- **Review und Theme:** drei Leisten untereinander, keine Tastaturbedienung, kein Hinweis auf die Quarantäne. Es gilt das egui-Standardaussehen mit 18 fest verdrahteten `Color32`-Stellen, die im hellen Theme schlecht lesbar sind.

Betroffen ist zunächst der Entwickler als Power-User, der das Werkzeug oft nutzt und weniger Klicks und bessere Lesbarkeit will. Ein geführter Ablauf für Laien ist ausdrücklich **nicht** Ziel dieser Spec.

## Goals
Umsetzung in vier voneinander unabhängigen Stufen, jede einzeln baubar, testbar und mergebar, in dieser Reihenfolge.

- [x] **Stufe 1, Kopfleiste:** Eine feste Leiste mit Ordner/Profil-Auswahl, Scannen und Klassifizieren auf allen Seiten außer den Einstellungen. Die Übersicht zeigt nur noch Zustand.
- [x] **Stufe 2, Rohausgaben:** Lesbare Status-Texte, relative Zeiten, Kennzahl-Kacheln, Lauf-Tabelle, gekürzte Pfade, zentral in `gui/format.rs`.
- [x] **Stufe 3, Aufräumen mit Zahlen:** Plan-Arten als Karten mit Analyse-Zahlen, Dauer-Felder als Zahl plus Einheit, Regelnamen als Häkchen.
- [x] **Stufe 4, Review, Tastatur, Theme:** Feste Review-Kopfzeile mit Quarantäne-Hinweis, Auswahl-Menü, Tastaturkürzel, Theme-Tokens für hell und dunkel, einheitliche Abstände, Icons.
- [ ] Die Grundprinzipien und die Funktionsgleichheit mit der CLI bleiben unverändert.
- [ ] Die Logik hinter jeder Darstellung (Formatierung, Auswahl per Tastatur, Karten-Zahlen) ist ohne Fenster testbar.

## Out of Scope
- Geführter Assistent bzw. Wizard für Laien, Schrittleiste
- Neue Analyse- oder Aufräumfunktionen, die die CLI nicht hat (u. a. keine neue Schätzung für Archivieren, Versionen und Regeln)
- Mehrsprachigkeit (nur Deutsch, Texte weiter zentral in `texts.rs`)
- Eigene Fensterdekoration, Animationen, neue Schriften
- Änderungen am Plan-Format, am Journal oder am Config-Schema (die Theme-Wahl liegt im eframe-Storage)
- Ein Kürzel, das direkt anwendet. Schreibende Aktionen laufen weiter nur über den Bestätigungsdialog
- Der Aufbau der Einstellungen-Seiten selbst (nur Theme-Tokens und Abstände wirken dort mit)

## User Journey

### Stufe 1: Kopfleiste
1. Der Nutzer startet die GUI. Über dem Seiteninhalt steht eine feste Leiste, zum Beispiel:
   `[📁 D:\Daten ▾]  Letzter Scan: vor 2 Tagen · Score 72 (▲3)   [Scannen] [Klassifizieren]`
2. Das Dropdown listet die *zuletzt benutzten Ordner*, die *Profile* (eigene Gruppenüberschrift) und „Anderen Ordner wählen …“. Der Kopf zeigt immer die tatsächliche Auswahl, auch bei Profilen.
3. **Scannen** und **Klassifizieren** wirken auf die Auswahl. Der Fortschritt mit Abbrechen erscheint im Kopf. Die Statusleiste unten zeigt weiter Hintergrundaufgaben, wenn eine andere Seite offen ist.
4. Ein Knopf „Optionen …“ öffnet den Dialog **Scan-Optionen** mit zwei getrennten Abschnitten: *Scannen* (Netzlaufwerk, Standard-Ausschlüsse, Index neu aufbauen, Threads, Vorlage, zusätzliche Ausschlüsse, „Nur Summe“) und *Klassifizieren* (ohne LLM, alles neu, Endungen, Glob, Text-Cache löschen).
5. Die **Übersicht** zeigt nur noch Zustand: gescannte Ordner, Profile, letzte Läufe, ausstehende Quarantäne. Ist noch nichts gescannt, steht dort eine große Aufforderung mit dem Knopf „Ordner wählen und scannen“.
6. Die Auswahl wird zwischen den Starts gemerkt (`Persisted`). Auf den Einstellungen ist die Kopfleiste ausgeblendet, weil diese ordnerunabhängig sind.

### Stufe 2: Lesbare Ausgaben
7. **Status** erscheint mit Symbol, Farbe und deutschem Text, zum Beispiel „✔ Vollständig“, „⚠ Teilweise“, „↩ Rückgängig gemacht“, „Scan unvollständig, 3 Fehler“.
8. **Zeiten** sind relativ („vor 2 Tagen“, „heute 14:03“, ab einer Woche „12.09.2026“), der genaue Wert steht im Tooltip.
9. **Scan- und Classify-Ergebnis** erscheinen als Kennzahl-Kacheln *Dateien · Ordner · Größe · Fehler*. Hinweise stehen darunter als Liste mit Symbol, ein Klick auf „Fehler“ öffnet die Details. Der Score trägt eine Einordnung (z. B. „72 – gut“) mit den Schwellen aus `health/score.rs`.
10. **Letzte Läufe** sind eine Tabelle *Wann · Ordner · Art · Ergebnis · Größe · Aktion*. Die Lauf-ID steht im Tooltip bzw. in den Details, und „Rückgängig“ ist direkt erreichbar, wo es möglich ist.
11. **Pfade** werden in der Mitte gekürzt (`D:\…\Projekte\Alt`). Der volle Pfad steht im Tooltip, und ein Rechtsklick kopiert ihn.
12. **Quarantäne-Zeile:** „4,2 GB in 3 Läufen warten auf Löschung“ mit dem Knopf „Quarantäne leeren …“. Bei Profilen wird „Wählen“ zu „Öffnen“ und setzt die Kopfleiste. „Zum Prüfen“ ist ein klickbares Badge, das in „Analyse → Inhalte“ springt.

### Stufe 3: Aufräumen mit Zahlen
13. Die Seite zeigt sechs **Karten** mit Titel, einem Satz Erklärung und, wo vorhanden, der Zahl aus dem letzten Scan:
    - Duplikate: „37 Gruppen, 4,2 GB könnten frei werden“
    - Müll: „1.204 Dateien, 860 MB“
    - Leere Ordner: „58 Ordner“
    - Archivieren, Versionen, Nach Regeln: nur Beschreibung, keine Zahl
14. Ist der Scan älter als 7 Tage, steht an den Zahlen „Stand: vor 12 Tagen, neu scannen?“. Die Schwelle ist eine Konstante ohne Config-Eintrag.
15. Ein Klick auf eine Karte öffnet die Optionen mit den Defaults aus Config bzw. Profil. „Plan erzeugen“ ist der einzige Hauptknopf (Karte → Plan erzeugen genügt für den Standardfall).
16. **Dauer-Felder** („Älter als“, „Mindestalter“) sind Zahl plus Einheit (*Tage / Monate / Jahre*). Der Platzhalter zeigt den Config-Wert („Standard: 2 Jahre“). Intern entsteht weiter `2y` bzw. `30d`, derselbe Parser wie in der CLI. Leer heißt „aus der Config“.
17. **Regeldatei:** Dateiwahl mit gekürztem Pfad. Die Regelnamen erscheinen als Häkchen aus der geladenen Datei statt als kommagetrennte Liste.
18. **Duplikat-Strategie** ist ein Dropdown mit Erklärung („älteste Datei bleibt“). Der Ordner für „unterhalb von Ordner“ wird per Dialog gewählt.
19. **Zuletzt erzeugte Pläne** zeigen Art, Zeit und Größe lesbar. Ein Klick öffnet den Plan.

### Stufe 4: Review, Tastatur, Theme
20. Die **Review-Kopfzeile** bleibt beim Scrollen sichtbar. „Anwenden (480 Einträge, 3,1 GB)“ ist der hervorgehobene Hauptknopf und bleibt ohne Auswahl deaktiviert.
21. Darunter steht der **Sicherheitshinweis:** „Nichts wird gelöscht. Dateien gehen in die Quarantäne und lassen sich 30 Tage lang zurückholen.“ Die Zahl stammt aus `quarantine_days`. Bei Archivieren lautet der Text entsprechend anders.
22. Die fünf Auswahl-Knöpfe werden das Menü **„Auswahl ▾“** (Alle · Keine · Gefilterte wählen/abwählen · Ordner wählen/abwählen). „Übersprungene (12)“ zeigt einen Zähler.
23. **Tastaturkürzel:**

    | Taste | Wirkung |
    |---|---|
    | ↑ / ↓ | Zeile wechseln |
    | Leertaste | Eintrag an- bzw. abwählen, eine Zeile weiter |
    | Strg+A / Strg+Umschalt+A | Gefilterte wählen / abwählen |
    | Strg+F | Fokus ins Textfilter-Feld |
    | Strg+Enter | „Anwenden …“ (öffnet nur den Bestätigungsdialog) |
    | Strg+O | Plan öffnen |
    | Strg+R | Neu laden |
    | Strg+1 bis Strg+5 | Seite wechseln |
    | Esc | Dialog schließen |

    Die Kürzel stehen in Tooltips und in einer Liste hinter einem „?“-Knopf.
24. **Theme:** `gui/theme.rs` definiert Tokens (Erfolg, Warnung, Fehler, gedämpft, Akzent) für hell und dunkel. Die 18 festen `Color32`-Stellen ziehen darauf um. Das Theme folgt dem Windows-Modus. Eine Umschaltung hell/dunkel/System liegt in den Einstellungen und wird im eframe-Storage gemerkt.
25. Eine einheitliche Skala für Überschriften, Tabellenzeilenhöhe und Innenabstände ersetzt die verstreuten `add_space`-Werte. Die Seitenleiste bekommt Icons aus der schon geladenen Segoe-UI-Symbolschrift. Tabellen haben gestreifte Zeilen, feste Zeilenhöhe und rechtsbündige Zahlenspalten.

## Acceptance Criteria

### Stufe 1
- [ ] Given die GUI ist offen, when der Nutzer eine beliebige Seite außer den Einstellungen öffnet, then ist die Kopfleiste mit der aktuellen Auswahl sichtbar
- [ ] Given der Nutzer wählt ein Profil, when das Dropdown geschlossen ist, then zeigt der Kopf den Profilnamen (nicht „Profil …“)
- [ ] Given ein Scan läuft, when der Nutzer die Seite wechselt, then bleibt der Fortschritt mit Abbrechen im Kopf sichtbar
- [ ] Given kein Index existiert, when die Übersicht öffnet, then zeigt sie die Aufforderung „Ordner wählen und scannen“ mit funktionierendem Knopf
- [ ] Given der Dialog „Scan-Optionen“, when der Nutzer ihn öffnet, then sind die Optionen von Scannen und Klassifizieren in getrennten Abschnitten, und alle bisherigen Optionen sind weiter erreichbar
- [ ] Given die GUI wird beendet und neu gestartet, when sie startet, then ist der zuletzt gewählte Ordner bzw. das Profil wieder gewählt

### Stufe 2
- [ ] Given ein Lauf mit Status `Complete`, when die Lauf-Tabelle ihn zeigt, then steht dort der deutsche Statustext und nirgends ein Debug-Wert wie `Complete`
- [ ] Given ein Zeitstempel von vor 2 Tagen, when er angezeigt wird, then steht dort „vor 2 Tagen“ und der genaue Wert im Tooltip
- [ ] Given ein abgeschlossener Scan, when das Ergebnis erscheint, then zeigt es Kennzahl-Kacheln statt eines Textblocks
- [ ] Given ein Pfad länger als die Spaltenbreite, when er angezeigt wird, then ist er in der Mitte gekürzt, und Tooltip bzw. Kopieren liefern den vollständigen Pfad (auch bei langen Pfaden > 260 Zeichen und Umlauten)
- [ ] Given die Formatfunktionen in `format.rs`, when `cargo test` läuft, then decken Unit-Tests Status, relative Zeit, Pfadkürzung und Score-Einordnung ab

### Stufe 3
- [ ] Given ein gescannter Ordner mit Duplikaten, Müll und leeren Ordnern, when die Aufräumen-Seite öffnet, then zeigen die drei Karten die Zahlen aus dem Report (ohne neue Analyse)
- [ ] Given die Karten „Archivieren“, „Versionen“ und „Nach Regeln“, when sie angezeigt werden, then enthalten sie keine Zahl
- [ ] Given ein Scan älter als 7 Tage, when die Karten angezeigt werden, then steht dort der Hinweis „Stand: vor … Tagen, neu scannen?“
- [ ] Given die Eingabe „2“ und „Jahre“, when der Plan erzeugt wird, then entsteht derselbe Plan wie mit `--old-after 2y` in der CLI
- [ ] Given ein leeres Dauer-Feld, when der Plan erzeugt wird, then gilt der Config-Wert, und der Platzhalter zeigt ihn an
- [ ] Given eine gewählte Regeldatei, when die Seite sie lädt, then erscheinen die Regelnamen als Häkchen, und nur angehakte Regeln fließen in den Plan ein
- [ ] Given eine Regeldatei mit Syntaxfehler, when der Nutzer sie wählt, then erscheint der Fehler mit Datei und Zeile, und die GUI stürzt nicht ab

### Stufe 4
- [ ] Given eine Review-Liste mit 100.000 Einträgen, when der Nutzer scrollt, filtert oder sortiert, then bleiben die Zeiten aus `gui.md` (Filter/Sortierung < 300 ms) erhalten
- [ ] Given eine Review mit Auswahl, when der Nutzer scrollt, then bleibt die Kopfzeile mit „Anwenden (N Einträge, X GB)“ sichtbar
- [ ] Given `quarantine_days = 14`, when ein Quarantäne-Plan geprüft wird, then nennt der Hinweis „14 Tage“
- [ ] Given der Fokus in der Liste, when der Nutzer die Leertaste drückt, then wird der Eintrag umgeschaltet und die nächste Zeile fokussiert, und die Logik dazu ist als Unit-Test ohne Fenster abgedeckt
- [ ] Given ein beliebiger Tastendruck oder eine beliebige Kombination, when sie ausgeführt wird, then wird nie ohne Bestätigungsdialog angewendet, rückgängig gemacht oder gelöscht
- [ ] Given Windows steht auf dunkel bzw. hell, when die GUI startet, then folgt das Theme, und Fehler-, Warn- und Erfolgsfarben sind in beiden Modi gut lesbar
- [ ] Given `grep Color32::` über `src/gui` außerhalb von `theme.rs`, when geprüft wird, then gibt es keine festen Farbwerte mehr
- [ ] Given eine Skalierung von 150 % bis 200 %, when die Kopfleiste und die Review-Kopfzeile angezeigt werden, then ist nichts abgeschnitten

### Übergreifend
- [ ] Given jede Stufe, when sie gemergt wird, then bestehen `cargo test`, `cargo clippy -- -D warnings` und `cargo fmt --check`
- [ ] Given die CI-Sperre für HTTP- und TLS-Crates, when neue Abhängigkeiten nötig würden, then werden keine hinzugefügt (alles mit egui, `egui_extras` und `chrono`, die schon da sind)

## Edge Cases & Error States
| Scenario | Expected Behavior |
|----------|-------------------|
| Gewählter Ordner existiert nicht mehr (Laufwerk getrennt, Ordner gelöscht) | Kopf zeigt die Auswahl mit Warnsymbol, Scannen ist deaktiviert, Tooltip erklärt den Grund. Ein Klick auf „Anderen Ordner wählen …“ geht weiter |
| Zuletzt benutzte Ordner enthalten ungültige Einträge | Werden stillschweigend ausgelassen, die Liste ist auf eine feste Länge begrenzt |
| Profil wurde aus der Config entfernt, ist aber gemerkt | Auswahl fällt auf „keine“ zurück, Kopf zeigt „Ordner wählen“, kein Fehlerdialog |
| Config nicht lesbar beim Start | Profil-Gruppe im Dropdown zeigt den Fehler, Ordner-Auswahl funktioniert weiter (wie bisher auf der Übersicht) |
| Scan läuft, Nutzer wechselt Ordner | Auswahl wechselt, laufender Scan gehört weiter zum alten Ziel (Name im Fortschritt), Scannen für das neue Ziel ist bis zum Ende gesperrt (Scan-Sperre) |
| Zweiter Scan, während ein anderer läuft | Bestehende Meldung „Ein anderer Scan läuft“ |
| Kein Report vorhanden (nie gescannt) | Karten ohne Zahlen, Hinweis „Erst scannen“ |
| Report veraltet oder nicht lesbar | Karten ohne Zahlen, Fehlertext statt Absturz, Plan-Erzeugung bleibt möglich |
| Regeldatei groß oder langsam zu parsen | Parsen im Hintergrund-Task, UI bleibt bedienbar, Häkchenliste erscheint nach dem Laden |
| Dauer-Eingabe ist keine Zahl oder zu groß | Feld wird markiert, „Plan erzeugen“ bleibt deaktiviert (kein Überlauf, vgl. Bug `parse-old-after-overflow`) |
| Uhr des PCs geht falsch (Zeitstempel in der Zukunft) | Relative Zeit fällt auf das absolute Datum zurück, kein negatives „vor …“ |
| Status oder Art eines Laufs aus einer neueren Version unbekannt | Neutrale Anzeige „Unbekannt“ mit dem Rohwert im Tooltip |
| Tastenkürzel, während ein Textfeld Fokus hat | Textfeld hat Vorrang (Leertaste tippt ein Leerzeichen, Strg+A markiert Text). Listenkürzel wirken nur mit Fokus in der Liste |
| Strg+Enter ohne Auswahl oder bei laufender Aufgabe | Wirkungslos, wie der deaktivierte Knopf |
| Dunkler Modus wechselt zur Laufzeit | Theme folgt, ohne Neustart |
| Schriftdateien fehlen (`C:\Windows\Fonts`) | Wie bisher egui-Standardschriften, Symbole fallen auf Text zurück |
| Review mit sehr vielen Einträgen und Tastaturnavigation | Fokuszeile bleibt im sichtbaren Bereich, ohne die virtuelle Liste zu bremsen |

## Technical Constraints
- Performance:
  - Die virtuelle Liste und die vorbereiteten Filterindizes aus `gui.md` bleiben unangetastet. Neue Darstellung (Statussymbole, gekürzte Pfade) wird nur für sichtbare Zeilen berechnet.
  - Gekürzte Pfade und relative Zeiten werden nicht pro Frame neu als Strings aufgebaut, wenn sich die Eingabe nicht ändert. Das gilt besonders für die Review-Tabelle.
  - Das Parsen einer Regeldatei läuft als Hintergrund-Task (`TaskKind::Read`).
- Security:
  - Kein neuer Netzwerkzugriff, keine neuen Abhängigkeiten, die CI-Sperre bleibt gültig.
  - Tastaturkürzel lösen nur den Bestätigungsdialog aus, nie direkt eine schreibende Aktion.
  - Pfade im Tooltip und beim Kopieren werden unverändert ausgegeben (kein Kürzen im kopierten Wert).
- Compliance:
  - Keine Daten verlassen den PC. Die Theme-Wahl liegt im lokalen eframe-Storage, nicht in der Config.
- Architektur:
  - `shell.target` bleibt die einzige Quelle der Auswahl. Neue Module: `gui/header.rs`, `gui/format.rs`, `gui/theme.rs`. `ScanPanel` geht in die Kopfleiste und den Optionen-Dialog auf, und die Tasks für Scan und Classify hängen nicht mehr an `Route::Overview`.
  - Texte bleiben zentral in `gui/texts.rs`.
  - Fachlogik bleibt in der Bibliothek. Die GUI formatiert nur.
- Tests:
  - Logiktests ohne Fenster für `format.rs`, die Karten-Zahlen, die Dauer-Umrechnung (Zahl + Einheit → `2y`/`30d`) und die Tastatur-Auswahl.
  - Die vorhandenen Tests (`trend`, `scan_text`, `review_text`, `pending_quarantine`) ziehen mit um und bleiben grün.
  - Die Darstellung (Theme, Abstände, DPI) wird manuell geprüft und in `docs/manual-tests.md` ergänzt.
- Reihenfolge und Zuschnitt:
  - Jede Stufe ist ein eigener Implementierungsschritt mit eigenem Merge. Spätere Stufen dürfen auf früheren aufbauen, aber nicht umgekehrt (Theme-Tokens aus Stufe 4 werden in Stufe 2 für die Statusfarben vorbereitet, falls nötig als kleine Vorarbeit).

## Dependencies
- Service / feature:
  - [`gui.md`](gui.md) (Phase 6b) mit `gui/shell.rs`, `gui/app.rs`, `gui/views/*` und `gui/review.rs`
  - Report-Modell (`report::Report`, `health::*`) für die Karten-Zahlen und die Score-Einordnung (`health/score.rs`)
  - Dauer-Parser (`config::parse_duration_secs`, `analysis::age::parse_old_after`) und Regel-Parser in `rules`
  - `quarantine_days` aus der Config
- Bibliotheken: `eframe`/`egui` 0.36.2 und `egui_extras` (gepinnt), `chrono` (relative Zeiten), keine neuen Crates
- Windows: Modus hell/dunkel über egui (System-Theme), Segoe-UI-Symbolschrift

## Open Questions
- [ ] Kann egui 0.36.2 den Windows-Hell/Dunkel-Modus zur Laufzeit zuverlässig verfolgen, oder braucht „System“ einen eigenen Aufruf über das `windows`-Crate?
- [ ] Ist das Parsen der Regeldatei beim Wählen billig genug für die Häkchenliste, oder braucht sie ein Zwischenergebnis im Cache? (Prüfung im Implementierungsplan)
- [ ] Reicht die Segoe-UI-Symbolschrift für alle gewünschten Icons, oder fehlt ein Symbol auf Windows 10? (Fallback: Text)
- [x] Wie viele „zuletzt benutzte Ordner“? Entschieden: 8, dazu alle gescannten Wurzeln aus dem Index (ohne Dubletten)
- [x] Score-Einordnung („72 – mittel“): drei feste Bänder (≥ 80 gut, 60–79 mittel, < 60 schwach) in `gui/format.rs`, reine Darstellung. `health/score.rs` bleibt unverändert
- [x] Profile im Kopf-Dropdown: bei vielen Einträgen scrollen, kein Suchfeld
