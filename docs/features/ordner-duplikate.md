---
title: "Duplikate ganzer Ordner (plan dedupe-dirs)"
status: draft          # draft | approved | implemented
created: 2026-10-09
updated: 2026-10-09
---

# Feature: Duplikate ganzer Ordner (`plan dedupe-dirs`)

## Problem Statement
Komplette Ordnerkopien („Backup_alt“, „Kopie von Projekt“, „Projekt (2)“) gehören zu den größten Platzfressern. Das bestehende Datei-Dedupe (`plan dedupe`) findet ihre Dateien zwar, zerlegt eine Ordnerkopie aber in hunderte Einzelaktionen und lässt danach leere Ordnergerüste zurück. Das ist im Review unübersichtlich und macht nicht sichtbar, dass es sich um *eine* Kopie handelt.

Betroffen ist zunächst der Entwickler selbst, später Familie und Kollegen (GUI). Das Feature ist Abschnitt 7.1 der [`../roadmap.md`](../roadmap.md) und baut auf dem Plan → Apply → Undo-Fundament aus Phase 2 und 3 auf.

## Goals
- [ ] Neuer Befehl `plan dedupe-dirs <wurzel> [--keep oldest|newest|path:<prefix>] [--min-size <größe>] [--out <datei>]` erzeugt aus dem Index einen Plan, ohne etwas zu verändern
- [ ] Eine Quarantäne-Aktion pro doppeltem Ordner; `undo` stellt den Ordner vollständig her (Pfade, Inhalt, mtime)
- [ ] Gleichheit: dieselben relativen Dateipfade mit demselben Inhalts-Hash; der Name des Ordners selbst ist egal; Junk-Dateien zählen beim Vergleich nicht
- [ ] Welcher Ordner bleibt: Namens-Heuristik („Kopie“, „Backup“, „(2)“ …) zuerst, danach `--keep` wie bei `plan dedupe`
- [ ] Platzgewinn sichtbar: Plan, Terminal-Ausgabe und Bericht nennen die freiwerdenden Bytes; sortiert nach Platzgewinn
- [ ] Bericht: neuer Abschnitt „Ordner-Duplikate“ mit exakten Gruppen und „teilweise gleichen“ Ordner-Paaren (≥ 80 % gemeinsame Bytes, nur Hinweis), auch in JSON/CSV
- [ ] GUI: eigene Aufräumkarte „Doppelte Ordner“ mit Review wie die übrigen Karten; CLI und GUI erzeugen dasselbe Planformat
- [ ] `tests/roundtrip_random.rs` nimmt `dedupe-dirs` auf und bleibt grün (Plan ändert nichts, Apply ohne Fehler, Idempotenz, Undo stellt alles her)

## Out of Scope
- Teilmengen („Ordner A ist komplett in B enthalten“) als Aktion; sie erscheinen höchstens als „teilweise gleich“ im Bericht
- Teilweise gleiche Ordner im Plan (nur Hinweis im Bericht)
- Zusammenführen (Merge) zweier ähnlicher Ordner
- Ersetzen durch Hardlinks, ZIP (eigene Roadmap-Punkte)
- Vergleich nur über Dateiinhalte ohne Pfade (Umbenennen oder Umstrukturieren innerhalb der Kopie verhindert einen Treffer)
- Änderungen am Datei-Dedupe (`plan dedupe` bleibt unverändert und unabhängig)
- Änderungen am Health-Score (exakte Ordner-Duplikate stecken bereits im Abzug für Datei-Duplikate)
- Zusätzliches Lesen oder Hashen von Dateien (siehe „Technical Constraints“)

