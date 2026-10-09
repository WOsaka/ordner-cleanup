---
title: "Quarantäne-Ansicht (Inhalt, Größe, Ablauf, einzelne Dateien zurückholen)"
status: draft          # draft | approved | implemented
created: 2026-10-09
updated: 2026-10-09
---

# Feature: Quarantäne-Ansicht

Roadmap 7.4 („Laufender Betrieb“): Quarantäne-Ansicht. Als kleiner Sofortgewinn in 7.8 genannt.

## Problem Statement

Jede schreibende Aktion verschiebt Dateien in `<wurzel>\.ordner-cleanup\quarantine\<run-id>\…`. Die GUI zeigt die Quarantäne heute nur indirekt: Die Seite „Verlauf“ hat eine Zeile pro Lauf mit Größe und „Läuft ab“, „Rückgängig …“ für den ganzen Lauf und „Quarantäne leeren“. Welche Dateien in der Quarantäne liegen, sieht man nur im Explorer in einem versteckten Tool-Ordner. Eine einzelne, versehentlich verschobene Datei kann man nur zurückholen, indem man den ganzen Lauf rückgängig macht oder sie von Hand zurückkopiert. Beim Zurückkopieren bleibt das Journal unwissend.

Das betrifft alle GUI-Nutzer. Besonders betroffen sind Familie und Kollegen ohne CLI-Kenntnisse, für die „Ich finde meine Datei nicht mehr“ der wichtigste Vertrauensbruch ist.

## Goals

- [ ] Eine eigene GUI-Seite „Quarantäne“ zeigt alle Dateien und Ordner, die noch in der Quarantäne liegen. Pro Eintrag sieht man Name, ursprünglichen Pfad, Größe, Lauf, Datum und Ablauf.
- [ ] Eine versehentlich verschobene Datei ist in unter 30 Sekunden gefunden, mit Suche und Sortierung.
- [ ] Einzelne oder mehrere Einträge lassen sich mit zwei Klicks (Auswahl, „Zurückholen …“, Bestätigen) an ihren ursprünglichen Ort zurückholen.
- [ ] Das Zurückholen ist journaliert. Ein späteres „Rückgängig“ des ganzen Laufs und `purge` behandeln die zurückgeholten Einträge korrekt.

## Out of Scope

- **CLI:** Es gibt keinen neuen Befehl wie `quarantine list` oder `restore`. Die Funktion gibt es nur in der GUI. Die `ops`-Funktionen sind aber so geschnitten, dass ein CLI-Befehl später nur eine dünne Hülle wäre.
- **Endgültiges Löschen einzelner Einträge:** Es bleibt bei `purge` pro Lauf und Aufbewahrungsfrist. Die Ansicht bietet kein „Löschen“.
- **Dateien öffnen oder bearbeiten:** Das geht in der Ansicht nicht, es gibt nur „Im Explorer zeigen“.
- **Freies Ziel beim Zurückholen:** Ziel ist immer der ursprüngliche Pfad, bei Kollision mit Nummern-Suffix. Ziele wie „Auf den Desktop“ gibt es nicht.
- **Aufbewahrungsfrist:** Die Logik der Frist (`quarantine_days`, `purge`) ändert sich nicht.
- **Archiv-Moves:** Einträge mit `dest: archive` oder `dest: rules` liegen nicht in der Quarantäne und erscheinen nicht.

## User Journey

