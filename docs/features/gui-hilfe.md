---
title: "GUI-Hilfe: Erklärungen zu jeder Funktion direkt auf der Seite"
status: approved          # draft | approved | implemented
created: 2026-10-08
updated: 2026-10-08
---

# Feature: GUI-Hilfe (kontextbezogene Erklärungen)

## Problem Statement
Die Dokumentation steht in `README.md` und `docs/dokumentation.md`, außerhalb der GUI und für Entwickler geschrieben. Wer in der GUI nicht weiß, was eine Funktion oder Option tut (z. B. „Versionen“, „Mindestalter“, „Quarantäne“) oder wo die Erklärung steht, muss sie suchen. Betroffen sind alle Nutzer, vor allem Personen ohne CLI-Kenntnisse, die nur die GUI benutzen.

Heute gibt es nur die Kürzel-Liste (`?`, `help_window` in `src/gui/app.rs`) und Tooltips an den Einstellungsfeldern.

## Goals
- [ ] Von jeder Seite (Übersicht, Analyse, Aufräumen, Verlauf, Einstellungen) und jeder Aufräum-Karte erreicht man mit einem Klick die passende Erklärung, ohne die GUI zu verlassen und ohne zu suchen.
- [ ] Die Kurztexte sind in einfachem Deutsch ohne CLI-Begriffe geschrieben: was die Funktion tut, was sie verändert, wie man es rückgängig macht.
- [ ] Zu jedem Kurztext gibt es einen Link zur ausführlichen Doku an der passenden Stelle.
- [ ] Neue Seiten und Karten können nicht ohne Hilfetext erscheinen (Test).

## Out of Scope
- Volltextsuche über die Doku
- Videos und Screenshots
- Weitere Sprachen (bisher nur Deutsch)
- Einführungsassistent / Tour beim ersten Start
- Inhaltliche Änderungen an `README.md` und `docs/dokumentation.md` (sie werden nur verlinkt; fehlende Anker dürfen ergänzt werden)

## User Journey
1. Nutzerin öffnet „Aufräumen“ und sieht neben der Überschrift und im Kopf jeder Karte ein „?“.
2. Sie klickt auf das „?“ der Karte „Versionen“ (oder drückt `F1` auf der Seite).
3. Das Hilfefenster (Erweiterung des bestehenden Kürzel-Hilfefensters) öffnet sich mit dem Kurztext zu „Versionen“: Was es tut, was geändert wird, wie man es rückgängig macht, wichtigste Optionen.
4. Unter dem Text steht „Ausführliche Doku öffnen“. Ein Klick öffnet die mitgelieferte Datei an der passenden Stelle im Standardprogramm.
5. `Esc` oder „Schließen“ beendet die Hilfe. Die Seite darunter ist unverändert.
6. Die Kürzel-Liste bleibt im selben Fenster als eigener Abschnitt („Tastenkürzel“).

## Acceptance Criteria
- [ ] Given eine beliebige Seite, when „?“ neben der Überschrift geklickt oder `F1` gedrückt wird, then öffnet das Hilfefenster mit dem Kurztext dieser Seite.
- [ ] Given die Seite „Aufräumen“, when „?“ im Kopf einer Karte geklickt wird, then zeigt das Fenster den Kurztext genau dieser Karte (sechs Karten: Duplikate, Müll, Leere Ordner, Archivieren, Versionen, Nach Regeln).
- [ ] Given ein Kurztext, when „Ausführliche Doku öffnen“ geklickt wird und die Datei existiert, then öffnet sie im Standardprogramm.
- [ ] Given die Doku-Datei fehlt oder kein Programm für `.md` ist eingerichtet, when der Link geklickt wird, then erscheint eine Meldung mit dem erwarteten Pfad zum Kopieren, kein Fehlerdialog und kein Absturz. Der Kurztext bleibt lesbar.
- [ ] Given ein Scan, Apply oder anderer Lauf ist aktiv, when die Hilfe geöffnet wird, then läuft der Task weiter und die Hilfe blockiert nichts.
- [ ] Given ein Bestätigungsdialog ist offen, when `Esc` gedrückt wird und die Hilfe offen ist, then schließt `Esc` zuerst die Hilfe; der Dialog bleibt unverändert.
- [ ] Given die Hilfe ist offen, when die Kürzel-Liste gewünscht wird (`?` bzw. Eintrag „Tastenkürzel“), then erscheint sie im selben Fenster.
- [ ] Given die GUI läuft, when die Hilfe benutzt wird, then entsteht keine Netzwerkverbindung (`netstat`-Prüfung wie in `manual-tests.md` Abschnitt 4).
- [ ] Ein Test prüft, dass jede Seite und jede Aufräum-Karte einen Hilfetext hat und dass jeder Doku-Verweis auf eine existierende Datei und einen existierenden Abschnitt zeigt.
- [ ] Das „?“ ist per Tab erreichbar und hat einen Tooltip („Hilfe zu …, F1“).