## User Journey
1. Nutzer scannt eine Wurzel (`scan` oder GUI „Scannen“).
2. Der Bericht zeigt im Abschnitt „Ordner-Duplikate“ die exakten Gruppen mit Platzgewinn sowie teilweise gleiche Paare als Hinweis, dazu die Empfehlung: „Erst doppelte Ordner bereinigen, dann einzelne Duplikate (`plan dedupe`).“
3. Nutzer ruft `plan dedupe-dirs <wurzel>` auf bzw. öffnet die Karte „Doppelte Ordner“. Ausgabe: Anzahl der Gruppen und Ordner, freiwerdender Platz, Pfad der Plan-Datei.
4. Review: pro Aktion entfernter Ordner, behaltener Ordner, Dateianzahl, Größe, Grund (`exact-duplicate-dir`, bei Heuristik-Wahl mit Hinweis wie „Name enthält ‚Kopie‘“). In der GUI an- und abwählbar.
5. `apply <plan>` prüft jeden Ordner und den behaltenen Ordner erneut, verschiebt den Ordner komplett in die Quarantäne und meldet je Aktion das Ergebnis.
6. Ein zweiter `scan` + `plan dedupe-dirs` ergibt einen leeren Plan. `plan dedupe` findet für die entfernten Ordner nichts mehr.
7. Bei Bedarf stellt `undo <run-id>` alle Ordner wieder her; `purge` leert die Quarantäne nach der Aufbewahrungsfrist wie bisher.

## Fachliche Regeln

### Gleichheit
- Zwei Ordner sind gleich, wenn die Menge der Paare *(relativer Dateipfad, case-insensitiv; Größe; voller Hash)* identisch ist.
- **Ignoriert** beim Vergleich: Dateien, die eine Junk-Kategorie aus `plan junk` trifft (z. B. `Thumbs.db`, `desktop.ini`, `.DS_Store`, `~$`-Lockdateien, `.tmp`), sowie leere Unterordner. Ignorierte Dateien bleiben Teil des Ordners und wandern mit in die Quarantäne.
- Ein Ordner braucht mindestens eine nicht ignorierte Datei mit Größe > 0. Leere Ordner und Ordner nur mit Junk sind keine Kandidaten (dafür gibt es `plan empty-dirs` und `plan junk`).
- Nur oberste Treffer: Sind A und B gleich, erscheinen ihre Unterordner-Paare (A\sub, B\sub) nicht zusätzlich. Ist ein Ordner Teil eines bereits geplanten Ordners, kommt er nicht noch einmal in den Plan.
- Gruppen mit mehr als zwei gleichen Ordnern: einer bleibt, alle anderen gehen in die Quarantäne.
- Ein Ordner und einer seiner eigenen Vorfahren oder Nachfahren bilden nie eine Gruppe.
- Die Wurzel selbst ist nie Kandidat (weder entfernt noch behalten).

### Welcher Ordner bleibt
1. **Namens-Heuristik:** Ein Ordner gilt als „Kopie-Name“, wenn sein Name (nur die letzte Komponente) eines dieser Muster enthält, case-insensitiv: `Kopie`, `Copy`, `Backup`, `Sicherung`, `alt`/`old` als eigenes Wort, ` (n)` am Ende, ` - Kopie`, `_bak`, `.bak`. Ordner ohne Kopie-Namen werden bevorzugt behalten. Die Musterliste ist in der Config erweiterbar (`dedupe_dirs_copy_patterns`).
2. Bei Gleichstand: `--keep` wie bei `plan dedupe` (Default `oldest` nach der jüngsten mtime aller Dateien im Ordner, `newest`, `path:<prefix>`), dann kürzester Pfad, dann alphabetisch.
3. Mit `path:<prefix>` hat das Präfix Vorrang vor der Heuristik. Liegt kein Ordner der Gruppe unter dem Präfix, bleibt die Gruppe unberührt (`group-incomplete`), wie bei `plan dedupe`.