1. Der Nutzer öffnet in der Navigation die neue Seite „Quarantäne“. Sie steht direkt neben „Verlauf“.
2. Die Seite lädt im Hintergrund aus den Journalen aller bekannten Wurzeln. Bis dahin steht dort „Wird geladen …“.
3. Oben stehen die Wurzel-Auswahl („Alle“ oder eine Wurzel) und ein Suchfeld. Das Suchfeld filtert auf Name und ursprünglichen Pfad, ohne Groß- und Kleinschreibung.
4. Darunter folgt eine Kopfzeile mit Summen, zum Beispiel „1 284 Einträge, 3,2 GB, nächster Ablauf in 4 Tagen“.
5. Eine Tabelle zeigt die Spalten Auswahl, Name, Ursprünglicher Ort (gekürzt, voller Pfad im Tooltip), Größe, Aktion (Duplikat, Müll, …), Lauf/Datum, Läuft ab und Status. Sortieren lässt sich nach Name, Größe, Datum und Ablauf. Standard ist „zuletzt verschoben zuerst“.
6. Ein Rechtsklick oder ein Knopf in der Zeile bietet „Im Explorer zeigen“. Das öffnet die Datei in der Quarantäne.
7. Der Nutzer wählt eine oder mehrere Zeilen und klickt „Zurückholen …“.
8. Ein Bestätigungsdialog nennt die Anzahl, die Gesamtgröße und die Ziele. Bei mehr als 15 Zielen zeigt er die ersten 15 und „… und N weitere“. Einträge, deren Ursprungspfad belegt ist, werden mit dem neuen Namen angezeigt, zum Beispiel „→ Bericht (2).pdf“. Weicht ein Hash ab, steht dort ein Warnhinweis.
9. Nach der Bestätigung läuft das Zurückholen als Schreib-Task im Hintergrund und lässt sich abbrechen.
10. Danach erscheint eine Meldung, zum Beispiel „12 zurückgeholt, 1 umbenannt, 1 fehlt“. Die Liste lädt neu, und die Verlaufsansicht zeigt die verringerte Größe des Laufs.
11. F1 oder das Hilfe-Symbol neben der Überschrift zeigt einen Kurztext zur Seite, wie in Phase 6d.

## Acceptance Criteria

- [ ] Given ein Lauf mit Quarantäne-Moves, when die Seite geöffnet wird, then erscheint jeder ausgeführte, nicht zurückgedrehte Quarantäne-Move als eine Zeile. Ein verschobener Ordner (`is_dir`) erscheint als eine Zeile mit Dateianzahl.
- [ ] Given Läufe mit `dest: archive`, `dest: rules` oder entfernten leeren Ordnern, then erscheinen diese Aktionen nicht.
- [ ] Given ein per `purge` geleerter oder vollständig zurückgedrehter Lauf, then erscheinen seine Einträge nicht.
- [ ] Given ein Eintrag, dessen Datei in der Quarantäne fehlt (von Hand gelöscht), then steht er mit Status „fehlt“ in der Liste und kann nicht ausgewählt werden.
- [ ] Given eine Auswahl, when „Zurückholen …“ bestätigt wird, then liegt jede Datei wieder unter ihrem ursprünglichen Pfad. Fehlende Elternordner werden angelegt.
- [ ] Given ein belegter Ursprungspfad, when zurückgeholt wird, then wird nichts überschrieben. Die Datei landet als `name (2).ext` usw. im selben Ordner, und der Dialog hat den Namen vorher angezeigt.
- [ ] Given ein Journal-Eintrag mit Hash (`dedupe`), when die Datei in der Quarantäne einen anderen Hash hat, then warnt der Dialog. Das Zurückholen bleibt möglich.
- [ ] Given ein zurückgeholter Eintrag, when danach der ganze Lauf rückgängig gemacht wird, then wird der Eintrag übersprungen („bereits zurück“) und nicht als Konflikt gemeldet.
- [ ] Given ein zurückgeholter Eintrag, when der Lauf später per `purge` geleert wird, then ist die zurückgeholte Datei davon unberührt.
- [ ] Given ein Lauf, aus dem einzelne Einträge zurückgeholt wurden, then zeigt die Verlaufsansicht die verringerte Größe. Der Status bleibt „ausgeführt“ und springt nicht auf „zurückgedreht“, solange noch Einträge in der Quarantäne liegen.
- [ ] Given ein Lauf, aus dem alle Quarantäne-Einträge einzeln zurückgeholt wurden, then bleibt er in der Verlaufsansicht sichtbar, mit Größe 0. Das Verhalten von „Rückgängig …“ für die übrigen Aktionen (Archiv, leere Ordner) ist unverändert.
- [ ] Given das Zurückholen wird abgebrochen, then sind bereits zurückgeholte Einträge journaliert, und die übrigen bleiben in der Quarantäne.
- [ ] Given 100 000 Quarantäne-Einträge, then lädt die Liste im Hintergrund, und Scrollen, Suchen und Sortieren bleiben flüssig (Messung analog zu den bestehenden Perf-Tests).
- [ ] Given ein Cloud-only-Platzhalter in der Quarantäne, then liest oder hasht die Ansicht ihn nicht. Nur das Zurückholen bewegt ihn per Rename, ohne Download.
- [ ] Alle bestehenden Tests bleiben grün, inklusive `tests/roundtrip_random.rs`. Neue Tests decken Zurückholen, Kollision, späteres Undo und `purge` ab.

