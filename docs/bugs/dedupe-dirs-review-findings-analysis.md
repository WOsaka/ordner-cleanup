# Analyse: dedupe-dirs — Code-Review-Befunde 1–3 (PR #8)

Quelle: `/code-review 8`. Betrifft `plan dedupe-dirs` und dessen Apply. Analyse nur per Code-Lektüre; die Szenarien sind noch nicht mit Tests reproduziert (das geschieht im Fix per Rot-Grün-Test).

## Zusammenfassung

| # | Befund | Einschätzung |
|---|--------|--------------|
| 1 | `_Archiv` wird weder im Planer noch im Apply ausgeschlossen | bestätigt; aber Dateien-`dedupe` verhält sich genauso → **Designentscheidung nötig** |
| 2 | Spätere Gruppe kann Vorfahren eines behaltenen Ordners entfernen | wahrscheinlich bestätigt, Voraussetzung: Gruppe mit flacherem Mitglied außerhalb |
| 3 | `dir_fingerprint` → `None` wird immer zu `KeepMissing` | bestätigt, aber abgeschwächt: Meldungstext nennt „nicht nutzbar“ schon |

## Befund 1: `_Archiv` nicht ausgeschlossen

**Ursache (Rang 1):** `plan_dedupe_dirs` (`src/change/dedupe_dirs.rs:97`) übernimmt alle Gruppen aus `folder_dups::analyze` (`src/analysis/folder_dups.rs:147`) ohne Filter auf `<Wurzel>\_Archiv`. `verify_quarantine_dir` (`src/change/apply.rs:612`) hat keinen `is_in_archive`-Check, anders als `verify_move` (`apply.rs:645`). `Plan::validate` kennt `archive_key` nur für Move-Ziele (`src/change/plan.rs:235`, `:410`).

**Beleg:** `grep is_in_archive` trifft in `apply.rs` nur Zeile 645 (Move). `src/change/dedupe.rs` (Dateien) nutzt `_Archiv` ebenfalls nirgends. `docs/features/ordner-duplikate.md` sagt zu `_Archiv` nichts. Die Keep-Wahl (`Chooser::order`, `dedupe_dirs.rs:78`) bevorzugt `_Archiv` nicht.

**Auswirkung:** Ein archivierter Ordner (`_Archiv\2019\Projekt`) bildet mit einer Live-Kopie eine Gruppe. Je nach Namens-/Zeit-/Pfadregel wird die archivierte oder die Live-Kopie in Quarantäne verschoben. Eine Quarantäne in `_Archiv` ist nach Ablauf der Frist ein Verlust von Archivinhalt.

**Offene Entscheidung (für fix-plan):** Wahl zwischen
- A) `_Archiv` ist nie Mitglied einer Gruppe (Planer filtert, Apply meldet `InArchive` als Absicherung) — analog zu `archive`/`versions`/`rules`/`empty_dirs`, die `_Archiv` auslassen;
- B) `_Archiv` darf Mitglied sein, ist aber immer der **behaltene** Ordner.

Empfehlung: A (einfach, konsistent mit den anderen Planern). Gruppen, die dadurch unter zwei Mitglieder fallen, entfallen. Die Dateien-`dedupe` bleibt unverändert (nicht Teil dieser Analyse).

## Befund 2: Gruppenübergreifende Überschneidung

**Ursache (Rang 1):** `locked` (`dedupe_dirs.rs:185`) ist `within(&kept, key)` und prüft nur, ob ein Mitglied **auf oder unter** einem behaltenen Ordner liegt. Ob ein Mitglied einen behaltenen Ordner **enthält**, wird nirgends geprüft. `Plan::validate` prüft pro Aktion nur das eigene `keep` (`plan.rs:286`: nur ob `keep` selbst eine Aktionsquelle ist), nicht ob ein `keep` unter einer Quelle liegt.

**Wann es auftritt:** Gruppen werden nach dem flachsten Mitglied sortiert (`dedupe_dirs.rs:130`). Szenario:
- G1 = `{Z\k, A\B\k, A\B\k2}` (flachstes Mitglied Tiefe 1) wird zuerst bearbeitet und behält `A\B\k` (z. B. `--keep path:A`).
- G2 = `{A\B, M\N}` (flachstes Mitglied Tiefe 2) kommt danach. `A\B` liegt nicht *unter* einem behaltenen Ordner, ist also nicht `locked`, und kann entfernt werden — samt dem behaltenen `A\B\k`.