### Teilweise gleiche Ordner (nur Bericht)
- Für Ordner-Paare mit gemeinsamen Dateien (gleicher Hash, Pfad egal): gemeinsame Bytes geteilt durch Bytes des kleineren Ordners (ohne Junk).
- Hinweis ab ≥ 80 % (Config `dedupe_dirs_partial_threshold`, Default `0.8`), exakte Gruppen ausgenommen.
- Bericht: Top 50 nach gemeinsamen Bytes im HTML, vollständig in JSON/CSV. Nur oberste Paare (Unterordner-Paare innerhalb eines bereits gelisteten Paars entfallen).

### Platzgewinn
- Summe der Dateigrößen der entfernten Ordner (inkl. ignorierter Junk-Dateien). Hardlinks werden wie bei `plan dedupe` behandelt: Bytes zählen nur, wenn alle Namen einer Datei-Identität entfernt werden.

## Acceptance Criteria
- [ ] Given zwei Ordner `Projekt` und `Kopie von Projekt` mit identischen Dateien, when `plan dedupe-dirs`, then enthält der Plan genau eine Aktion `quarantine` mit `is_dir: true` für `Kopie von Projekt`, `keep` = `Projekt`, `reason: exact-duplicate-dir`, und das Dateisystem ist unverändert.
- [ ] Given dieser Plan, when `apply --yes`, then liegt `Kopie von Projekt` komplett unter `quarantine\<run-id>\…` und `Projekt` ist unverändert; when `undo <run-id> --yes`, then ist der Baum byteidentisch zum Zustand vor dem Apply (Pfade, Größen, mtime, Inhalt) und unter `quarantine\<run-id>` bleibt nichts zurück.
- [ ] Given zwei sonst gleiche Ordner, von denen einer zusätzlich `Thumbs.db` und `desktop.ini` enthält, when `plan dedupe-dirs`, then gelten sie als gleich.
- [ ] Given zwei Ordner, die sich in einer Datei (Inhalt oder relativer Pfad) unterscheiden, when `plan dedupe-dirs`, then enthält der Plan keine Aktion für sie.
- [ ] Given drei gleiche Ordner, when `plan dedupe-dirs`, then bleibt genau einer und zwei sind im Plan.
- [ ] Given gleiche Ordner `A` und `B`, deren Unterordner `A\sub` und `B\sub` ebenfalls gleich sind, when `plan dedupe-dirs`, then enthält der Plan nur `B` (bzw. `A`), nicht zusätzlich `B\sub`.
- [ ] Given gleiche Ordner, von denen einer einen geschützten Pfad enthält, when `plan dedupe-dirs`, then wird dieser Ordner weder entfernt noch als `keep` gewählt (`skipped: protected`); bleibt kein zweiter zulässiger Ordner, enthält der Plan für die Gruppe keine Aktion.
- [ ] Given gleiche Ordner, von denen einer eine Cloud-only-Datei oder eine Datei ohne vollen Hash enthält, when `plan dedupe-dirs`, then ist dieser Ordner kein Kandidat und die Datei wird nie gelesen.
- [ ] Given ein Plan und danach eine neue, geänderte oder gelöschte Datei im zu entfernenden Ordner, when `apply`, then wird der Ordner als `stale` übersprungen, der Rest läuft, Exit-Code wie bei den übrigen Aktionen (2).
- [ ] Given ein Plan und danach eine Änderung oder Löschung im behaltenen Ordner, when `apply`, then wird nichts entfernt (`skipped: keep-changed` bzw. `keep-missing`).
- [ ] Given OneDrive-Wurzel und mehr Dateien in den geplanten Ordnern als `onedrive_max_move_files`, when `apply` ohne `--allow-large`, then bricht es ohne Änderung ab und nennt `--allow-large`.
- [ ] Given `Projekt`, `Projekt (2)`, `Backup Projekt`, alle gleich und `Projekt (2)` am ältesten, when `plan dedupe-dirs` (Default `oldest`), then bleibt `Projekt` (Heuristik vor Alter).
- [ ] Given ein Paar mit 85 % gemeinsamen Bytes, when `report`, then erscheint es im Abschnitt „Ordner-Duplikate“ unter „teilweise gleich“ und nicht im Plan; bei 75 % erscheint es nicht.
- [ ] Given `apply` des Plans, `scan` und erneut `plan dedupe-dirs`, then ist der Plan leer (Idempotenz).
- [ ] Given die GUI, when der Nutzer die Karte „Doppelte Ordner“ öffnet, plant, im Review einen Eintrag abwählt und anwendet, then wird nur der Rest angewendet; der GUI-Plan lässt sich per CLI `apply` anwenden und umgekehrt.
- [ ] `tests/roundtrip_random.rs` enthält `dedupe-dirs` in der Liste der Planer, die Zufallsbäume erzeugen Ordnerkopien, und alle Seeds sind grün.