## Edge Cases & Error States

| Scenario | Expected Behavior |
|----------|-------------------|
| Ursprungspfad belegt | Nie überschreiben; Ziel `name (2).ext` usw. wie `quarantine::unique_target`. Der neue Name steht im Dialog und im Journal |
| Ursprünglicher Ordner existiert nicht mehr | Elternordner werden angelegt (wie beim Undo) |
| Ursprünglicher Elternordner ist inzwischen eine Datei oder ein Link | Eintrag wird nicht zurückgeholt, Status „Fehler: Ziel ist kein Ordner“ in der Ergebnis-Meldung |
| Datei in der Quarantäne fehlt | Status „fehlt“ in der Liste, nicht auswählbar |
| Hash weicht ab (nur bei Einträgen mit Hash) | Warnung im Dialog, Zurückholen erlaubt |
| Kein Hash im Journal (alle Aktionen außer `dedupe`) | Keine Prüfung, kein Hinweis |
| Verschobener Ordner (`dedupe-dirs`) | Eine Zeile mit Dateianzahl; wird als Ganzes zurückgeholt. Bei belegtem Ursprungsordner: `Ordner (2)` |
| Lauf ist teilweise zurückgedreht | Nur die noch nicht zurückgedrehten Einträge erscheinen |
| Lauf ohne `run_end` (abgebrochener Apply) | Ausgeführte Einträge (`done`) erscheinen normal |
| Journal unlesbar | Lauf erscheint nicht in der Liste; Hinweis in der Kopfzeile „N Läufe nicht lesbar“, Details im Verlauf |
| Journal verweist außerhalb von Wurzel oder Quarantäne | Eintrag wird nicht angezeigt bzw. nicht zurückgeholt (gleiche Prüfungen wie `Restore::move_back`) |
| Tool-Ordner oder Lauf-Ordner ist ein Link | Zurückholen abgelehnt (wie `ensure_plain_dirs`) |
| Datei gesperrt (z. B. geöffnet) | Eintrag scheitert mit Fehlermeldung, die übrigen laufen weiter |
| Gleichzeitig läuft Apply, Undo oder Purge | Die GUI serialisiert Schreib-Tasks; „Zurückholen …“ ist gesperrt, solange ein Schreib-Task läuft |
| Abbruch während des Zurückholens | Bereits zurückgeholte Einträge bleiben zurück und sind journaliert |
| Wurzel nicht erreichbar (Laufwerk fehlt) | Wurzel erscheint mit Hinweis „nicht erreichbar“, ihre Einträge fehlen |
| Keine Einträge | Leerer Zustand: „Die Quarantäne ist leer.“ |
| Sehr lange Pfade | Wie überall mit `\\?\`; Anzeige gekürzt, voller Pfad im Tooltip |

## Technical Constraints

- **Quelle der Liste:** Die Liste kommt aus den Journalen (`collect_ops` in `change/undo.rs`) und nicht aus einem Verzeichnisscan der Quarantäne. Das Journal liefert ursprünglichen Pfad, Quarantäne-Pfad, Größe, Hash und Aktion. Für den Status „fehlt“ reicht ein `exists` je Eintrag über `FsOps`, ohne die Datei zu öffnen.
- **Journal beim Zurückholen:** Für jeden zurückgeholten Eintrag kommt ein Eintrag ins Journal des ursprünglichen Laufs. Ohne Kollision genügt `UndoDone`. Mit Kollision braucht es einen Eintrag mit dem tatsächlichen Ziel, damit Verlauf-Details und ein späteres Undo wissen, wo die Datei liegt. Ob das ein neues Feld an `UndoDone` oder ein neuer Eintragstyp wird, entscheidet der Implementierungsplan (siehe Open Questions). Ältere Journale müssen lesbar bleiben.
- **Laufstatus:** Einzelnes Zurückholen schreibt kein `UndoStart`/`UndoEnd`. So bleibt der Laufstatus („ausgeführt“) unverändert, und nur die Größe in der Quarantäne sinkt. `summarize` berücksichtigt das schon über `undone`.
- **Wiederverwendung:** Die Prüfungen aus `Restore::move_back` (Pfad unter Wurzel bzw. Lauf-Quarantäne, kein `..`, nie überschreiben, leere Elternordner in der Quarantäne aufräumen) werden genutzt oder herausgelöst, nicht dupliziert.
- **Performance:** Bei 100 000 Einträgen läuft das Laden im Hintergrund-Task. Die Tabelle rendert virtualisiert (nur sichtbare Zeilen). Suche und Sortierung laufen im Speicher auf vorbereiteten Schlüsseln. Hash-Prüfung nur für die ausgewählten Einträge und erst vor dem Bestätigungsdialog, als eigener Lese-Task.
- **OneDrive:** Platzhalter (`RECALL_ON_DATA_ACCESS`/`OFFLINE`) werden nie gelesen oder gehasht. Bei ihnen entfällt die Hash-Prüfung mit dem Hinweis „nicht geprüft (nur online)“.
- **Security:** Es gibt keine Netzwerkzugriffe und keine neuen Rechte. Zurückgeholt wird nur aus `<wurzel>\.ordner-cleanup\quarantine\<run-id>\` an Pfade unter derselben Wurzel.
- **Compliance:** Keine Anforderungen.

## Dependencies

- `src/change/undo.rs`: Zurückholen einzelner Aktionen (neue Funktion neben `undo_run`), gemeinsame Prüfungen aus `Restore`.
- `src/change/journal.rs`: ggf. neues Feld oder neuer Eintrag für das tatsächliche Ziel. Abwärtskompatibel lesen.
- `src/change/quarantine.rs`: `unique_target`, `cleanup_empty_parents`, `ensure_plain_dirs`.
- `src/ops/`: neue Funktionen „Quarantäne-Inhalt auflisten“ und „Einträge zurückholen“, analog zu `ops/runs.rs`.
- GUI:
  - neue Seite `gui/views/quarantine.rs`
  - Eintrag in Navigation und `Route`/`Page`
  - Wiederverwendung von `widgets/table` (`short_path_cell`, `status_cell`), `Dialog::Confirm` und Tasks (`TaskKind::Read`/`Write`)
  - Hilfetext in `gui/help.rs` und `texts.rs`
  - optional ein Link „Inhalt ansehen“ aus der Verlaufsansicht auf die Seite, gefiltert auf den Lauf
- Doku:
  - `docs/dokumentation.md`: GUI-Seite und Journal-Eintrag
  - `docs/manual-tests.md`: neuer Abschnitt
  - `docs/roadmap.md`: Status

## Open Questions

- [ ] Journal-Format für Zurückholen mit Kollision: optionales Feld `to` an `UndoDone` oder neuer Eintrag `Restored { action, to }`? Vorschlag: optionales Feld an `UndoDone`, weil `collect_ops` dann unverändert `undone` setzt.
- [ ] Soll die Verlaufsansicht einen Knopf „Inhalt ansehen“ bekommen, der die Quarantäne-Seite auf den Lauf gefiltert öffnet? Vorschlag: ja, ist klein und hilft beim Finden.
- [ ] Spalte „Aktion“: Woher kommt die Art (Duplikat, Müll, Versionen, …)? Das Journal speichert sie nicht je Eintrag. Vorschlag: aus `run_start` bzw. der Plan-Art des Laufs, falls vorhanden, sonst Spalte weglassen.
