---
title: "Analyse: dedupe-dirs — Code-Review Runde 2 (PR #8)"
status: analysed
created: 2026-10-09
updated: 2026-10-09
previous: dedupe-dirs-review-findings-analysis.md
---

# Analyse: dedupe-dirs — Code-Review Runde 2 (PR #8)

Zweiter `/code-review 8` nach den Fixes aus `dedupe-dirs-review-findings-fix-plan.md`. Zehn Befunde. Jeder wurde am Code geprüft; Befunde 1, 2, 4 und 8 zusätzlich mit Wegwerf-Tests reproduziert (nicht im Repo, Arbeitsbaum ist sauber). Die Erwartungen stammen aus `docs/features/ordner-duplikate.md`.

## Ergebnis auf einen Blick

| # | Befund | Urteil | Schwere | Fix nötig |
|---|--------|--------|---------|-----------|
| 1 | Gruppenreihenfolge: behaltener Ordner enthält entfernten Ordner | **bestätigt** (reproduziert, Planer und Apply) | mittel | ja |
| 2 | Quelle wird nur über Anzahl/Summe/jüngste mtime geprüft | **bestätigt** (reproduziert: Umbenennen bleibt unbemerkt) | niedrig–mittel | ja |
| 3 | Cache der Keep-Prüfung gilt für den ganzen Lauf | **teilweise**: wirkt nur über Befund 1 bzw. wie beim Datei-`dedupe` gewollt | niedrig | über Befund 1 mit |
| 4 | `.ordner-cleanup` nur als Ordnername ausgeschlossen | **bestätigt für den Bericht**, Planer ist geschützt; Elternordner-Teil widerlegt | niedrig | ja (Bericht) |
| 5 | Leere/Junk-Unterordner zählen nicht zur Gleichheit | **widerlegt**: Spec-Entscheidung | – | nein (Doku-Hinweis) |
| 6 | `Oldest` als Auffangzweig, vergleicht jüngste mtime | **widerlegt**: Spec-Entscheidung | – | nein |
| 7 | `all_linked` vs. `freed_bytes` | **widerlegt**: Spec-Entscheidung, Test vorhanden | – | nein |
| 8 | `short_path` überschreitet `max_chars` bei langem Kopf | **bestätigt** (reproduziert), kosmetisch, nicht Teil des Features | niedrig | ja (klein) |
| 9 | Performance (`locked`, mehrfache Walks, `hex`, doppeltes Laden) | **Optimierung, kein Fehler**; Messung steht aus | niedrig | optional |
| 10 | `contains_protected` nutzt `check_cached` statt `check` | **praktisch widerlegt**: gewollte Perf-Änderung, Lücke durch `has_link` gedeckt | – | nein (Test + Doku) |

## Befund 1: behaltener Ordner enthält einen entfernten Ordner (bestätigt)

**Ursache (Rang 1):** `plan_dedupe_dirs` sortiert die Gruppen nach der Tiefe ihres *flachsten Mitglieds* (`src/change/dedupe_dirs.rs:130`). Das ist kein echtes „von oben nach unten“. Liegt ein Mitglied einer Gruppe G1 tief in einem Ordner `Y` einer anderen Gruppe G2, während G1 ein flacheres Mitglied außerhalb hat, läuft G1 zuerst. G1 entfernt dann den Ordner in `Y`, danach behält G2 den Ordner `Y` — dessen `keep_fingerprint` (Index-Stand, `meta_print`, `dedupe_dirs.rs:48`) den entfernten Ordner noch enthält. Die Fixes aus Runde 1 (`locked`, `validate`) fangen nur den umgekehrten Fall (entfernter Ordner *enthält* ein `keep`).

**Reproduktion (Planer):** Index `W\f.txt` (mtime 1), `P\Y\x\f.txt` (5), `P\Z\x\f.txt` (10), gleicher Hash, Strategie `oldest`. Plan:
- `P\Y\x` (keep `W`)
- `P\Z` (keep `P\Y`)
- `P\Z\x` (keep `W`)

`P\Y` ist behalten, enthält aber die entfernte Quelle `P\Y\x`. Außerdem stehen `P\Z` und sein Unterordner `P\Z\x` beide im Plan.

**Reproduktion (Apply, echte Dateien):** Aktionen in Schlüsselreihenfolge: `P\Y\x` → `Done`, `P\Z` → `Skipped(KeepChanged)`, `P\Z\x` → `Done`. Der Lauf endet mit Exit-Code 2, der angekündigte Platzgewinn von `P\Z` tritt nicht ein. Mit anderen Ordnernamen (`Z` → `Y2`) sortiert `P\Y2` vor `P\Y\x`, und es geht gut: Das Ergebnis hängt vom Namen ab.