## Edge Cases & Error States
| Scenario | Expected Behavior |
|----------|-------------------|
| Doku-Datei liegt nicht neben der `exe` (z. B. nur die `exe` kopiert) | Meldung mit erwartetem Pfad; Kurztexte (in der `exe` eingebaut) funktionieren trotzdem |
| Kein Standardprogramm für `.md` | Meldung mit Pfad zum Kopieren statt Fehler |
| Hilfe während Scan, Apply, Undo | Task läuft weiter, kein Blockieren |
| Hilfe, während ein Dialog offen ist | `Esc` schließt zuerst die Hilfe |
| Abschnitt in der Doku wurde umbenannt | Test schlägt fehl (Anker prüfen), in der App öffnet der Link die Datei am Anfang |
| Dunkles Theme, Skalierung 100–200 % | Fenster und Text lesbar, nichts abgeschnitten, Fenster scrollt bei langem Text |
| Sehr schmales Fenster | Text bricht um, „Schließen“ bleibt erreichbar |
| Tastaturfokus in einem Textfeld, `F1` | Hilfe öffnet trotzdem (`F1` ist in Textfeldern frei); die Buchstaben-Kürzel bleiben dort wirkungslos |

## Technical Constraints
- Performance: Öffnen des Fensters ohne spürbare Verzögerung (Texte sind statisch); keine Last im Leerlauf.
- Security: Der Link öffnet nur Dateien im eigenen Programmordner (`docs\` neben der `exe`), keine frei zusammengesetzten Pfade aus Nutzereingaben; Datei wird nur zum Lesen vom Standardprogramm geöffnet. Keine Netzwerkverbindung, kein Nachladen.
- Compliance: –
- Plattform: nur Windows 10/11; Öffnen über die vorhandene Shell-Funktion (`platform::shell`).
- Darstellung: Farben nur über Theme-Tokens (`tests/gui_theme_static.rs` bleibt grün).

## Dependencies
- Bestehendes Hilfefenster (`help_window`, `src/gui/app.rs`) und Tastenzuordnung (`src/gui/keys.rs`, `F1` neu).
- `docs/dokumentation.md` und `README.md` als Ziel der Links; ein Verzeichnis `docs\` muss mit der `exe` ausgeliefert werden (Build/Release-Anleitung ergänzen).
- Seitenstruktur der GUI (Seiten, sechs Aufräum-Karten in `cleanup_cards.rs`).
- Reihenfolge: nach dem Layout-Fix `docs/bugs/cleanup-page-layout-fix-plan.md` (Fix-Plan wartet auf Freigabe), weil beide Features die Aufräumen-Seite und Kartenköpfe anfassen.

## Open Questions
- [ ] Sollen auch einzelne Optionen (z. B. „Mindestalter“) ein eigenes „?“ bekommen oder reicht der Kurztext der Karte? Vorschlag: Karte und Seite zuerst, Optionen über vorhandene Tooltips.
- [ ] Wo genau liegen die Doku-Ankerpunkte? Vorschlag: pro Funktion ein Abschnitt in `README.md`; Überschriften-Anker prüft der Test.
- [ ] Ob `.md` unter Windows ein Standardprogramm hat, ist nicht sicher; Alternative wäre, die Doku zusätzlich als `.html` auszuliefern (Browser). Im Plan zu entscheiden.
- [ ] Format der Kurztexte (Rust-Konstanten in `texts.rs` oder eine eingebundene Datei mit `include_str!`)? Im Implementierungsplan festlegen.