## Edge Cases & Error States
| Scenario | Expected Behavior |
|----------|-------------------|
| Ordner enthält Cloud-only-Platzhalter oder ungehashte Datei | Kein Kandidat, Grund im Bericht (`skipped: unverifiable`); nie lesen oder hashen |
| Ordner enthält geschützten Pfad (`.git`, `node_modules`, Konfig) | Weder entfernen noch behalten (`protected`) |
| Alle Dateien des Kandidaten sind Hardlinks auf den behaltenen Ordner | Übersprungen (`hardlink`), kein Platzgewinn |
| Teilweise Hardlinks | Ordner wird geplant, Platzgewinn zählt nur echt freiwerdende Bytes |
| Ordner enthält Symlink/Junction | Kein Kandidat (`skipped: link`); Links werden nie verfolgt |
| Quarantäne-Ziel zu lang | `skipped: too-long` wie bei `plan dedupe` |
| Ordner zwischen Plan und Apply verändert | `stale`, Rest läuft weiter |
| Behaltener Ordner zwischen Plan und Apply verändert oder gelöscht | `keep-changed` / `keep-missing`, nichts wird entfernt |
| Ordner zwischen Plan und Apply schon entfernt | `already-done` wie bei den übrigen Aktionen |
| Datei im Ordner gesperrt (Apply) | Ordner-Verschiebung schlägt fehl, Ordner bleibt vollständig an Ort und Stelle (keine halbe Verschiebung), Grund im Ergebnis |
| Undo, aber an der Stelle liegt inzwischen ein neuer Ordner gleichen Namens | Kollision melden, nichts überschreiben (wie bei `archive`) |
| Groß-/Kleinschreibung unterscheidet sich in relativen Pfaden (`Bild.JPG` vs. `bild.jpg`) | Gilt als gleich (Windows ist case-insensitiv) |
| Ordner mit nur leeren Dateien oder nur Junk | Kein Kandidat |
| Zwei gleiche Ordner auf verschiedenen Ebenen (`A` und `X\Y\A`) | Normale Gruppe, Heuristik und `--keep` entscheiden |
| Wurzel ist gleich einem Unterordner von sich selbst (unmöglich) bzw. Vorfahr/Nachfahr | Nie eine Gruppe |
| Index älter als die Plan-Erzeugung bzw. kein Scan vorhanden | Gleiche Meldung wie die anderen Planer („erst scannen“) |

