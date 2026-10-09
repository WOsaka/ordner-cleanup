---
title: "Fix-Plan: dedupe-dirs — Review-Befunde 1–3 (PR #8)"
status: fixed
fixed_at: 2026-10-09
created: 2026-10-09
updated: 2026-10-09
analysis: dedupe-dirs-review-findings-analysis.md
---

# Fix-Plan: dedupe-dirs — Review-Befunde 1–3

Entscheidung des Users zu Befund 1: **Variante A** — `_Archiv` ist nie Mitglied einer Gruppe. Der Planer filtert, Apply meldet `InArchive` als Absicherung.

Drei unabhängige Korrekturen, jede mit eigenem Commit (Rot-Grün-Test zuerst, `superpowers:test-driven-development`). Reihenfolge: 1, 2, 3.

## Fix 1: `_Archiv` ausschließen

### Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/change/dedupe_dirs.rs` | `plan_dedupe_dirs`, vor der Gruppenschleife (~Z. 128) | `archive_key = paths::dir_key(&root.join(ARCHIVE_DIR))` bilden (wie `rules.rs:224`, `empty_dirs.rs:79`). |
| `src/change/dedupe_dirs.rs` | Mitgliederschleife (Z. 165–175) | Mitglieder mit `paths::is_under(key, &archive_key)` **still überspringen** (kein `Skipped`-Eintrag, kein `had_protected`): `_Archiv` selbst und alles darunter. Bleiben weniger als zwei zulässige Mitglieder, greift die bestehende Prüfung `eligible.len() < 2`. Import `ARCHIVE_DIR` aus `super::plan`. |
| `src/change/apply.rs` | `verify_quarantine_dir` (Z. 612) | Nach dem `Protected`-Check: `if is_in_archive(root, Path::new(&a.path)) { return Err(Verdict::Skip(SkipReason::InArchive)); }` — vor `verify_dir_source`, damit nichts gelesen wird. `is_in_archive` ist schon importiert (Z. 12). |
| `src/change/mod.rs` | Doku-Kommentar `SkipReason::InArchive` (Z. 254) | Ergänzen: „(`move`, `dedupe-dirs`)“. Text bleibt. |
| `docs/features/ordner-duplikate.md` | Abschnitt zu Regeln/Ausschlüssen | Satz: „Ordner unter `_Archiv` nehmen nicht teil (weder als Duplikat noch als behaltener Ordner).“ |

Warum still und ohne Skip-Eintrag: `_Archiv` ist ein gewollter Ausschluss, kein Problemfall; die anderen Planer verfahren ebenso. Der Bericht (`report::folder`) nutzt weiter `folder_dups::analyze` ungefiltert und kann `_Archiv`-Gruppen zeigen. Er verändert nichts und bleibt **außerhalb** dieses Fixes (siehe Risiken).

### Tests
1. `src/change/dedupe_dirs.rs`: `ordner_unter_archiv_nehmen_nicht_teil` — Index mit `Z:\Root\Projekt\a.txt` und `Z:\Root\_Archiv\2019\Projekt\a.txt` (gleicher Hash) → leerer Plan, keine Skips. Vorher rot (Aktion vorhanden).
2. Dasselbe mit dreien: zwei Live-Kopien + eine im Archiv → Plan entfernt genau eine Live-Kopie, `keep` liegt nicht in `_Archiv`.
3. `_Archiv` selbst als Mitglied: Ordner `_Archiv` mit gleichem Inhalt wie ein anderer Ordner → keine Aktion.
4. `src/change/apply.rs`: `ordner_quelle_unterhalb_von_archiv_wird_nicht_entfernt` — `fx.dir_dedupe_plan` mit Quelle `_Archiv/2019/Projekt`, Keep `Projekt` → `Skipped(InArchive)`, Ordner bleibt, kein `intent` im Journal. Stil wie `quelle_unterhalb_von_archiv_wird_nicht_erneut_verschoben` (Z. 1656).

## Fix 2: Gruppenübergreifende Überschneidung

### Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/change/dedupe_dirs.rs` | `locked` (Z. 185) | Von `within(&kept, key)` zu „liegt in einem behaltenen Ordner **oder enthält einen behaltenen Ordner**“: `within(&kept, key) \|\| kept.iter().any(\|k\| paths::is_under(k, key))`. Der Name `locked` bleibt. Wirkung: Ein Mitglied, das einen behaltenen Ordner enthält, wird (a) nie entfernt (Filter Z. 199–203) und (b) in `Chooser::order` als „bereits behalten“ bevorzugt (Z. 83). So bleibt `A\B` erhalten, und `M\N` geht, statt dass die Gruppe aufgelöst oder der falsche Ordner gewählt wird. Wird `A\B` dadurch Keep, landet es in `kept` (Z. 244). |
| `src/change/dedupe_dirs.rs` | Kommentar am Modulkopf (Z. 3–6) | Ein Halbsatz: „und nichts entfernt wird, das einen behaltenen Ordner enthält“. |
| `src/change/plan.rs` | `Plan::validate`, zweite Schleife (Z. 285–310) | Absicherung für handgebaute/alte Pläne: Hat eine Aktion `keep` und ist `is_dir`, darf `keep` weder auf noch unter der Quelle einer **anderen** Ordner-Aktion liegen. Umsetzung mit vorhandenem `moved_with_dir(&dir_moves, a.id, &keep_key)` (Z. 455); Fehler: „Aktion {id}: der behaltene Ordner {keep} liegt in einem Ordner, der selbst entfernt wird“. Zusätzlich: `keep` auf/unter der **eigenen** Quelle ist ungültig (`ancestors_inclusive`-Check gegen `path_key(a.path)`). Dateien-Pläne bleiben unberührt, weil `dir_moves` nur `is_dir`-Aktionen enthält. |

Hinweis zum Fall „zwei Mitglieder enthalten je einen behaltenen Ordner“: beide sind `locked`, der Chooser wählt per `order` eines als Keep, das andere wird nicht entfernt und stillschweigend übergangen (gleiches Verhalten wie bisher bei `locked`). Das ist sicher; falls es zu leise wirkt, später als `GroupIncomplete` melden (nicht Teil dieses Fixes).

### Tests
1. `dedupe_dirs.rs`: `mitglied_mit_behaltenem_unterordner_wird_nie_entfernt` — Aufbau aus der Analyse: G1 = `Z\k`, `A\B\k`, `A\B\k2`; G2 = `A\B`, `M\N` mit gleichem Inhalt, Strategie `path:Z:\Root\A`. Erwartung: kein `keep` liegt auf/unter der Quelle einer anderen Aktion (Hilfsprüfung im Test über alle Aktionspaare); `A\B` wird nicht entfernt. Vorher rot. Falls der Aufbau die Lücke nicht trifft (Sortierung/Keep-Wahl), den Test an die tatsächliche Reihenfolge anpassen; erst wenn er rot ist, fixen.
2. `dedupe_dirs.rs`: Bytes-Summe `freed_bytes` stimmt mit den entfernten, nicht überlappenden Ordnern überein (kein Doppelzählen).
3. `plan.rs`: `validate` lehnt einen dedupe-dirs-Plan ab, dessen `keep` unter der Quelle einer anderen Aktion liegt; lehnt `keep` unter der eigenen Quelle ab; akzeptiert den normalen Plan (bestehende Tests bleiben grün).
4. Bestehender Test `mitglied_im_behaltenen_ordner_wird_nie_entfernt` (Z. 572) und `drei_gleiche_ordner_behalten_genau_einen` müssen unverändert grün bleiben.

## Fix 3: `Unverifiable` statt `KeepMissing`

### Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/change/apply.rs` | `dir_fingerprint` (Z. 566) | Rückgabe `Result<u128, KeepProblem>` mit privatem `enum KeepProblem { Missing, Unusable }`. `Missing`: Metadaten `NotFound`, oder kein Ordner (`!is_dir`), oder `read_dir` mit `NotFound` (verschwunden während der Prüfung). `Unusable`: Link, Reparse-Punkt, Cloud-Platzhalter (Ordner oder Eintrag darin), jeder andere Metadaten-/`read_dir`-Fehler (z. B. Zugriff verweigert), nicht auswertbarer Dateiname/relativer Pfad. |
| `src/change/apply.rs` | `verify_keep_dir` (Z. 595) | Cache bleibt `HashMap<String, Option<SkipReason>>`. Abbildung: `Err(Missing)` → `KeepMissing`, `Err(Unusable)` → `Unverifiable`, abweichender Abdruck → `KeepChanged`. |
| `src/change/mod.rs` | `SkipReason::Unverifiable` Text (Z. 306) | „Inhalt nicht prüfbar (Cloud-only, Link oder nicht lesbar)“. `KeepMissing` bleibt unverändert (viele Tests und Meldungen hängen daran). Doku-Kommentar von `Unverifiable` anpassen: „`dedupe-dirs`: behaltener Ordner nicht prüfbar (Cloud-only, Link oder nicht lesbar)“. Der Serialisierungsname (`unverifiable`) bleibt; der vorhandene Test auf Teilstring „nicht prüfbar“ (Z. 395) bleibt grün. |
| `src/change/fsops.rs` | Testmodul `FaultyFs` | Neue `Op::ReadDir`: `fail(Op::ReadDir, pfad)` lässt `read_dir` auf diesem Pfad mit Zugriff-verweigert (`from_raw_os_error(5)`) scheitern. Nur im Testcode. |
| `docs/features/ordner-duplikate.md` | Tabelle der Skip-Gründe | `unverifiable` auch für „behaltener Ordner beim Apply nicht lesbar/Link/Platzhalter“ nennen. |