**Weitere Folgen derselben Ursache:**
- Verschachtelte Aktionen (`P\Z` und `P\Z\x`): ein Unterordner wird in einer früheren Gruppe entfernt, sein Elternordner in einer späteren. Beim Apply wird der Elternordner wegen geänderter Dateizahl `stale` oder der Unterordner `AlreadyDone`. Diesen Fall hatte ich nach Fix 2 schon gemeldet.
- `freed_bytes` (`dedupe_dirs.rs:312`) zählt die Bytes verschachtelt entfernter Ordner doppelt (`removed_dirs` enthält beide).
- `Plan::validate` prüft beides nicht.

**Fix-Richtung:**
1. **Reihenfolge:** Gruppen so sortieren, dass Gruppen mit *Vorfahren-Mitgliedern* zuerst laufen. Dafür taugt die Höhe des Teilbaums (größte Dateitiefe unterhalb des Ordners, nur nicht ignorierte Dateien): Ein Vorfahre hat immer eine strikt größere Höhe als jeder Nachfahre, Mitglieder einer Gruppe haben gleiche Höhe (gleicher Fingerabdruck), ein Vorfahre kann nie dieselbe Gruppe wie sein Nachfahre bilden. Sortierung: Höhe absteigend, dann Tiefe, Bytes, Mitglieder. Im Test `scratch` aus der Analyse: G2 `{P\Y, P\Z}` läuft zuerst, `P\Z` geht, `P\Y\x` und `P\Z\x` entfallen oder gelten als „in behaltenem/entferntem Ordner“.
2. **Absicherung in `Plan::validate`** (Spiegel zu Fix 2): Der Pfad einer Aktion darf nicht auf oder unter dem `keep` einer *anderen* Aktion liegen, und keine Quelle liegt unter der Quelle einer anderen (keine Verschachtelung).
3. `freed_bytes` ist danach automatisch richtig, weil nichts mehr verschachtelt ist; ein Test sichert das.

**Risiko:** Der bestehende Test `mitglied_im_behaltenen_ordner_wird_nie_entfernt` (`R\sub` wird entfernt, weil `P\sub` im behaltenen `P` liegt) muss grün bleiben. Die neue Reihenfolge ändert das Ergebnis dort nicht, weil `P`/`Q` höher sind als `P\sub`/`Q\sub`/`R\sub`.

## Befund 2: Quelle nur grob geprüft (bestätigt)

**Ursache:** `verify_dir_source` (`src/change/apply.rs:532`) vergleicht Dateianzahl, Summe und jüngste mtime (`summarize_dir`, `apply.rs:442`). Relative Pfade und Einzelgrößen fehlen. Der Plan trägt dafür kein Feld (`keep_fingerprint` gilt nur für den behaltenen Ordner).

**Reproduktion:** Plan für `Kopie von Projekt`, danach `a.txt` in `umbenannt.txt` umbenannt (Größe, Anzahl und mtime bleiben gleich). Apply: `Done`. Der Ordner stimmt nicht mehr mit dem Duplikat überein, wandert aber in die Quarantäne. Über Undo rückholbar; ein Datenverlust entsteht nicht. (Ein zweiter Versuch mit getauschten Größen scheiterte im Test am Setzen der Dateizeit unter Windows, `Zugriff verweigert`; der Umbenennen-Fall genügt als Nachweis.)

**Fix-Richtung:** Neues optionales Plan-Feld `source_fingerprint` (Hex, wie `keep_fingerprint`) für Aktionen mit `is_dir`; der Planer berechnet es mit `meta_print` für die Quelle, Apply bildet `dir_fingerprint` der Quelle und vergleicht (Abweichung → `Stale`). `PLAN_VERSION` bleibt 2 (optionales Feld mit `skip_serializing_if`); ältere Pläne ohne Feld prüfen wie bisher. `validate` verlangt das Feld nur bei neu erzeugten `dedupe-dirs`-Plänen nicht zwingend (Kompatibilität); Entscheidung im Fix-Plan. Hinweis: Das ändert die Plan-Datei und gehört in die Doku zum Plan-Format.

## Befund 3: Cache der Keep-Prüfung (teilweise)

`verify_keep_dir` (`apply.rs:629`) merkt sich das Ergebnis je Keep und Fingerabdruck für den Lauf. Das entspricht dem Datei-`dedupe` (`verify_keep`, gleiche Cache-Struktur) und ist so gewollt: Der Keep wird einmal je Lauf geprüft.

Eine Aktion kann den Keep einer anderen nur verändern, wenn ihre Quelle *im* Keep liegt, also nur in einem inkonsistenten Plan wie in Befund 1. Die Reproduktion dort zeigt `KeepChanged` ohne Cache-Beteiligung. Mit der Plan-Invariante aus Befund 1 (Quelle liegt nie unter einem `keep`) ist der Cache unbedenklich; ein späterer externer Eingriff während eines Laufs fällt unter die allgemeine Annahme „Dateisystem ändert sich während Apply nicht gleichzeitig“. **Kein eigener Fix**, der Cache bleibt.