## Technical Constraints
- **Performance:** Kein zusätzliches Lesen von Dateien. Zwei gleiche Ordner bestehen ausschließlich aus Dateien, die auch einzeln Duplikate sind; der Index hat für sie bereits volle Hashes. Die Erkennung arbeitet nur auf dem Index: Ordner-Fingerabdruck aus den sortierten (relativer Pfad, Größe, Hash)-Paaren, Bottom-up über den Baum (Merkle-Prinzip). Ordner mit einer Datei, die in keiner Duplikat-Gruppe ist, scheiden sofort aus. Ziel: `plan dedupe-dirs` auf 1 Mio. indizierten Dateien unter 30 s (Messung in `tests/perf_plans.rs`).
- **Teilweise gleich:** Die Paarbildung läuft nur über Dateien in Duplikat-Gruppen; Gruppen mit sehr vielen Mitgliedern (z. B. dieselbe Lizenzdatei 5000-mal) werden für die Paarbildung gekappt, damit die Paaranzahl nicht quadratisch explodiert (Grenze im Implementierungsplan festlegen).
- **Security / Sicherheit:** Grundprinzipien gelten unverändert: Dry-Run als Standard, nie hart löschen (Quarantäne), geschützte Pfade, Cloud-only-Dateien nie lesen, Prüfung des Zustands beim Apply. Apply verschiebt den Ordner als Ganzes (ein `MoveFile` innerhalb des Volumes); schlägt das fehl, bleibt der Ordner unverändert.
- **Plan-Format:** bleibt v2. Neu ist die Kombination `type: quarantine` mit `is_dir: true` sowie das optionale Feld `keep_fingerprint` (Fingerabdruck des behaltenen Ordners) und `files` (Dateianzahl, wie bei `archive`). Ältere Programmversionen lehnen solche Pläne mit klarer Meldung ab, statt sie falsch auszuführen.
- **Prüfung beim Apply:** Für den zu entfernenden Ordner Dateianzahl, Summe und jüngste mtime wie bei `archive`; für den behaltenen Ordner Abgleich gegen `keep_fingerprint` über die Metadaten (relativer Pfad, Größe, mtime) ohne erneutes Hashen.
- **Compliance:** Keine Daten verlassen den PC; keine Netzwerkzugriffe.

## Dependencies
- Service / feature:
  - Phase 1: Index mit Duplikat-Gruppen und vollen Hashes (`index::dups`, `scan::duplicates`)
  - Phase 2/3: Plan-Format v2, Apply, Undo-Journal, Quarantäne, `stale`-Erkennung für Ordner (`change::archive`, `change::apply`, `change::undo`), geschützte Pfade (`change::protect`), OneDrive-Obergrenze (`change::limits`)
  - `change::junk` für die Junk-Kategorien, die beim Vergleich ignoriert werden
  - `plan dedupe` (`change::dedupe`) als Vorbild für `--keep`, Hardlinks und Platzgewinn
  - Bericht (`report::*`), GUI (`gui/views/cleanup_cards.rs`, `gui/review.rs`, `ops::plan`)
  - `tests/roundtrip_random.rs`

## Decisions
- [x] Gleichheit: gleicher Inhalt an gleichen relativen Pfaden, Ordnername egal
- [x] Aktion: ganzer Ordner in die Quarantäne (eine Aktion pro Ordner)
- [x] Erfolg: Platzgewinn sichtbar, teilweise gleiche als Hinweis, GUI-Karte, Rundreise-Test grün
- [x] Behalten: Namens-Heuristik vor `--keep` (Default `oldest`)
- [x] Junk-Dateien beim Vergleich ignorieren
- [x] Keine Mindestgröße (optional `--min-size`), sortiert nach Platzgewinn
- [x] `dedupe-dirs` und `dedupe` bleiben unabhängig; Bericht/GUI empfehlen die Reihenfolge
- [x] Teilweise gleich ab 80 % gemeinsamer Bytes (am kleineren Ordner), in der Config änderbar
- [x] Health-Score unverändert
- [x] Priorität: nächstes Feature, kein Termin

## Open Questions
- [ ] Kappungsgrenze für sehr große Duplikat-Gruppen bei der Paarbildung „teilweise gleich“ (Implementierungsplan)
- [ ] Endgültige Musterliste der Namens-Heuristik nach einem Test auf echten Ordnern prüfen (z. B. ob „alt“ als eigenes Wort zu Fehltreffern wie „Altbau“ führt; geplant ist Wortgrenze)
- [ ] Reihenfolge der Karten in der GUI: „Doppelte Ordner“ vor „Duplikate“ (Empfehlung des Berichts) oder nach Platzgewinn