### Tests
1. Bestehend, bleibt: `fehlender_behaltener_ordner_verhindert_das_entfernen` → `KeepMissing` (Z. 2573).
2. Neu `apply.rs`: behaltener Ordner enthält Cloud-Platzhalter (`FaultyFs::cloud_only(Projekt/a.txt)`, `run_with`) → `Skipped(Unverifiable)`, Quelle bleibt, nichts gelesen/gehasht.
3. Neu: `FaultyFs::cloud_only(Projekt)` (Ordner selbst) → `Unverifiable`.
4. Neu: `fail(Op::ReadDir, Projekt/sub)` → `Unverifiable`.
5. Neu: behaltener „Ordner“ ist eine Datei (statt Ordner) → `KeepMissing`.
6. Zwei Aktionen mit demselben Keep: Verdict wird gecacht und gilt für beide (bestehendes Verhalten, Test `zwei_kopien_desselben_ordners_gehen_beide` bleibt grün).

## Gesamtablauf und Prüfung
1. Pro Fix: roter Test → Umsetzung → `cargo test` für das Modul → Commit auf `dev` (kurze Message, ohne Footer laut CLAUDE.md).
2. Zum Schluss: `cargo fmt`, `cargo clippy -- -D warnings`, volles `cargo test` (inkl. Rundreise-Test mit Zufallsbäumen; dort ggf. `_Archiv`-Bäume prüfen, falls der Generator es anlegt).
3. `superpowers:verification-before-completion` vor `status: fixed`.
4. Push auf `dev`; PR #8 aktualisiert sich selbst. Kurzer Kommentar im PR zu den behobenen Befunden 1–3.

## Risiken
- **Bericht zeigt weiter `_Archiv`-Gruppen** (Fix 1): Bericht und Plan können abweichen. Bewusst nicht Teil des Fixes; bei Bedarf als eigene Aufgabe (Filter in `folder_dups::analyze` über `FolderDupOptions`, würde auch die Berichtszahlen ändern).
- **Dateien-`dedupe` behandelt `_Archiv` weiter nicht gesondert** (unverändert, außerhalb des Umfangs). Eine Folgefrage für die Roadmap.
- **`locked` wirkt breiter** (Fix 2): Es entfernt weniger. Mögliche Folge: Ein Duplikat-Paar bleibt unberührt, wenn beide Mitglieder behaltene Ordner enthalten. Das ist die sichere Seite.
- **`validate` strenger** (Fix 2): Bereits gespeicherte dedupe-dirs-Pläne mit überlappenden Aktionen werden abgelehnt. Solche Pläne sind erst mit PR #8 entstanden und noch nicht released; kein Migrationsbedarf.
- **Neue Skip-Zuordnung** (Fix 3): Wer auf `KeepMissing` für Platzhalter/Links filtert, sieht jetzt `Unverifiable`. Im Repo gibt es dafür keinen Verbraucher außer den Anzeigetexten (Grep: nur `mod.rs`).

## Rollback
Drei getrennte Commits, einzeln mit `git revert` zurücknehmbar. Keine Änderung am Plan-Format (`PLAN_VERSION` bleibt 2), keine neuen Felder. Fix 1 und 3 ändern nur Planer-/Apply-Entscheidungen, Fix 2 zusätzlich eine strengere Validierung; ein Revert stellt das Verhalten von PR #8 wieder her.