## Befund 4: `.ordner-cleanup` im Bericht (bestätigt, Teil widerlegt)

**Bestätigt:** `folder_dups::analyze` schließt nur Ordner aus, deren *Name* `.ordner-cleanup` ist (`folder_dups.rs:176`). Unterordner (`.ordner-cleanup\quarantine\<Lauf>\Projekt`) bleiben Mitglied. Test: `Projekt\a.txt` und `.ordner-cleanup\quarantine\r1\Projekt\a.txt` ergeben eine Gruppe. `partial_pairs` hat dagegen den Filter `key.contains("\\.ordner-cleanup\\")` (`folder_dups.rs:375`). Der Planer ist nicht betroffen, weil der `Protector` jeden Pfad mit der Komponente `.ordner-cleanup` schützt (`protect.rs:258`) — der Bericht ruft den Protector aber nicht. Folge: Nach einem Apply zeigt der Bericht die Quarantäne-Kopie als „doppelten Ordner“ mit falschem Platzgewinn.

**Widerlegt:** Der Werkzeugordner liegt immer direkt unter der Wurzel (`quarantine.rs:19`), und die Wurzel ist nie Gruppenmitglied (`folder_dups.rs:164`). Sein Einfluss auf den Fingerabdruck eines Elternordners kommt nur vor, wenn ein Elternordner einer anderen Wurzel gescannt wird. Das ist kein Normalfall; trotzdem sinnvoll, ihn in `print_dir` wie `Print::Empty` zu behandeln.

**Fix-Richtung:** In `analyze` Schlüssel mit Komponente `.ordner-cleanup` ausschließen (gleiche Prüfung wie in `partial_pairs`, als gemeinsame Hilfsfunktion `is_tool_key`), und in `print_dir` ein Kind `.ordner-cleanup` überspringen.

## Befunde 5, 6, 7: Spec-Entscheidungen (widerlegt)

- **5, leere Unterordner:** `ordner-duplikate.md` Z. 48: „Ignoriert … leere Unterordner. Ignorierte Dateien bleiben Teil des Ordners und wandern mit in die Quarantäne.“ Das Modul `folder_dups.rs` dokumentiert es im Kopf. Kein Fehler. Optional: den Satz im Plan-Hinweis der GUI wiederholen („leere Unterordner und Müll wandern mit“).
- **6, `Oldest`:** Spec Z. 57: „`oldest` nach der jüngsten mtime aller Dateien im Ordner“. Der Auffangzweig `_ =>` deckt `Oldest` und `PathPrefix`; bei `PathPrefix` wählt `chooser.allowed` ohnehin nur Mitglieder im Präfix. Das Verhalten ist beschlossen. Namensvorschlag: Kommentar am Vergleich ergänzen.
- **7, Hardlinks:** Spec Z. 66 und 93: „Teilweise Hardlinks: Ordner wird geplant, Platzgewinn zählt nur echt freiwerdende Bytes.“ Genau das tut `freed_bytes`; der Test `teilweise_hardlinks_werden_geplant_und_zaehlen_nur_echten_platzgewinn` (`dedupe_dirs.rs`) deckt es ab. Ein zusätzlicher Hinweis „nur ein Teil bringt Platz“ wäre eine Funktionserweiterung, kein Fehler.

## Befund 8: `short_path` (bestätigt, kosmetisch)