**Folgen:** Der Plan widerspricht sich. Beim Apply ist `A\B\k` als Keep entweder schon weg (`KeepMissing`) oder `A\B` wird zuerst verschoben, sodass spätere Aktionen auf fehlende Quellen laufen. `freed_bytes` zählt Bytes, die nicht frei werden. Es geht nichts verloren (der Inhalt steckt in der Quarantäne), aber das Ergebnis ist falsch und verwirrend.

**Fix-Richtung:** Im Planer ein Mitglied als nicht entfernbar behandeln, wenn es einen Eintrag aus `kept` enthält (Gegenstück zu `within`: ein `contains`-Check auf `kept`). Als Absicherung `Plan::validate`: kein `keep` einer Aktion darf auf oder unter der Quelle einer anderen Aktion liegen. Das ist auch gegen handgeschriebene Pläne nützlich.

**Unsicherheit:** Ob ein Mitglied, das ein `kept` enthält, stattdessen *behalten* werden soll (dann wäre G1s Keep sicher) oder nur übersprungen wird, ist Teil der Fix-Planung. Einfachste sichere Variante: nicht entfernen, als `GroupIncomplete` melden.

## Befund 3: `None` → immer `KeepMissing`

**Ursache (Rang 1):** `dir_fingerprint` (`apply.rs:566`) gibt in allen Fehlerfällen `None` zurück: Ordner fehlt, Link, Reparse-Punkt, Cloud-Platzhalter, `read_dir`-Fehler (z. B. Zugriff verweigert). `verify_keep_dir` (`apply.rs:604`) macht daraus pauschal `KeepMissing`.

**Abschwächung:** Der Text von `KeepMissing` lautet „behaltene Datei fehlt oder ist nicht nutzbar“ (`src/change/mod.rs:288`). Der Nutzer wird also nicht ganz in die Irre geführt. Der Text sagt aber „Datei“ statt „Ordner“ und unterscheidet die Ursachen nicht.

**`Unverifiable`:** Die Variante (`mod.rs:275`, Text: „Inhalt nicht prüfbar (Cloud-only oder ohne Hash)“) wird nirgends erzeugt (Review-Befund 4). Der Text passt für Cloud-Platzhalter, nicht für „Zugriff verweigert“.

**Fix-Richtung:** `dir_fingerprint` liefert ein `Result<u128, KeepProblem>` (oder ein kleines Enum): `Missing` (NotFound, kein Ordner) → `KeepMissing`; Link/Reparse-Punkt/Cloud-Platzhalter/sonstiger Lesefehler → `Unverifiable`. Dabei Text von `Unverifiable` so fassen, dass er auch Lesefehler abdeckt („Inhalt nicht prüfbar (Cloud-only, Link oder nicht lesbar)“). Hinweis: Das Testmuster in `apply.rs` zu `KeepMissing` (Zeilen 1776, 1809, 1904, 2582) muss geprüft werden, ob es Dateien- oder Ordnerfälle abdeckt.

## Betroffene Stellen

- `src/change/dedupe_dirs.rs:97–200` (Gruppenauswahl, `locked`, Keep-Wahl)
- `src/analysis/folder_dups.rs:147` (Gruppenbildung), `:265` (`within`)
- `src/change/apply.rs:566–629` (`dir_fingerprint`, `verify_keep_dir`, `verify_quarantine_dir`)
- `src/change/plan.rs:231–312` (`Plan::validate`)
- `src/change/archive.rs:37` (`is_in_archive`)
- `src/change/mod.rs:240, 255, 275–306` (`SkipReason`)

## Reproduktion (für die Fix-Tests)

1. **#1:** Index mit `Projekt\a.txt` und `_Archiv\2019\Projekt\a.txt` (gleicher Inhalt/Hash). `plan dedupe-dirs` erzeugt heute eine Aktion. Erwartet: keine. Für Apply: handgebauter Plan mit Quelle unter `_Archiv` → erwartet `Skipped(InArchive)`.
2. **#2:** G1 mit flachem Mitglied `Z\k` und `A\B\k`, `A\B\k2`; G2 mit `A\B` und `M\N` (gleicher Inhalt), `--keep path:A`. Erwartet: kein `keep` liegt unter der Quelle einer anderen Aktion; `Plan::validate` lehnt einen solchen Plan ab.
3. **#3:** Apply mit behaltenem Ordner, der (a) fehlt, (b) einen Link/Platzhalter enthält, (c) ein nicht lesbares Unterverzeichnis hat (Fake-FS). Erwartet: (a) `KeepMissing`, (b)/(c) `Unverifiable`.

## Nächster Schritt

`/debug:fix-plan` — dort zuerst Befund 1 (A oder B) entscheiden.