`short_path` (`src/gui/format.rs:141`) behält den „Kopf“ (Laufwerk oder `\\Server\Freigabe\`) immer vollständig. Ist der Kopf selbst länger als `max_chars`, bleibt `budget` 0, `tail` leer, und der Fallback gibt `head…` zurück. Gemessen: `\\server\share` bei `max_chars = 8` ergibt 15 Zeichen; `\\sehr-langer-servername\freigabe\ordner\datei.txt` bei 12 ergibt 35 Zeichen. Der Befund im Review (zu wenige Trennzeichen) ist ein Sonderfall derselben Ursache. Betroffen sind nur schmale GUI-Spalten bei UNC-Pfaden.

**Fix-Richtung:** Ist `head_len + 1 > max_chars`, den Kopf selbst von hinten kürzen (`…` + Ende des Kopfs, ggf. nur der Dateiname). Test mit beiden Beispielen und der Invariante `chars() <= max_chars`. Gehört nicht zum Duplikat-Feature, ist aber klein.

## Befund 9: Performance (Optimierung)

Kein Fehler. Es gibt einen Perf-Test (`tests/perf_plans.rs`: 100.000 Dateien unter 3 s, Spec: 1 Mio. unter 30 s). Eine Messung in dieser Sitzung scheiterte, weil der Release-Build mit LTO das Zeitlimit überschritt; die aktuelle Laufzeit ist daher nicht belegt. Konkrete Stellen:
- `hex()` (`folder_dups.rs:62`): `format!` je Byte, ein String pro Hash-Byte. Einfach zu verbessern (vorbelegter String, Nachschlagetabelle) und ohne Risiko.
- `locked` (`dedupe_dirs.rs`): `kept.iter().any(...)` je Aufruf, im `min_by` pro Vergleich zweimal. Seit Fix 2 linear in der Zahl der behaltenen Ordner. Bei der Neuordnung aus Befund 1 ist ein Vorfahren-Set der behaltenen Ordner naheliegend.
- `subtree_files`/`tree.stats`/`contains_protected`: mehrfach je Ordner. Ergebnis je Mitglied einmal pro Gruppe berechnen.
- `file_hashes_under` und `Tree::new` laden Bericht und Planer jeweils neu; der Bericht läuft die Ordneranalyse immer (`report/build.rs:304`).

**Empfehlung:** `hex()` und das `locked`-Set zusammen mit Befund 1 erledigen. Alles weitere nur, wenn eine Messung (`cargo test --release --test perf_plans -- --ignored`) die Grenze reißt.

## Befund 10: `contains_protected` mit `check_cached` (praktisch widerlegt)

`check_cached` kam im Commit `23f9854` bewusst als Performance-Änderung in `tree.rs` (vorher `check` in `archive.rs`). Der Unterschied: Der letzte Pfadteil wird nicht über das Dateisystem aufgelöst (`protect.rs:212`). Eine Datei, die selbst Link auf ein geschütztes Ziel ist, bleibt dadurch unerkannt. Diese Lücke ist gedeckt:
- Datei-Links tragen `is_link` im Index; `Tree` setzt `has_link` für den Teilbaum (`tree.rs:103`), `plan archive` überspringt solche Ordner (`archive.rs:91`, `SkipReason::Link`), und die Ordneranalyse macht sie `Unverifiable`.
- Verzeichnis-Links und Ordnernamen prüft `check_inside` weiter streng.
- `apply` prüft immer streng mit `check` (`apply.rs`, `summarize_dir`).

Offen ist nur die fehlende Absicherung per Test und eine Doku-Zeile. **Kein Codefix.** Vorschlag: Test, dass ein Archiv-Plan und ein `dedupe-dirs`-Plan einen Ordner mit Datei-Link nicht aufnehmen, und im Kommentar von `contains_protected` die Begründung festhalten.

## Betroffene Stellen

- `src/change/dedupe_dirs.rs:130–200, 312` (Reihenfolge, `locked`, `freed_bytes`)
- `src/change/plan.rs:285–320` (`validate`), Plan-Format (`source_fingerprint`)
- `src/change/apply.rs:442, 532, 566–630` (`summarize_dir`, `verify_dir_source`, `dir_fingerprint`, `verify_keep_dir`)
- `src/analysis/folder_dups.rs:62, 85–125, 176, 375`
- `src/gui/format.rs:141`
- `src/change/tree.rs:166`

## Reproduktionen für die Fix-Tests

1. **#1 Planer:** `W\f.txt`(1), `P\Y\x\f.txt`(5), `P\Z\x\f.txt`(10), Strategie `oldest` → kein `keep` enthält eine andere Aktionsquelle, keine Quelle liegt unter einer anderen.
2. **#1 Apply:** Dateien wie oben; handgebauter Plan `[(P/Y/x→W), (P/Z→P/Y), (P/Z/x→W)]` wird von `validate` abgelehnt.
3. **#2:** Plan, dann Datei in der Quelle umbenennen → `Skipped(Stale)`, Quelle bleibt.
4. **#4:** Index mit `Projekt\a.txt` und `.ordner-cleanup\quarantine\r1\Projekt\a.txt` → keine Gruppe; Bericht ohne Eintrag.
5. **#8:** `short_path` mit UNC-Pfaden und kurzem `max_chars` → `chars().count() <= max_chars`.
6. **#10:** Archiv- und dedupe-dirs-Plan mit Datei-Link im Ordner → Ordner nicht geplant.

## Nächster Schritt

`/debug:fix-plan` für diese Analyse. Entscheidungen, die der Plan treffen muss:
- Befund 2: neues Plan-Feld `source_fingerprint` (optional) oder bewusst bei der groben Prüfung bleiben.
- Befund 9: Umfang (nur `hex()` und `locked`, oder Messung vorab).
- Befund 8 gehört nicht zum Duplikat-Feature; im selben Fix-Plan als eigener Schritt oder separat.
